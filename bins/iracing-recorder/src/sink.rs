//! The adapter from the core's [`Sink`] to the runtime's [`WriteSink`].
//!
//! Lap and setup messages are built on the writer thread: the closures handed to the
//! `WriteSink` own the lap's channel columns and the setup snapshot.

use std::borrow::Cow;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use iracing_core::catalog::{cars_only, session_catalog};
use iracing_core::io::{
    FinishedLap, ParticipantLap, RepairEvent, SessionOpenInfo, SetupReason, Sink as CoreSink,
    StallOccasion, StallTyres,
};
use iracing_core::lap::{Columns, Present};
use iracing_core::meta::SessionMeta;
use iracing_core::positions::file::{LapRecord, PositionsHeader, encode_closed, encode_lap};
use iracing_core::resume::{IrClock, identity_of};
use iracing_core::sim_data::{REPAIR, STALL_TYRES};
use serde_json::{Map, Value, json};
use stintmate_connector_link::sink::{SessionOpen, WriteSink};
use stintmate_connector_wire::messages as wire;

use crate::positions;

/// Receives the session clock before a session may open (for resuming it after a restart).
pub trait NoteClock {
    /// The clock of the row being processed, while no session is open.
    fn note_clock(&mut self, _clock: IrClock) {}
}

/// The recorder's sink: the core's records to the Pitwall write API through the outbox.
pub struct TrackerSink {
    /// The runtime's sink (status, resume and closing go through it).
    pub write: WriteSink<IrClock>,
    /// The positions file of each open session, by local session number.
    positions: HashMap<u64, PathBuf>,
    /// The folder of the positions files, next to the outbox.
    positions_dir: PathBuf,
    /// Set while the session may be resumed by the next recorder: its end is not written to the
    /// positions file then, because the session goes on.
    pub keep_positions_open: bool,
}

impl TrackerSink {
    /// Wraps the runtime's sink.
    pub fn new(write: WriteSink<IrClock>) -> TrackerSink {
        let positions_dir = positions::dir_of(&write.outbox_dir());
        TrackerSink {
            write,
            positions: HashMap::new(),
            positions_dir,
            keep_positions_open: false,
        }
    }

    /// Opens the positions file of the session just started, when the session names its track and
    /// car (the disk files are matched by them).
    fn open_positions(&mut self, local: u64, info: &SessionOpenInfo) {
        let Some(session_key) = self.write.session_key(Some(local)) else {
            return;
        };
        let meta = &info.meta;
        let track_id = meta.track.layout_id.parse::<i64>().ok().filter(|t| *t > 0);
        let car_path = meta
            .player
            .as_ref()
            .map(|p| p.car_id.clone())
            .filter(|c| !c.is_empty());
        let (Some(track_id), Some(car_path)) = (track_id, car_path) else {
            return;
        };
        let header = PositionsHeader {
            session_key,
            session_num: info.key.session_num,
            session_unique_id: info.key.unique_id,
            sub_session_id: meta.sub_session_id,
            track_id,
            car_path,
            opened_unix: positions::unix_now(),
        };
        if let Some(path) = positions::open_file(&self.positions_dir, &header) {
            self.positions.insert(local, path);
        }
    }
}

/// The `lap_positions` message of a lap's exact positions.
pub fn positions_message(p: iracing_core::positions::job::LapPositions) -> wire::LapPositions {
    wire::LapPositions {
        channels: wire::LapChannels(vec![
            (Cow::Borrowed("x"), p.x),
            (Cow::Borrowed("z"), p.z),
            (Cow::Borrowed("y"), p.y),
        ]),
    }
}

impl NoteClock for TrackerSink {
    fn note_clock(&mut self, clock: IrClock) {
        self.write.note_clock(clock);
    }
}

fn some_text(s: &str) -> Option<String> {
    let t = s.trim();
    (!t.is_empty()).then(|| t.to_owned())
}

fn f64_of(v: Option<f32>) -> Option<f64> {
    v.map(f64::from).filter(|x| x.is_finite())
}

fn conditions(c: &iracing_core::Conditions) -> Option<wire::Conditions> {
    wire::conditions(
        f64_of(c.ambient_c),
        f64_of(c.track_c),
        f64_of(c.rain),
        f64_of(c.wind_speed_ms),
    )
}

/// The `session_start` message of a session.
pub fn session_start(info: &SessionOpenInfo) -> wire::SessionStart {
    let m = &info.meta;
    wire::SessionStart {
        started_at: info.started_at.clone(),
        session_type: m.kind.as_str().to_owned(),
        car_id: m.player.as_ref().and_then(|p| some_text(&p.car_id)),
        track_id: some_text(&m.track.track_id),
        layout_id: some_text(&m.track.layout_id),
        external_ref: (m.sub_session_id > 0)
            .then(|| format!("iracing:{}:{}", m.sub_session_id, info.key.session_num)),
        conditions: conditions(&info.conditions),
    }
}

