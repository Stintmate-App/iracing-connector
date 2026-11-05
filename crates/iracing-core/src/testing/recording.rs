//! A [`Sink`] that writes every call down as JSON, and a fixed [`Clock`], for the golden tests.

use std::sync::Arc;

use serde_json::{Value, json};

use crate::catalog;
use crate::io::{
    Clock, Conditions, FinishedLap, ParticipantLap, SessionOpenInfo, SetupReason, Sink, StallTyres,
};
use crate::lap::Columns;
use crate::meta::SessionMeta;

/// A clock whose wall time is always the same; its monotonic time moves only when told to
/// ([`FixedClock::advance`]).
#[derive(Clone, Debug)]
pub struct FixedClock {
    iso: String,
    mono: std::cell::Cell<f64>,
}

impl FixedClock {
    /// Moves the monotonic clock on by `s` seconds.
    pub fn advance(&self, s: f64) {
        self.mono.set(self.mono.get() + s);
    }
}

impl Default for FixedClock {
    fn default() -> FixedClock {
        FixedClock {
            iso: "2026-01-01T00:00:00Z".to_owned(),
            mono: std::cell::Cell::new(0.0),
        }
    }
}

impl Clock for FixedClock {
    fn iso(&self) -> String {
        self.iso.clone()
    }
    fn mono_s(&self) -> f64 {
        self.mono.get()
    }
}

/// Rounds to three decimals so the files do not depend on the last bits of a float.
fn r3(x: f64) -> Value {
    json!((x * 1000.0).round() / 1000.0)
}

fn opt(x: Option<f32>) -> Value {
    x.map_or(Value::Null, |v| r3(f64::from(v)))
}

fn conditions(c: &Conditions) -> Value {
    json!({
        "ambient_c": opt(c.ambient_c),
        "track_c": opt(c.track_c),
        "rain": opt(c.rain),
        "wind_speed_ms": opt(c.wind_speed_ms),
    })
}

/// Name, length, first and last value and a checksum of every column.
fn channels(c: &Columns) -> Value {
    let all: [(&str, &Vec<f32>); 18] = [
        ("t", &c.t),
        ("dist", &c.dist),
        ("speed", &c.speed),
        ("throttle", &c.throttle),
        ("brake", &c.brake),
        ("clutch", &c.clutch),
        ("steer", &c.steer),
        ("gear", &c.gear),
        ("rpm", &c.rpm),
        ("g_lat", &c.g_lat),
        ("g_long", &c.g_long),
        ("g_vert", &c.g_vert),
        ("fuel", &c.fuel),
        ("abs", &c.abs),
        ("invalid", &c.invalid),
        ("pit_mode", &c.pit_mode),
        ("ext_surface", &c.ext_surface),
        ("ext_incidents", &c.ext_incidents),
    ];
    let mut out = serde_json::Map::new();
    for (name, v) in all {
        let sum: f64 = v.iter().map(|x| f64::from(*x)).sum();
        out.insert(
            name.to_owned(),
            json!({
                "len": v.len(),
                "first": v.first().map_or(Value::Null, |x| r3(f64::from(*x))),
                "last": v.last().map_or(Value::Null, |x| r3(f64::from(*x))),
                "sum": r3(sum),
            }),
        );
    }
    Value::Object(out)
}

/// Records every call as a JSON value, in order.
#[derive(Clone, Debug, Default)]
pub struct RecordingSink {
    events: Vec<Value>,
    next_local: u64,
    laps: Vec<FinishedLap>,
}

impl RecordingSink {
    /// An empty sink.
    pub fn new() -> RecordingSink {
        RecordingSink::default()
    }

    /// The calls so far, as one JSON array.
    pub fn to_json(&self) -> Value {
        Value::Array(self.events.clone())
    }

    /// The names of the calls so far, in order.
    pub fn kinds(&self) -> Vec<&str> {
        self.events
            .iter()
            .filter_map(|e| e["call"].as_str())
            .collect()
    }

    /// The finished laps received, with their columns.
    pub fn laps(&self) -> &[FinishedLap] {
        &self.laps
    }

    /// The recorded calls of one kind.
    pub fn calls(&self, kind: &str) -> Vec<&Value> {
        self.events.iter().filter(|e| e["call"] == kind).collect()
    }
}

