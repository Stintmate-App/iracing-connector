//! The live frame: what happens on track now, in Pitwall's vocabulary and units.
//!
//! The recorder copies a [`LiveSnapshot`] for every row (plain bytes, no allocation). The JSON
//! frame is built from the latest snapshot only when a client asks for it, on the handler's
//! thread: [`frame_json`] allocates freely.

use serde_json::{Map, Value, json};

use crate::io::StallTyres;
use crate::meta::{DriverEntry, SessionMeta};
use crate::tick::Tick;
use crate::tracker::{LapView, State};
use crate::vars::CARS;
use irsdk::defs::{UNLIMITED_LAPS, UNLIMITED_TIME, session_flags};

/// Seconds without a new row after which a driving or garage state shows as `paused`.
pub const FROZEN_S: f64 = 1.0;

/// `CarIdxTrackSurface` of a car that is not in the world.
const LOC_NOT_IN_WORLD: i32 = -1;
/// `CarIdxTrackSurface` of a car in its pit stall.
const LOC_IN_PIT_STALL: i32 = 1;
/// `SessionState` from which a race has started (the green flag, racing).
const STARTED_FROM: i32 = 4;

/// The inputs of a frame, copied from the loop for every row. A fixed-size value without
/// pointers, so a double buffer can publish it with a plain copy.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LiveSnapshot {
    /// The row.
    pub tick: Tick,
    /// The lap view after the row.
    pub view: LapView,
    /// The generation of the session data the loop was using, so a reader can tell it is current.
    pub meta_generation: u64,
    /// The tyre readings of the last stall entry, when there was one.
    pub tyres: Option<StallTyres>,
    /// Wall clock of the row, seconds since the Unix epoch.
    pub wall_s: f64,
    /// Monotonic clock of the row, seconds.
    pub mono_s: f64,
}

impl LiveSnapshot {
    /// A snapshot of nothing: the simulator is gone.
    pub fn empty() -> LiveSnapshot {
        LiveSnapshot {
            tick: *Tick::new(),
            view: crate::tracker::Tracker::new(0).view(),
            meta_generation: 0,
            tyres: None,
            wall_s: 0.0,
            mono_s: 0.0,
        }
    }
}

/// Rounds to `places` decimals so single-precision values read cleanly in JSON.
fn round(x: f32, places: i32) -> f64 {
    let k = 10f64.powi(places);
    (f64::from(x) * k).round() / k
}

fn positive(x: f32, places: i32) -> Value {
    if x.is_finite() && x > 0.0 {
        json!(round(x, places))
    } else {
        Value::Null
    }
}

fn id(s: &str) -> Value {
    if s.is_empty() { Value::Null } else { json!(s) }
}

/// The frame as the connector protocol defines it.
///
/// `now_mono_s` is the monotonic time of the request (it gives `data_age_s` and the paused
/// state) and `iso` the row's wall time as ISO 8601 text with an offset. Without session data
/// while a session is open the phase is `menus`, which carries only `sim`, `phase`, `taken_at`
/// and `data_age_s`.
pub fn frame_json(
    s: &LiveSnapshot,
    meta: Option<&SessionMeta>,
    now_mono_s: f64,
    iso: &str,
) -> Value {
    let age = (now_mono_s - s.mono_s).max(0.0);
    let head = |phase: &str| {
        json!({
            "sim": "iracing",
            "phase": phase,
            "taken_at": iso,
            "data_age_s": round_f64(age, 3),
        })
    };
    let in_session = matches!(s.view.state, State::Garage | State::Pits | State::Track);
    let (Some(meta), true) = (meta, in_session) else {
        return head("menus");
    };
    let (Some(player), true) = (meta.player.as_ref(), !meta.track.track_id.is_empty()) else {
        return head("menus");
    };
    let phase = if age >= FROZEN_S {
        "paused"
    } else if s.view.state == State::Garage {
        "garage"
    } else {
        "on_track"
    };
    let mut frame = head(phase);
    let o = frame.as_object_mut().expect("object");
    o.insert("session".into(), session(s));
    o.insert("track".into(), track(meta));
    o.insert("car".into(), car(s, meta, player));
    o.insert("lap".into(), lap(s));
    if let Some(splits) = splits(s, meta, player) {
        o.insert("splits".into(), splits);
    }
    o.insert("conditions".into(), conditions(&s.tick));
    o.insert("participants".into(), participants(s, meta));
    frame
}

fn round_f64(x: f64, places: i32) -> f64 {
    let k = 10f64.powi(places);
    (x * k).round() / k
}