/// The channel columns of a lap in catalogue order; an optional channel the lap did not have
/// on every row is left out.
pub fn channels(cols: Columns, present: Present) -> Option<wire::LapChannels> {
    if cols.t.is_empty() {
        return None;
    }
    let mut out: Vec<(Cow<'static, str>, Vec<f32>)> = Vec::with_capacity(20);
    let mut add = |name: &'static str, keep: bool, col: Vec<f32>| {
        if keep {
            out.push((Cow::Borrowed(name), col));
        }
    };
    add("t", true, cols.t);
    add("dist", true, cols.dist);
    add("speed", true, cols.speed);
    add("throttle", true, cols.throttle);
    add("brake", true, cols.brake);
    add("clutch", present.clutch, cols.clutch);
    add("steer", true, cols.steer);
    add("gear", true, cols.gear);
    add("rpm", true, cols.rpm);
    add("g_lat", true, cols.g_lat);
    add("g_long", true, cols.g_long);
    add("g_vert", present.g_vert, cols.g_vert);
    add("fuel", true, cols.fuel);
    add("abs", present.abs, cols.abs);
    add("invalid", true, cols.invalid);
    add("pit_mode", true, cols.pit_mode);
    add("ext_surface", present.surface, cols.ext_surface);
    add("ext_incidents", present.incidents, cols.ext_incidents);
    add("x", present.position, cols.x);
    add("z", present.position, cols.z);
    Some(wire::LapChannels(out))
}

/// `value` rounded to `places` decimals, or none when it is not a number within `min..=max`
/// (a reading outside the schema's range is left out, never clamped).
fn reading(value: f32, places: i32, min: f64, max: f64) -> Option<f64> {
    let scale = 10f64.powi(places);
    let v = (f64::from(value) * scale).round() / scale;
    (v.is_finite() && (min..=max).contains(&v)).then_some(v)
}

/// The `data` of a `repair` record: the fields the simulator published. Described by
/// [`iracing_core::sim_data::kinds`].
pub fn repair_data(e: &RepairEvent) -> Map<String, Value> {
    let mut data = Map::new();
    if let Some(v) = e.tow_time_s.and_then(|v| reading(v, 1, 0.0, 100_000.0)) {
        data.insert("tow_time_s".to_owned(), json!(v));
    }
    if let Some(v) = e.fast_repairs_available {
        data.insert("fast_repairs_available".to_owned(), json!(v));
    }
    if let Some(v) = e.fast_repairs_used {
        data.insert("fast_repairs_used".to_owned(), json!(v));
    }
    data
}

/// The `data` of a `stall_tyres` record for a new set of readings. The simulator's left and right
/// readings are the tyre's own, so on a left wheel left is the car's outside; `in` is the edge
/// towards the car's centreline. iRacing gives no surface temperature or hot pressure at a stop.
/// Fields are described by [`iracing_core::sim_data::kinds`]. None when no wheel has a value.
///
/// A `car_entry` carries only its occasion: the simulator's readings at an entry repeat the last
/// set that came into the pits (or its defaults), not the tyres on the car, so it marks the start
/// of a stint and nothing more.
pub fn stall_tyres_data(t: &StallTyres) -> Option<Map<String, Value>> {
    if t.occasion == StallOccasion::CarEntry {
        let mut data = Map::new();
        data.insert("occasion".to_owned(), json!(t.occasion.as_str()));
        return Some(data);
    }
    let wheel = |i: usize| {
        let [l, m, r] = t.temp_c[i];
        let [wl, wm, wr] = t.wear[i];
        let left_wheel = i.is_multiple_of(2);
        let (c_in, c_out) = if left_wheel { (r, l) } else { (l, r) };
        let (w_in, w_out) = if left_wheel { (wr, wl) } else { (wl, wr) };
        let c = |v: f32| reading(v, 2, -50.0, 500.0);
        let w = |v: f32| reading(v, 4, 0.0, 1.0);
        let mut wheel = Map::new();
        // A value out of range is left out; a wheel with none left is left out too.
        for (key, v) in [
            ("carcass_in_c", c(c_in)),
            ("carcass_mid_c", c(m)),
            ("carcass_out_c", c(c_out)),
            ("tread_in", w(w_in)),
            ("tread_mid", w(wm)),
            ("tread_out", w(w_out)),
        ] {
            if let Some(v) = v {
                wheel.insert(key.to_owned(), json!(v));
            }
        }
        (!wheel.is_empty()).then_some(Value::Object(wheel))
    };
    let mut wheels = Map::new();
    for (key, i) in [("fl", 0), ("fr", 1), ("rl", 2), ("rr", 3)] {
        if let Some(w) = wheel(i) {
            wheels.insert(key.to_owned(), w);
        }
    }
    if wheels.is_empty() {
        return None;
    }
    let mut data = Map::new();
    data.insert("occasion".to_owned(), json!(t.occasion.as_str()));
    data.insert("wheels".to_owned(), Value::Object(wheels));
    Some(data)
}

