//! The tracker: turns rows into sessions, laps, pit visits and the live state of the lap.
//!
//! One [`Tracker::step`] per row. While no lap ends and no session changes it allocates nothing.
//! Laps leave through the [`Sink`]; the tracker reads no clock of its own (time is the row's
//! `SessionTime`, wall time comes from the [`Clock`]).
//!
//! What the simulator was seen to do, and what the tracker does about it:
//!
//! - Both lap counters drop to 0 when the player leaves the car and `Lap` goes to 1 again on
//!   re-entry in the stall without a crossing. The counters are taken again whenever driving
//!   starts and when a counter falls; neither is a lap end.
//! - A reset to the pits shows the car in the stall in one row (`OnPitRoad` and
//!   `PlayerCarInPitStall` rise together) with the lap fraction jumping to the stall's. That is
//!   tested before the wrap of the lap fraction, which it would otherwise pass.
//! - `LapLastLapTime` of -1 means the lap was not timed.
//! - Out of the car, with a session loaded, is the garage, also before the first drive; the
//!   live replay the simulator shows then is not a menu. The garage screen is `IsInGarage` or
//!   `IsGarageVisible`.
//! - `FuelLevel` reads 0 out of the car: fuel is read only while driving.
//! - The live tyre readings change only in the row the car enters its stall.

use std::sync::Arc;

use crate::codes::{self, SessionKind};
use crate::io::{
    Clock, Conditions, FinishedLap, LapType, SessionKey, SessionOpenInfo, SetupReason, Sink,
    StallTyres, TimeSource,
};
use crate::lap::{LapBuffer, LapPoint};
use crate::meta::SessionMeta;
use crate::sectors::{SectorClock, SectorTimes};
use crate::tick::Tick;
use crate::validity::{IncidentTracker, InvalidCause, Validity};

/// Seconds the recording predicate may be false before the lap closes.
pub const GATE_GRACE_S: f64 = 0.5;
/// Seconds after a lap started at the line in which a second crossing signal only re-syncs the
/// simulator's lap number.
pub const SYNC_WINDOW_S: f64 = 1.0;
/// Seconds to wait for the simulator's lap time after a crossing.
pub const PENDING_TIMEOUT_S: f64 = 3.0;
/// Partial laps shorter than this are dropped.
pub const MIN_PARTIAL_LAP_S: f64 = 5.0;
/// Laps whose top speed stays under this are dropped (km/h).
pub const MOVING_KMH: f32 = 3.0;
/// Seconds without rows after which the session ends.
pub const OFF_GRACE_S: f64 = 5.0;
/// The smallest jump of the lap fraction that is a reset, not driving.
pub const JUMP_PCT_MIN: f64 = 0.02;

const TRK_LOC_IN_PIT_STALL: i32 = 1;
const SESSION_STATE_PARADE: i32 = 3;

/// What the recorder is doing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    /// The simulator is gone or not connected.
    Off,
    /// Connected, but no session can open (no description yet, a spectator, a real replay).
    Idle,
    /// A session is open and the car is not driving (garage screen, out of the car, replay).
    Garage,
    /// Driving on the pit road.
    Pits,
    /// Driving off the pit road.
    Track,
}

/// The live state of the lap, for the live frame, the telemetry and the status.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LapView {
    /// The recorder state.
    pub state: State,
    /// The kind of the open session.
    pub session_kind: SessionKind,
    /// The kind of the lap in progress so far.
    pub lap_type: Option<LapType>,
    /// Whether the lap in progress is timed (a formation lap is not).
    pub timed: bool,
    /// Seconds since the lap started.
    pub lap_clock_s: f32,
    /// Whether the lap is still valid.
    pub valid: bool,
    /// Why it is not.
    pub cause: Option<InvalidCause>,
    /// Whether the cause is certain.
    pub cause_certain: bool,
    /// Metres along the lap where it became invalid.
    pub invalid_at_m: Option<f32>,
    /// The index of the last sector start passed.
    pub sector: Option<u8>,
    /// Laps sent in this session.
    pub laps_recorded: u32,
    /// The best valid lap time of the session.
    pub best_s: Option<f32>,
    /// The last lap time of the session.
    pub last_s: Option<f32>,
    /// The pit mode code of the last row (0 on track, 1 into the pits, 2 in the box, 3 out of
    /// the pits, 4 garage).
    pub pit_mode: u8,
}