impl Sink for RecordingSink {
    fn start_session(&mut self, open: &SessionOpenInfo) -> u64 {
        self.next_local += 1;
        let cat = catalog::session_catalog(&open.meta);
        self.events.push(json!({
            "call": "start_session",
            "local": self.next_local,
            "unique_id": open.key.unique_id,
            "session_num": open.key.session_num,
            "kind": open.meta.kind.as_str(),
            "track": open.meta.track.track_id,
            "layout": open.meta.track.layout_id,
            "car": open.meta.player.as_ref().map(|p| p.car_id.clone()),
            "catalog": serde_json::to_value(&cat).unwrap_or(Value::Null),
            "player_name": open.meta.player.as_ref().map(|p| p.name.clone()),
            "roster": open.meta.roster.iter().flatten().filter(|d| d.is_real).map(|d| d.name.clone()).collect::<Vec<_>>(),
            "parse_errors": open.meta.parse_errors.len(),
            "conditions": conditions(&open.conditions),
            "started_at": open.started_at,
        }));
        self.next_local
    }

    fn end_session(&mut self, local: u64, ended_at: &str) {
        self.events
            .push(json!({"call": "end_session", "local": local, "ended_at": ended_at}));
    }

    fn lap(&mut self, local: u64, lap: FinishedLap) {
        self.events.push(json!({
            "call": "lap",
            "local": local,
            "sim_lap": lap.sim_lap,
            "type": lap.lap_type.as_str(),
            "partial": lap.partial,
            "time_s": opt(lap.time_s),
            "time_source": lap.time_source.map(|s| s.as_str()),
            "measured_time_s": opt(lap.measured_time_s),
            "valid": lap.valid,
            "cause": lap.cause.map(|c| c.as_str()),
            "cause_certain": lap.cause_certain,
            "invalid_at_m": opt(lap.invalid_at_m),
            "invalid_at_s": opt(lap.invalid_at_s),
            "on_track": lap.on_track,
            "touched_pit": lap.touched_pit,
            "started_at_s": r3(lap.started_at_s),
            "ended_at_s": r3(lap.ended_at_s),
            "fuel_start_l": opt(lap.fuel_start_l),
            "fuel_used_l": r3(f64::from(lap.fuel_used_l)),
            "max_speed_kmh": r3(f64::from(lap.max_speed_kmh)),
            "sectors": lap.sectors.as_slice().iter().map(|s| opt(*s)).collect::<Vec<_>>(),
            "conditions": conditions(&lap.conditions),
            "channels": channels(lap.buffer.columns()),
        }));
        self.laps.push(lap);
    }

    fn setup(
        &mut self,
        local: u64,
        reason: SetupReason,
        at: &str,
        meta: Arc<SessionMeta>,
        fuel_l: f32,
    ) {
        self.events.push(json!({
            "call": "setup",
            "local": local,
            "reason": reason.as_str(),
            "at": at,
            "fuel_l": r3(f64::from(fuel_l)),
            "session_num": meta.session_num,
            "setup_update": meta.setup_update,
        }));
    }

    fn pit_exit(&mut self, local: u64, at: &str, fuel_l: Option<f64>) {
        self.events.push(json!({
            "call": "pit_exit",
            "local": local,
            "at": at,
            "fuel_l": fuel_l.map(r3),
        }));
    }

    fn participants(&mut self, local: u64, rows: Vec<ParticipantLap>) {
        let rows: Vec<Value> = rows
            .iter()
            .map(|r| {
                json!({
                    "participant": r.participant, "lap": r.lap, "name": r.name,
                    "is_player": r.is_player, "time_s": opt(r.time_s),
                    "time_source": r.time_source.map(|s| s.as_str()),
                    "sectors": r.sectors.iter().map(|s| opt(*s)).collect::<Vec<_>>(),
                    "car_id": r.car_id, "class_id": r.class_id,
                    "from_line": r.from_line, "crossed_at_s": r.crossed_at_s.map(r3),
                    "sim_best_lap_s": opt(r.sim_best_lap_s), "state": r.state,
                    "valid": r.valid, "pit": r.pit, "pit_stops": r.pit_stops,
                    "position": r.position,
                })
            })
            .collect();
        self.events
            .push(json!({"call": "participants", "local": local, "rows": rows}));
    }

    fn catalog(&mut self, meta: Arc<SessionMeta>) {
        let cat = catalog::cars_only(&meta);
        self.events.push(json!({
            "call": "catalog",
            "catalog": serde_json::to_value(&cat).unwrap_or(Value::Null),
        }));
    }

    fn stall_tyres(&mut self, local: u64, tyres: &StallTyres) {
        self.events.push(json!({
            "call": "stall_tyres",
            "local": local,
            "at_s": r3(tyres.at_s),
            "temp_c": tyres.temp_c.iter().map(|w| w.iter().map(|v| r3(f64::from(*v))).collect::<Vec<_>>()).collect::<Vec<_>>(),
            "wear": tyres.wear.iter().map(|w| w.iter().map(|v| r3(f64::from(*v))).collect::<Vec<_>>()).collect::<Vec<_>>(),
        }));
    }
}