fn session(s: &LiveSnapshot) -> Value {
    let t = &s.tick;
    let laps = t.laps_total.filter(|l| *l > 0 && *l < UNLIMITED_LAPS);
    let total = t
        .time_total
        .filter(|x| *x > 0.0 && *x < f64::from(UNLIMITED_TIME));
    let total = total.filter(|_| laps.is_none());
    json!({
        "type": s.view.session_kind.as_str(),
        "time_remaining_s": total.and(t.time_remain).map(|r| round_f64(r.max(0.0), 1)),
        "laps_in_event": laps,
        "duration_s": total,
        "additional_laps": null,
        "started": t.session_state >= STARTED_FROM,
    })
}

fn track(meta: &SessionMeta) -> Value {
    let t = &meta.track;
    json!({
        "track_id": t.track_id,
        "layout_id": t.layout_id,
        "track_name": t.track_name,
        "layout_name": t.layout_name,
        "length_m": t.length_m.filter(|l| *l > 0.0),
    })
}

fn car(s: &LiveSnapshot, meta: &SessionMeta, player: &DriverEntry) -> Value {
    let t = &s.tick;
    let driving = matches!(s.view.state, State::Pits | State::Track);
    // The simulator reads 0 out of the car; that is "not known", not an empty tank.
    let fuel = (driving || t.fuel_l > 0.0).then(|| round(t.fuel_l.max(0.0), 2));
    let flags = match (t.car_flags, usize::try_from(meta.player_idx)) {
        (Some(all), Ok(i)) if i < CARS => all[i],
        _ => t.session_flags,
    };
    let mut car = json!({
        "car_id": player.car_id,
        "class_id": id(&player.class_id),
        "car_name": player.car_name,
        "class_name": player.class_name,
        "pit_mode": s.view.pit_mode,
        "speed_kmh": round(t.speed.max(0.0) * 3.6, 1),
        "gear": t.gear.max(-1),
        "rpm": round(t.rpm.max(0.0), 0),
        "fuel_l": fuel,
        "fuel_capacity_l": meta.fuel_capacity_l.filter(|c| *c > 0.0),
        "penalty": (flags & session_flags::BLACK != 0).then_some("other"),
    });
    if let Some(w) = wheels(s, meta) {
        car["wheels"] = w;
    }
    car
}

/// The wheels of the last stall entry: temperatures and wear then, hot pressure from the
/// session text (known only after a garage visit). `None` until a stall entry happened.
fn wheels(s: &LiveSnapshot, meta: &SessionMeta) -> Option<Value> {
    let st = s.tyres.as_ref()?;
    let names = ["fl", "fr", "rl", "rr"];
    let mut out = Map::new();
    for (w, name) in names.iter().enumerate() {
        let temps = st.temp_c[w];
        // Readings are left, middle, right; the left wheels' outside is the left reading.
        let (out_c, in_c) = if w % 2 == 0 {
            (temps[0], temps[2])
        } else {
            (temps[2], temps[0])
        };
        let wear = st.wear[w].iter().sum::<f32>() / 3.0;
        let pressure = meta
            .setup
            .as_ref()
            .and_then(|set| set.readings.wheels[w].hot_pressure_kpa)
            .filter(|p| *p > 0.0);
        out.insert(
            (*name).into(),
            json!({
                "tread_out_c": round(out_c, 1),
                "tread_mid_c": round(temps[1], 1),
                "tread_in_c": round(in_c, 1),
                "pressure_kpa": pressure,
                "carcass_c": round(temps.iter().sum::<f32>() / 3.0, 1),
                "wear": round((1.0 - wear).clamp(0.0, 1.0), 3),
                "compound": null,
                "brake_c": null,
            }),
        );
    }
    Some(Value::Object(out))
}

fn lap(s: &LiveSnapshot) -> Value {
    let (t, v) = (&s.tick, &s.view);
    let time = if v.timed {
        Some(round(v.lap_clock_s, 3))
    } else if v.state == State::Garage && t.lap_current_time.is_some_and(|c| c > 0.0) {
        t.lap_current_time.map(|c| round(c, 3))
    } else {
        None
    };
    json!({
        "current_lap": t.lap.max(0),
        "laps_completed": t.lap_completed.max(0),
        "distance_m": round(t.lap_dist, 1),
        "time_s": time,
        "valid": v.valid,
        "sector": v.sector.map(|i| u32::from(i) + 1),
        "last_lap_s": positive(t.lap_last_time, 3),
        "best_lap_s": t.lap_best_time.map_or(Value::Null, |b| positive(b, 3)),
        "lap_type": v.lap_type.map(|l| l.as_str()),
        "invalid_cause": v.cause.map(|c| c.as_str()),
        "invalid_cause_certain": v.cause_certain,
        "invalid_at_m": v.invalid_at_m.map(|m| round(m, 1)),
    })
}

/// Whether car `i` is a real car in the world (not the pace car, not absent).
fn real(meta: &SessionMeta, t: &Tick, i: usize) -> bool {
    meta.roster[i].as_ref().is_some_and(|d| d.is_real)
        && i as i32 != meta.pace_car_idx
        && t.car_loc[i] != LOC_NOT_IN_WORLD
}