impl LapView {
    const IDLE: LapView = LapView {
        state: State::Off,
        session_kind: SessionKind::Other,
        lap_type: None,
        timed: false,
        lap_clock_s: 0.0,
        valid: true,
        cause: None,
        cause_certain: false,
        invalid_at_m: None,
        sector: None,
        laps_recorded: 0,
        best_s: None,
        last_s: None,
        pit_mode: 4,
    };
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StartKind {
    Line,
    PitStall,
    Garage,
    Resume,
}

/// What the previous row showed.
#[derive(Clone, Copy, Debug)]
struct Prev {
    time: f64,
    lap: i32,
    completed: i32,
    pct: f32,
    on_pit_road: bool,
    in_stall: bool,
    service: bool,
    tow: bool,
}

struct Open {
    key: SessionKey,
    local: u64,
    meta: Arc<SessionMeta>,
    length_m: f32,
}

struct LapState {
    active: bool,
    started_at_s: f64,
    start_kind: StartKind,
    start_on_pit_road: bool,
    touched_pit: bool,
    was_on_track: bool,
    max_speed_ms: f32,
    fuel_start: Option<f32>,
    fuel_used: f32,
    formation: bool,
    sim_lap: i32,
    start_row_s: f64,
}

impl LapState {
    const NONE: LapState = LapState {
        active: false,
        started_at_s: 0.0,
        start_kind: StartKind::Resume,
        start_on_pit_road: false,
        touched_pit: false,
        was_on_track: false,
        max_speed_ms: 0.0,
        fuel_start: None,
        fuel_used: 0.0,
        formation: false,
        sim_lap: 0,
        start_row_s: f64::NEG_INFINITY,
    };

