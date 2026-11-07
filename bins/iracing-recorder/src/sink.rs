//! The adapter from the core's [`Sink`] to the runtime's [`WriteSink`].
//!
//! Lap and setup messages are built on the writer thread: the closures handed to the
//! `WriteSink` own the lap's channel columns and the setup snapshot.

use std::borrow::Cow;
use std::sync::Arc;

use iracing_core::catalog::{cars_only, session_catalog};
use iracing_core::io::{
    FinishedLap, ParticipantLap, SessionOpenInfo, SetupReason, Sink as CoreSink,
};
use iracing_core::lap::{Columns, Present};
use iracing_core::meta::SessionMeta;
use iracing_core::resume::{IrClock, identity_of};
use stintmate_connector_link::sink::{SessionOpen, WriteSink};
use stintmate_connector_wire::messages as wire;

/// Receives the session clock before a session may open (for resuming it after a restart).
pub trait NoteClock {
    /// The clock of the row being processed, while no session is open.
    fn note_clock(&mut self, _clock: IrClock) {}
}

/// The recorder's sink: the core's records to the Pitwall write API through the outbox.
pub struct TrackerSink {
    /// The runtime's sink (status, resume and closing go through it).
    pub write: WriteSink<IrClock>,
}

impl TrackerSink {
    /// Wraps the runtime's sink.
    pub fn new(write: WriteSink<IrClock>) -> TrackerSink {
        TrackerSink { write }
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
        self.write.start_session(SessionOpen {
            identity,
            catalog: Some(session_catalog(&info.meta)),
            start: session_start(info),
        })
    }

    fn end_session(&mut self, local: u64, ended_at: &str) {
        self.write.end_session(local, ended_at);
    }

    fn lap(&mut self, local: u64, lap: FinishedLap) {
        self.write.lap(local, Box::new(move || lap_message(lap)));
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

    fn participants(&mut self, local: u64, rows: Vec<ParticipantLap>) {
        let rows = rows.into_iter().map(participant).collect();
        self.write.participants(local, rows);
    }

    fn catalog(&mut self, meta: Arc<SessionMeta>) {
        self.write.catalog(cars_only(&meta));
    }
}