/// The `lap` message of a finished lap. Consumes the lap: its columns are moved.
pub fn lap_message(f: FinishedLap) -> wire::Lap {
    let timed = f.time_s.filter(|t| *t > 0.0 && t.is_finite());
    let (cols, present) = f.buffer.into_parts();
    wire::Lap {
        lap_type: f.lap_type.as_str().to_owned(),
        on_track: Some(f.on_track),
        time_s: timed.map(f64::from),
        valid: f.valid,
        time_source: timed.and(f.time_source).map(|s| s.as_str().to_owned()),
        measured_time_s: f64_of(f.measured_time_s.filter(|t| *t > 0.0)),
        started_at: None,
        sim_lap: u32::try_from(f.sim_lap).ok(),
        fuel_start_l: f64_of(f.fuel_start_l.filter(|v| *v >= 0.0)),
        fuel_used_l: f64_of(Some(f.fuel_used_l)),
        max_speed_kmh: f64_of(Some(f.max_speed_kmh).filter(|v| *v >= 0.0)),
        conditions: conditions(&f.conditions),
        sectors: f
            .sectors
            .as_slice()
            .iter()
            .map(|s| wire::Sector {
                time_s: f64_of(s.filter(|v| *v > 0.0)),
            })
            .collect(),
        channels: channels(cols, present),
    }
}

fn participant(p: ParticipantLap) -> wire::ParticipantLap {
    wire::ParticipantLap {
        participant: u32::from(p.participant),
        lap: u32::try_from(p.lap).unwrap_or(0),
        name: p.name,
        car_id: some_text(&p.car_id),
        class_id: some_text(&p.class_id),
        is_player: p.is_player,
        time_s: f64_of(p.time_s),
        time_source: p.time_s.and(p.time_source).map(|s| s.as_str().to_owned()),
        sectors: p.sectors.into_iter().map(f64_of).collect(),
        valid: p.valid,
        pit: p.pit,
        pit_stops: Some(p.pit_stops),
        from_line: p.from_line,
        position: p.position.and_then(|v| u32::try_from(v).ok()),
        crossed_at_s: p.crossed_at_s,
        sim_best_lap_s: f64_of(p.sim_best_lap_s),
        state: p.state,
    }
}

impl CoreSink for TrackerSink {
    fn start_session(&mut self, info: &SessionOpenInfo) -> u64 {
        let identity = identity_of(&info.meta, info.key.unique_id, info.key.session_num);
        let local = self.write.start_session(SessionOpen {
            identity,
            catalog: Some(session_catalog(&info.meta)),
            start: session_start(info),
        });
        self.open_positions(local, info);
        local
    }

    fn end_session(&mut self, local: u64, ended_at: &str) {
        if let Some(path) = self.positions.remove(&local)
            && !self.keep_positions_open
        {
            positions::append(&path, &encode_closed(positions::unix_now()));
        }
        self.write.end_session(local, ended_at);
    }

    fn lap(&mut self, local: u64, mut lap: FinishedLap) {
        let Some(path) = self.positions.get(&local).cloned() else {
            self.write.lap(local, Box::new(move || lap_message(lap)));
            return;
        };
        // The address of the lap's samples moves out of the buffer here; the writer thread
        // appends it to the positions file once the lap is stored, with the lap's number.
        let address = lap.buffer.take_address();
        self.write.lap_with_address(
            local,
            Box::new(move || lap_message(lap)),
            Box::new(move |_key, lap_seq| {
                if address.ticks.is_empty() {
                    return;
                }
                let record = LapRecord {
                    lap_seq,
                    start_crossing: address.start_crossing,
                    end_crossing: address.end_crossing,
                    ticks: address.ticks,
                    pct: address.pct,
                };
                positions::append(&path, &encode_lap(&record));
            }),
        );
    }

    fn setup(
        &mut self,
        local: u64,
        reason: SetupReason,
        at: &str,
        meta: Arc<SessionMeta>,
        fuel_l: f32,
    ) {
        let Some(snapshot) = meta.setup.clone() else {
            return;
        };
        let reason_name = reason.as_str();
        let fuel = (fuel_l > 0.0).then(|| f64::from(fuel_l));
        let when = at.to_owned();
        self.write.setup(
            local,
            at,
            reason_name,
            Box::new(move || snapshot.to_setup(reason_name, &when, fuel)),
        );
    }

    fn pit_exit(&mut self, local: u64, at: &str, fuel_l: Option<f64>) {
        self.write.pit_exit(local, at, fuel_l);
    }

    fn repair(&mut self, local: u64, at: &str, event: &RepairEvent) {
        self.write.sim_data(local, REPAIR, at, repair_data(event));
    }

    fn stall_tyres(&mut self, local: u64, at: &str, tyres: &StallTyres) {
        if let Some(data) = stall_tyres_data(tyres) {
            self.write.sim_data(local, STALL_TYRES, at, data);
        }
    }

    fn participants(&mut self, local: u64, rows: Vec<ParticipantLap>) {
        let rows = rows.into_iter().map(participant).collect();
        self.write.participants(local, rows);
    }

    fn catalog(&mut self, meta: Arc<SessionMeta>) {
        self.write.catalog(cars_only(&meta));
    }
}