    fn lap_type(&self, ended_on_pit_road: bool, garage_return: bool) -> LapType {
        if self.formation {
            LapType::Formation
        } else if matches!(self.start_kind, StartKind::PitStall | StartKind::Garage)
            || self.start_on_pit_road
        {
            LapType::Out
        } else if ended_on_pit_road || (garage_return && self.was_on_track) {
            LapType::In
        } else {
            LapType::Flying
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct PitVisit {
    was_in_stall: bool,
    tyres_taken: bool,
}

/// A lap that crossed the line and waits for the simulator's time.
struct Pending {
    lap: FinishedLap,
    sectors: SectorClock,
    validity: Validity,
    baseline: f32,
    crossed_at: f64,
    from_line: bool,
}

/// The tracker. See the module documentation.
pub struct Tracker {
    state: State,
    open: Option<Open>,
    lap: LapState,
    validity: Validity,
    incidents: IncidentTracker,
    sectors: SectorClock,
    buffer: LapBuffer,
    spare: Option<LapBuffer>,
    pool_rows: usize,
    pending: Option<Pending>,
    visit: PitVisit,
    prev: Option<Prev>,
    gap_since: Option<f64>,
    off_since: Option<f64>,
    last_seen_last: f32,
    last_fuel: f32,
    conditions: Conditions,
    stall_tyres: Option<StallTyres>,
    laps_recorded: u32,
    best_s: Option<f32>,
    last_s: Option<f32>,
    pit_mode: u8,
    view: LapView,
}

impl Tracker {
    /// A tracker whose lap buffers reserve `pool_rows` rows each (two are made now).
    pub fn new(pool_rows: usize) -> Tracker {
        Tracker {
            state: State::Off,
            open: None,
            lap: LapState::NONE,
            validity: Validity::new(),
            incidents: IncidentTracker::new(),
            sectors: SectorClock::new(&[]),
            buffer: LapBuffer::with_capacity(pool_rows),
            spare: Some(LapBuffer::with_capacity(pool_rows)),
            pool_rows,
            pending: None,
            visit: PitVisit {
                was_in_stall: false,
                tyres_taken: false,
            },
            prev: None,
            gap_since: None,
            off_since: None,
            last_seen_last: -1.0,
            last_fuel: 0.0,
            conditions: Conditions::default(),
            stall_tyres: None,
            laps_recorded: 0,
            best_s: None,
            last_s: None,
            pit_mode: 4,
            view: LapView::IDLE,
        }
    }

    /// The live state of the lap.
    pub fn view(&self) -> LapView {
        self.view
    }

    /// The recorder state.
    pub fn state(&self) -> State {
        self.state
    }

    /// The local id of the open session.
    pub fn session_local(&self) -> Option<u64> {
        self.open.as_ref().map(|o| o.local)
    }

    /// The tyre readings of the last stall entry of the open session.
    pub fn stall_tyres(&self) -> Option<&StallTyres> {
        self.stall_tyres.as_ref()
    }

    /// The game went away: the open lap closes, the session ends.
    pub fn on_off(&mut self, sink: &mut dyn Sink, clock: &dyn Clock) {
        self.close_session(true, sink, clock);
        self.state = State::Off;
        self.prev = None;
        self.off_since = None;
        self.view = LapView::IDLE;
    }

    /// The recorder stops. With `keep_open` the session is not ended (it may resume); its open
    /// lap is still closed and sent.
    pub fn stop(&mut self, keep_open: bool, sink: &mut dyn Sink, clock: &dyn Clock) {
        if keep_open {
            self.close_session(false, sink, clock);
            self.state = State::Off;
            self.prev = None;
            self.view = LapView::IDLE;
        } else {
            self.on_off(sink, clock);
        }
    }

    /// Feeds one new row. `meta` is the latest session description.
    ///
    /// Hot: allocates nothing while no lap ends and no session opens or closes.
    pub fn step(
        &mut self,
        t: &Tick,
        meta: Option<&Arc<SessionMeta>>,
        sink: &mut dyn Sink,
        clock: &dyn Clock,
    ) {
        if !t.connected {
            let since = *self
                .off_since
                .get_or_insert(self.prev.map_or(t.session_time, |p| p.time));
            self.state = State::Off;
            self.view.state = State::Off;
            if t.session_time - since >= OFF_GRACE_S {
                self.on_off(sink, clock);
            }
            return;
        }
        self.off_since = None;
        if let Some(p) = self.prev {
            let dt = t.session_time - p.time;
            if !(-1.0..=OFF_GRACE_S).contains(&dt) {
                self.on_off(sink, clock);
            }
        }

        let m = meta.filter(|m| m.session_num == t.session_num);
        let changed = self.open.as_ref().is_some_and(|open| {
            open.key.unique_id != t.session_unique_id
                || open.key.session_num != t.session_num
                || m.is_some_and(|m| m.identity_hash != open.key.meta_identity)
        });
        if changed {
            self.close_session(true, sink, clock);
        } else if let (Some(open), Some(m)) = (&mut self.open, m)
            && !Arc::ptr_eq(&open.meta, m)
        {
            open.meta = Arc::clone(m);
        }

        let driving = t.is_on_track && t.is_on_track_car && !t.is_replay;
        let real_replay = t.is_replay && t.replay_frame_end.unwrap_or(0) > 1;
        let before = self.state;
        if self.open.is_none()
            && let Some(m) = m
            && !(real_replay && !driving)
            && m.player.is_some()
            && !m.track.track_id.is_empty()
            && m.track.length_m.is_some()
        {
            self.open_session(t, m, driving, sink, clock);
        }

        self.conditions = Conditions::of(t);
        if self.open.is_none() {
            self.state = State::Idle;
            self.finish_row(t);
            return;
        }
        self.state = if !driving {
            State::Garage
        } else if t.on_pit_road {
            State::Pits
        } else {
            State::Track
        };
        let rise = self.incidents.step(t);
        self.poll_pending(t, sink);

        if driving {
            self.gap_since = None;
            self.drive(t, before, rise.as_ref(), sink, clock);
        } else {
            self.not_driving(t, sink);
            self.pit_mode = 4;
        }
        self.last_seen_last = t.lap_last_time;
        self.finish_row(t);
    }

    // ---- sessions -----------------------------------------------------------------------

    fn open_session(
        &mut self,
        t: &Tick,
        meta: &Arc<SessionMeta>,
        driving: bool,
        sink: &mut dyn Sink,
        clock: &dyn Clock,
    ) {
        let key = SessionKey {
            unique_id: t.session_unique_id,
            session_num: t.session_num,
            meta_identity: meta.identity_hash,
        };
        let length_m = meta.track.length_m.map_or(0.0, |l| l as f32);
        let at = clock.iso();
        let info = SessionOpenInfo {
            key,
            meta: Arc::clone(meta),
            conditions: Conditions::of(t),
            started_at: at.clone(),
        };
        let local = sink.start_session(&info);
        if driving {
            self.last_fuel = t.fuel_l;
        }
        sink.setup(
            local,
            SetupReason::SessionStart,
            &at,
            Arc::clone(meta),
            self.last_fuel,
        );
        self.sectors = SectorClock::new(&meta.sector_starts);
        self.buffer.clear();
        self.buffer.set_length(length_m);
        self.incidents.reset();
        self.validity.reset();
        self.lap = LapState::NONE;
        self.pending = None;
        self.stall_tyres = None;
        self.laps_recorded = 0;
        self.best_s = None;
        self.last_s = None;
        self.gap_since = None;
        self.last_seen_last = t.lap_last_time;
        self.open = Some(Open {
            key,
            local,
            meta: Arc::clone(meta),
            length_m,
        });
    }

    /// Closes the open lap and sends everything pending. `end` sends `end_session`.
    fn close_session(&mut self, end: bool, sink: &mut dyn Sink, clock: &dyn Clock) {
        let Some(local) = self.open.as_ref().map(|o| o.local) else {
            return;
        };
        let end_s = self.prev.map_or(self.lap.started_at_s, |p| p.time);
        self.end_partial(end_s, false, true, sink);
        self.resolve_pending(self.last_seen_last, sink);
        if end {
            sink.end_session(local, &clock.iso());
        }
        self.open = None;
        self.lap.active = false;
        self.last_fuel = 0.0;
        self.laps_recorded = 0;
        self.best_s = None;
        self.last_s = None;
    }

    // ---- laps ---------------------------------------------------------------------------

    fn not_driving(&mut self, t: &Tick, sink: &mut dyn Sink) {
        if !self.lap.active {
            return;
        }
        let since = *self.gap_since.get_or_insert(t.session_time);
        let out_of_play =
            t.is_in_garage || t.is_garage_visible.unwrap_or(false) || t.is_replay || in_stall(t);
        if out_of_play || t.session_time - since > GATE_GRACE_S {
            let end_s = self.prev.map_or(t.session_time, |p| p.time);
            self.end_partial(end_s, false, true, sink);
            self.gap_since = None;
        }
    }

    fn drive(
        &mut self,
        t: &Tick,
        before: State,
        rise: Option<&crate::validity::IncidentRise>,
        sink: &mut dyn Sink,
        clock: &dyn Clock,
    ) {
        let now = t.session_time;
        let stall = in_stall(t);
        let service = t.pit_service_active.unwrap_or(false);
        let tow = t.tow_time.unwrap_or(0.0) > 0.0;
        let prev = self.prev;

        if !self.lap.active {
            if tow {
                return;
            }
            let kind = if stall {
                StartKind::PitStall
            } else if before == State::Garage {
                StartKind::Garage
            } else {
                StartKind::Resume
            };
            self.start_lap(t, kind, now);
            self.visit = PitVisit {
                was_in_stall: stall,
                tyres_taken: false,
            };
            if stall {
                self.take_tyres(t, sink);
            }
        } else if let Some(p) = prev {
            let length = self.open.as_ref().map_or(0.0, |o| o.length_m);
            let mut line = false;
            if tow && !p.tow {
                self.validity
                    .mark_tow(t.lap_dist, (now - self.lap.started_at_s) as f32);
                self.end_partial(now, false, false, sink);
                return;
            } else if t.on_pit_road && !p.on_pit_road && stall && !p.in_stall {
                // A reset to the pits: the car reappears in the stall.
                self.validity
                    .mark_tow(t.lap_dist, (now - self.lap.started_at_s) as f32);
                self.end_partial(now, false, false, sink);
                self.start_lap(t, StartKind::PitStall, now);
                self.visit = PitVisit {
                    was_in_stall: true,
                    tyres_taken: false,
                };
            } else {
                let fell = t.lap < p.lap || t.lap_completed < p.completed;
                let dt = now - p.time;
                let moved = {
                    let d = f64::from(t.lap_dist_pct - p.pct).rem_euclid(1.0);
                    d.min(1.0 - d)
                };
                let limit = if length > 0.0 {
                    JUMP_PCT_MIN.max(3.0 * f64::from(t.speed) * dt / f64::from(length))
                } else {
                    JUMP_PCT_MIN
                };
                if moved > limit {
                    self.validity
                        .mark_tow(t.lap_dist, (now - self.lap.started_at_s) as f32);
                    self.end_partial(now, false, false, sink);
                    self.start_lap(t, StartKind::Resume, now);
                    self.visit.was_in_stall = stall;
                } else if !fell {
                    let counted = t.lap > p.lap || t.lap_completed > p.completed;
                    let wrapped = p.pct > 0.9 && t.lap_dist_pct < 0.1;
                    if counted || wrapped {
                        if self.lap.start_kind == StartKind::Line
                            && now - self.lap.started_at_s < SYNC_WINDOW_S
                        {
                            self.lap.sim_lap = t.lap;
                        } else {
                            line = true;
                        }
                    }
                }
                if line {
                    let crossed = crossing_time(p, t);
                    self.end_at_line(t, crossed, sink);
                    self.start_lap(t, StartKind::Line, crossed);
                }
            }
            if t.on_pit_road && !p.on_pit_road {
                self.visit = PitVisit {
                    was_in_stall: stall,
                    tyres_taken: false,
                };
            }
            if !stall {
                self.visit.tyres_taken = false;
            }
            if (stall && !p.in_stall) || (service && !p.service) {
                self.take_tyres(t, sink);
            }
            if stall || matches!(t.pit_sv_status, Some(1 | 2)) {
                self.visit.was_in_stall = true;
            }
            if p.on_pit_road && !t.on_pit_road {
                self.pit_exit(t, sink, clock);
            }
        }

        self.sample(t, rise);
    }

    /// Records the row into the open lap.
    fn sample(&mut self, t: &Tick, rise: Option<&crate::validity::IncidentRise>) {
        let now = t.session_time;
        let clock_s = (now - self.lap.started_at_s) as f32;
        self.validity
            .step(t, self.state == State::Track, clock_s, rise);
        if let Some(p) = self.prev {
            self.sectors
                .step(f64::from(p.pct), f64::from(t.lap_dist_pct), p.time, now);
        }
        let stall = in_stall(t);
        let l = &mut self.lap;
        if t.fuel_l < self.last_fuel && !stall && self.last_fuel > 0.0 {
            l.fuel_used += self.last_fuel - t.fuel_l;
        }
        self.last_fuel = t.fuel_l;
        l.max_speed_ms = l.max_speed_ms.max(t.speed);
        l.was_on_track |= self.state == State::Track;
        l.touched_pit |= t.on_pit_road;
        // The row that starts a lap at the line still shows the state of the lap before.
        l.formation |= t.session_state == SESSION_STATE_PARADE && now > l.start_row_s;
        self.pit_mode = codes::pit_mode(
            false,
            t.in_pit_stall,
            t.track_loc,
            t.on_pit_road,
            self.visit.was_in_stall,
        );
        let point = LapPoint {
            timed: !l.formation,
            lap_clock_s: clock_s,
            valid: self.validity.valid,
        };
        self.buffer.push(t, &point, self.pit_mode);
    }

    fn start_lap(&mut self, t: &Tick, kind: StartKind, at_s: f64) {
        self.lap = LapState {
            active: true,
            started_at_s: at_s,
            start_kind: kind,
            start_on_pit_road: t.on_pit_road,
            touched_pit: t.on_pit_road,
            was_on_track: false,
            max_speed_ms: 0.0,
            fuel_start: (t.fuel_l > 0.0).then_some(t.fuel_l),
            fuel_used: 0.0,
            formation: false,
            sim_lap: t.lap,
            start_row_s: t.session_time,
        };
        self.last_fuel = t.fuel_l;
        self.validity.reset();
        self.sectors.start_lap(at_s, kind != StartKind::Line);
        self.buffer.clear();
    }

    fn take_buffer(&mut self) -> LapBuffer {
        let length = self.open.as_ref().map_or(0.0, |o| o.length_m);
        let mut fresh = self
            .spare
            .take()
            .unwrap_or_else(|| LapBuffer::with_capacity(self.pool_rows));
        fresh.set_length(length);
        self.spare = Some(LapBuffer::with_capacity(self.pool_rows));
        std::mem::replace(&mut self.buffer, fresh)
    }

    fn base_lap(&mut self, end_s: f64, lap_type: LapType, partial: bool) -> FinishedLap {
        let buffer = self.take_buffer();
        let l = &self.lap;
        FinishedLap {
            sim_lap: l.sim_lap,
            lap_type,
            partial,
            time_s: None,
            time_source: None,
            measured_time_s: None,
            valid: false,
            cause: None,
            cause_certain: false,
            invalid_at_m: None,
            invalid_at_s: None,
            on_track: l.was_on_track,
            touched_pit: l.touched_pit,
            started_at_s: l.started_at_s,
            ended_at_s: end_s,
            fuel_start_l: l.fuel_start,
            fuel_used_l: l.fuel_used,
            max_speed_kmh: l.max_speed_ms * 3.6,
            sectors: SectorTimes::NONE,
            conditions: self.conditions,
            buffer,
        }
    }

    /// A lap ended at the line at session time `crossed`. It waits for the simulator's time.
    fn end_at_line(&mut self, t: &Tick, crossed: f64, sink: &mut dyn Sink) {
        self.resolve_pending(t.lap_last_time, sink);
        let duration = crossed - self.lap.started_at_s;
        if self.lap.start_kind != StartKind::Line && duration < MIN_PARTIAL_LAP_S {
            self.buffer.clear();
            return;
        }
        if self.lap.touched_pit {
            self.validity.mark_pit_road();
        }
        let lap_type = self.lap.lap_type(t.on_pit_road, false);
        let from_line = self.lap.start_kind == StartKind::Line;
        let lap = self.base_lap(crossed, lap_type, false);
        self.pending = Some(Pending {
            lap,
            sectors: self.sectors.clone(),
            validity: self.validity.clone(),
            baseline: self.last_seen_last,
            crossed_at: crossed,
            from_line,
        });
    }

    /// The open lap ends before the line. Short and stationary laps are dropped.
    fn end_partial(
        &mut self,
        end_s: f64,
        ended_on_pit_road: bool,
        garage_return: bool,
        sink: &mut dyn Sink,
    ) {
        if !self.lap.active {
            return;
        }
        self.lap.active = false;
        let duration = end_s - self.lap.started_at_s;
        if duration < MIN_PARTIAL_LAP_S || self.lap.max_speed_ms * 3.6 < MOVING_KMH {
            self.buffer.clear();
            return;
        }
        self.resolve_pending(self.last_seen_last, sink);
        if self.validity.valid {
            self.validity.mark_not_timed();
        }
        if self.lap.touched_pit {
            self.validity.mark_pit_road();
        }
        let lap_type = self.lap.lap_type(ended_on_pit_road, garage_return);
        let mut lap = self.base_lap(end_s, lap_type, true);
        self.fill_validity(&mut lap, &self.validity.clone());
        if let Some(local) = self.open.as_ref().map(|o| o.local) {
            self.laps_recorded += 1;
            sink.lap(local, lap);
        }
    }

    fn fill_validity(&self, lap: &mut FinishedLap, v: &Validity) {
        lap.valid = v.valid && !lap.partial;
        lap.cause = v.cause;
        lap.cause_certain = v.certain;
        lap.invalid_at_m = v.at_m;
        lap.invalid_at_s = v.at_s;
    }

    /// Resolves the waiting lap when the simulator's time came or the wait is over.
    fn poll_pending(&mut self, t: &Tick, sink: &mut dyn Sink) {
        let Some(p) = &self.pending else {
            return;
        };
        let changed = t.lap_last_time != p.baseline;
        if changed || t.session_time - p.crossed_at >= PENDING_TIMEOUT_S {
            self.resolve_pending(t.lap_last_time, sink);
        }
    }

    /// Sends the waiting lap. `last` is the simulator's last lap time now; when it has not
    /// changed yet the measured time is taken.
    fn resolve_pending(&mut self, last: f32, sink: &mut dyn Sink) {
        let Some(mut p) = self.pending.take() else {
            return;
        };
        let changed = last != p.baseline;
        let measured = (p.crossed_at - p.lap.started_at_s) as f32;
        if p.from_line {
            p.lap.measured_time_s = Some(measured);
        }
        let mut official = None;
        if changed && last > 0.0 {
            p.lap.time_s = Some(last);
            p.lap.time_source = Some(TimeSource::Official);
            official = Some(last);
        } else {
            if changed {
                // -1 is the simulator's "not timed".
                p.validity.mark_not_timed();
            }
            if p.from_line {
                p.lap.time_s = Some(measured);
                p.lap.time_source = Some(TimeSource::Measured);
            }
        }
        p.lap.sectors = p.sectors.finish(p.crossed_at, official);
        self.fill_validity(&mut p.lap, &p.validity);
        if let Some(t) = p.lap.time_s
            && p.lap.valid
        {
            self.best_s = Some(self.best_s.map_or(t, |b| b.min(t)));
        }
        self.last_s = p.lap.time_s.or(self.last_s);
        self.laps_recorded += 1;
        if let Some(local) = self.open.as_ref().map(|o| o.local) {
            sink.lap(local, p.lap);
        }
    }

    // ---- pits ---------------------------------------------------------------------------

    fn take_tyres(&mut self, t: &Tick, sink: &mut dyn Sink) {
        if self.visit.tyres_taken {
            return;
        }
        self.visit.tyres_taken = true;
        if let (Some(temp), Some(wear)) = (t.tyre_temp, t.tyre_wear) {
            let tyres = StallTyres {
                at_s: t.session_time,
                temp_c: temp,
                wear,
            };
            self.stall_tyres = Some(tyres);
            if let Some(local) = self.open.as_ref().map(|o| o.local) {
                sink.stall_tyres(local, &tyres);
            }
        }
    }

    /// The car left the pit road: the waiting lap goes first, then the exit, then the setup.
    fn pit_exit(&mut self, t: &Tick, sink: &mut dyn Sink, clock: &dyn Clock) {
        self.resolve_pending(t.lap_last_time, sink);
        let Some(open) = &self.open else {
            return;
        };
        let at = clock.iso();
        sink.pit_exit(open.local, &at, Some(f64::from(t.fuel_l)));
        sink.setup(
            open.local,
            SetupReason::PitExit,
            &at,
            Arc::clone(&open.meta),
            t.fuel_l,
        );
    }

    // ---- the view -----------------------------------------------------------------------

    fn finish_row(&mut self, t: &Tick) {
        self.prev = Some(Prev {
            time: t.session_time,
            lap: t.lap,
            completed: t.lap_completed,
            pct: t.lap_dist_pct,
            on_pit_road: t.on_pit_road,
            in_stall: in_stall(t),
            service: t.pit_service_active.unwrap_or(false),
            tow: t.tow_time.unwrap_or(0.0) > 0.0,
        });
        let active = self.lap.active;
        self.view = LapView {
            state: self.state,
            session_kind: self
                .open
                .as_ref()
                .map_or(SessionKind::Other, |o| o.meta.kind),
            lap_type: active.then(|| self.lap.lap_type(t.on_pit_road, false)),
            timed: active && !self.lap.formation,
            lap_clock_s: if active {
                (t.session_time - self.lap.started_at_s) as f32
            } else {
                0.0
            },
            valid: self.validity.valid,
            cause: self.validity.cause,
            cause_certain: self.validity.certain,
            invalid_at_m: self.validity.at_m,
            sector: if active { self.sectors.current() } else { None },
            laps_recorded: self.laps_recorded,
            best_s: self.best_s,
            last_s: self.last_s,
            pit_mode: self.pit_mode,
        };
    }
}

fn in_stall(t: &Tick) -> bool {
    t.in_pit_stall
        .unwrap_or(t.track_loc == TRK_LOC_IN_PIT_STALL)
}

/// The session time the car crossed the line between the previous row and this one.
fn crossing_time(p: Prev, t: &Tick) -> f64 {
    let (a, b) = (f64::from(p.pct), f64::from(t.lap_dist_pct));
    if a > 0.5 && b < 0.5 {
        let total = (1.0 - a) + b;
        let frac = if total > 0.0 { (1.0 - a) / total } else { 1.0 };
        p.time + frac * (t.session_time - p.time)
    } else {
        t.session_time
    }
}

#[cfg(all(test, feature = "testing"))]
mod tests {
    use super::*;
    use crate::meta;
    use crate::testing::{FixedClock, RecordingSink, YamlBuilder};
    use irsdk::worker::SessionDoc;

    fn meta() -> Arc<SessionMeta> {
        let text = YamlBuilder::new().build_cp1252();
        Arc::new(meta::build(&SessionDoc::from_raw(1, &text), 0))
    }

    fn driving(time: f64, pct: f32) -> Box<Tick> {
        let mut t = Tick::new();
        t.connected = true;
        t.fresh = true;
        t.session_time = time;
        t.session_unique_id = 1;
        t.is_on_track = true;
        t.is_on_track_car = true;
        t.lap = 3;
        t.lap_completed = 2;
        t.lap_dist_pct = pct;
        t.lap_dist = pct * 4010.0;
        t.speed = 50.0;
        t.fuel_l = 30.0;
        t.track_loc = 3;
        t
    }

    #[test]
    fn tracker_out_of_the_car_with_a_session_is_the_garage() {
        let (meta, mut sink, clock) = (meta(), RecordingSink::new(), FixedClock::default());
        let mut tracker = Tracker::new(64);
        let mut t = driving(1.0, 0.5);
        t.is_on_track = false;
        t.is_on_track_car = false;
        t.is_replay = true;
        t.replay_frame_end = Some(1);
        t.fuel_l = 0.0;
        tracker.step(&t, Some(&meta), &mut sink, &clock);
        assert_eq!(tracker.state(), State::Garage);
        assert!(tracker.session_local().is_some());
        // A real replay with no session open is not the garage.
        let mut other = Tracker::new(64);
        t.replay_frame_end = Some(5000);
        other.step(&t, Some(&meta), &mut sink, &clock);
        assert_eq!(other.state(), State::Idle);
    }

    #[test]
    fn tracker_reset_into_the_stall_from_late_in_the_lap_is_not_a_crossing() {
        let (meta, mut sink, clock) = (meta(), RecordingSink::new(), FixedClock::default());
        let mut tracker = Tracker::new(64);
        let mut time = 0.0;
        let mut pct = 0.5;
        for _ in 0..600 {
            time += 1.0 / 60.0;
            pct += 0.0002;
            tracker.step(&driving(time, pct), Some(&meta), &mut sink, &clock);
        }
        assert!(pct > 0.6 && sink.laps().is_empty());
        // Late in the lap: the car reappears in the stall past the line, in one row.
        let mut t = driving(time + 0.1, 0.0313);
        t.on_pit_road = true;
        t.in_pit_stall = Some(true);
        t.track_loc = 1;
        tracker.step(&t, Some(&meta), &mut sink, &clock);
        assert_eq!(sink.laps().len(), 1);
        let lap = &sink.laps()[0];
        assert!(lap.partial && !lap.valid && lap.time_s.is_none());
        assert_eq!(lap.cause, Some(InvalidCause::Unknown));
    }

    #[test]
    fn tracker_counters_that_fall_are_not_a_lap() {
        let (meta, mut sink, clock) = (meta(), RecordingSink::new(), FixedClock::default());
        let mut tracker = Tracker::new(64);
        let mut time = 0.0;
        for i in 0..600 {
            time += 1.0 / 60.0;
            let mut t = driving(time, 0.3 + 0.0002 * i as f32);
            if i >= 300 {
                t.lap = 0;
                t.lap_completed = 0;
            }
            tracker.step(&t, Some(&meta), &mut sink, &clock);
        }
        assert!(sink.laps().is_empty());
        assert_eq!(tracker.view().state, State::Track);
    }
}