/// The time gaps to the cars ahead and behind. In a race, the difference of the cars' times
/// behind the leader by overall position; otherwise the on-track gap to the nearest car by lap
/// fraction, wrapped by the class lap estimate.
fn splits(s: &LiveSnapshot, meta: &SessionMeta, player: &DriverEntry) -> Option<Value> {
    let t = &s.tick;
    let me = usize::try_from(meta.player_idx)
        .ok()
        .filter(|i| *i < CARS)?;
    let gap = |x: f32| (x.is_finite() && x > 0.0).then(|| round(x, 2));
    let (ahead, behind) = if s.view.session_kind == crate::codes::SessionKind::Race {
        let (pos, f2) = (t.car_position?, t.car_f2?);
        let p = pos[me];
        let by_pos = |want: i32| {
            (p > 0 && want > 0)
                .then(|| (0..CARS).find(|&i| i != me && pos[i] == want && real(meta, t, i)))
                .flatten()
        };
        (
            by_pos(p - 1).map(|i| f2[me] - f2[i]),
            by_pos(p + 1).map(|i| f2[i] - f2[me]),
        )
    } else {
        let est = t.car_est?;
        let lap_s = player.class_est_lap_s.filter(|l| *l > 0.0)? as f32;
        let mut ahead: Option<(f32, usize)> = None;
        let mut behind: Option<(f32, usize)> = None;
        for i in (0..CARS).filter(|&i| i != me && real(meta, t, i)) {
            let d = (t.car_pct[i] - t.car_pct[me]).rem_euclid(1.0);
            if d > 0.0 && ahead.is_none_or(|(b, _)| d < b) {
                ahead = Some((d, i));
            }
            let back = 1.0 - d;
            if back > 0.0 && d > 0.0 && behind.is_none_or(|(b, _)| back < b) {
                behind = Some((back, i));
            }
        }
        (
            ahead.map(|(_, i)| (est[i] - est[me]).rem_euclid(lap_s)),
            behind.map(|(_, i)| (est[me] - est[i]).rem_euclid(lap_s)),
        )
    };
    Some(json!({
        "ahead_s": ahead.and_then(gap),
        "behind_s": behind.and_then(gap),
    }))
}

fn conditions(t: &Tick) -> Value {
    json!({
        "ambient_c": t.air_c.map(|x| round(x, 1)),
        "track_c": t.track_c.map(|x| round(x, 1)),
        "rain": t.precip.map(|p| round(p.clamp(0.0, 1.0), 3)),
        "wind_speed_ms": t.wind_ms.map(|x| round(x.max(0.0), 1)),
    })
}

fn participants(s: &LiveSnapshot, meta: &SessionMeta) -> Value {
    let t = &s.tick;
    let length = meta.track.length_m.unwrap_or(0.0) as f32;
    let me = usize::try_from(meta.player_idx).ok();
    let mut out = Vec::new();
    for i in 0..CARS {
        let Some(d) = meta.roster[i].as_ref().filter(|d| d.is_real) else {
            continue;
        };
        let is_player = me == Some(i);
        let result = &meta.results[i];
        if i as i32 == meta.pace_car_idx
            || !(is_player || t.car_loc[i] != LOC_NOT_IN_WORLD || result.position > 0)
        {
            continue;
        }
        let active = t.car_loc[i] != LOC_NOT_IN_WORLD;
        let position = t
            .car_position
            .map(|p| p[i])
            .filter(|p| *p > 0)
            .or((result.position > 0).then_some(result.position));
        let pit_mode = if is_player {
            u32::from(s.view.pit_mode)
        } else if !active {
            4
        } else if t.car_loc[i] == LOC_IN_PIT_STALL {
            2
        } else if t.car_on_pit_road[i] {
            // Whether it has already been in its stall is not tracked per car.
            1
        } else {
            0
        };
        out.push(json!({
            "index": i,
            "name": d.name,
            "is_player": is_player,
            "is_safety_car": false,
            "active": active,
            "position": position,
            "laps_completed": t.car_lap_completed[i].max(0),
            "current_lap": t.car_lap[i].max(0),
            "distance_m": round(t.car_pct[i].max(0.0) * length, 1),
            "sector": if is_player { s.view.sector.map(|x| u32::from(x) + 1) } else { None },
            "pit_mode": pit_mode,
            "car_id": id(&d.car_id),
            "class_id": id(&d.class_id),
            "last_lap_s": t.car_last.map(|l| l[i]).and_then(|x| positive(x, 3).as_f64()),
            "best_lap_s": t.car_best.map(|l| l[i]).and_then(|x| positive(x, 3).as_f64()),
        }));
    }
    Value::Array(out)
}
