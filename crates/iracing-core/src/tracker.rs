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
//! - A reset to the pits takes two rows at 60 Hz (seen in an `.ibt`; a probe sampling at about
//!   1 Hz had seen one): first the car out of the world (`PlayerTrackSurface` -1, the lap
//!   fraction 0, both counters 0), then in the stall (`OnPitRoad` and `PlayerCarInPitStall`
//!   rise together, the lap fraction jumps to the stall's). The out-of-world row ends the lap as
//!   a return to the pits; the stall row is tested before the wrap of the lap fraction, which
//!   it would otherwise pass.
//! - `LapLastLapTime` of -1 means the lap was not timed.
//! - Out of the car, with a session loaded, is the garage, also before the first drive; the
//!   live replay the simulator shows then is not a menu. The garage screen is `IsInGarage` or
//!   `IsGarageVisible`.
//! - `FuelLevel` reads 0 out of the car: fuel is read only while driving.
//! - The live tyre readings change only in the row the car enters its stall.
//! - After the session's chequered flag the lap counter rose at a later crossing (the pit lane
//!   on the way in) and fell back about five seconds later: iRacing counts no lap after the
//!   finish. The first crossing with the chequered bit up is the finish and ends its lap as
//!   always; a later one ends a lap only when the counter held for [`LATE_CROSSING_HOLD_S`].

use std::sync::Arc;

use irsdk::defs::session_flags;

use crate::codes::{self, SessionKind};
use crate::io::{
    Clock, Conditions, FinishedLap, LapType, SessionKey, SessionOpenInfo, SetupReason, Sink,
    StallOccasion, StallTyres, TimeSource,
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
/// Seconds a lap counter that rose after the finish must hold before the crossing is a lap.
/// Observed: it fell back after about five seconds.
pub const LATE_CROSSING_HOLD_S: f64 = 10.0;
/// Partial laps shorter than this are dropped.
pub const MIN_PARTIAL_LAP_S: f64 = 5.0;
/// Laps whose top speed stays under this are dropped (km/h).
pub const MOVING_KMH: f32 = 3.0;
/// Seconds without rows after which the session ends.
pub const OFF_GRACE_S: f64 = 5.0;
/// The smallest jump of the lap fraction that is a reset, not driving.
pub const JUMP_PCT_MIN: f64 = 0.02;

/// Seconds after driving starts in which a rise of a lap counter, with no wrap of the lap
/// fraction, only re-syncs the simulator's lap number (re-entry in the stall).
pub const DRIVE_START_QUIET_S: f64 = 1.0;
/// The fraction of the lap that must have been driven for the wrap of the lap fraction alone to
/// end a lap that started at the line.
pub const WRAP_MIN_DRIVEN: f64 = 0.5;

const TRK_LOC_NOT_IN_WORLD: i32 = -1;
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
    /// Fraction of the lap driven forward since the lap started.
    driven: f64,
    /// The first lap of a race that began at the line after the start.
    race_start: bool,
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
        driven: 0.0,
        race_start: false,
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
    /// Why the car is in the pits, for the tyre readings taken in this visit.
    occasion: StallOccasion,
}

/// The tyre readings last sent in the session, as bits so that a repeated value compares equal.
type TyreBits = ([u32; 12], [u32; 12]);

fn tyre_bits(temp: &[[f32; 3]; 4], wear: &[[f32; 3]; 4]) -> TyreBits {
    let mut a = [0u32; 12];
    let mut b = [0u32; 12];
    for (i, v) in temp.iter().flatten().enumerate() {
        a[i] = v.to_bits();
    }
    for (i, v) in wear.iter().flatten().enumerate() {
        b[i] = v.to_bits();
    }
    (a, b)
}

/// How many sets of readings can wait for one lap's time (more in the window replace the last).
const HELD_TYRES_MAX: usize = 4;

/// Tyre readings taken while the lap that ended before them still waits for its time.
struct HeldTyres {
    tyres: StallTyres,
    at: String,
}

/// A crossing after the finish that waits to see whether the counter holds.
#[derive(Clone, Copy)]
struct Late {
    crossed: f64,
    lap: i32,
}

/// A lap that crossed the line and waits for the simulator's time.
struct Pending {
    lap: FinishedLap,
    sectors: SectorClock,
    validity: Validity,
    baseline: f32,
    crossed_at: f64,
    from_line: bool,
    /// The lap started mid-track: the simulator's time is not the time of what was recorded.
    mid_lap: bool,
    race_start: bool,
}

/// How many resolved laps the tracker remembers the validity of.
const RESOLVED_KEEP: usize = 8;

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
    /// The finish crossing (the first with the chequered bit up) was taken.
    finished: bool,
    late: Option<Late>,
    visit: PitVisit,
    prev: Option<Prev>,
    gap_since: Option<f64>,
    /// Monotonic seconds ([`Clock::mono_s`]) when the first row without a connection came.
    off_since: Option<f64>,
    /// Session time of the row that started the current spell of driving.
    drive_since: f64,
    last_seen_last: f32,
    last_fuel: f32,
    conditions: Conditions,
    stall_tyres: Option<StallTyres>,
    /// The readings last sent in this session (or the first driving row's, not sent).
    tyres_sent: Option<TyreBits>,
    /// The next driving row sets the baseline of `tyres_sent`.
    tyres_baseline: bool,
    /// The driver just got into the car from the garage: this row's readings are sent as a
    /// `car_entry` whether or not they changed.
    tyres_entry: bool,
    /// The previous row was that entry row: a stall row right after it is the car appearing in
    /// its stall, not a reset to the pits.
    entry_row: bool,
    /// The inactive lap was ended by a tow: the car comes back in its stall to be serviced there.
    towed: bool,
    /// Sets waiting for the pending lap, oldest first; a fixed array, nothing allocates for it.
    held_tyres: [Option<HeldTyres>; HELD_TYRES_MAX],
    laps_recorded: u32,
    best_s: Option<f32>,
    last_s: Option<f32>,
    pit_mode: u8,
    view: LapView,
    /// The next lap that starts at the line is the first of the race.
    race_start_open: bool,
    /// The simulator's lap number and validity of the last laps sent, newest last.
    resolved: [(i32, bool); RESOLVED_KEEP],
    resolved_n: usize,
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
            finished: false,
            late: None,
            visit: PitVisit {
                was_in_stall: false,
                occasion: StallOccasion::PitStop,
            },
            prev: None,
            gap_since: None,
            off_since: None,
            drive_since: f64::NEG_INFINITY,
            last_seen_last: -1.0,
            last_fuel: 0.0,
            conditions: Conditions::default(),
            stall_tyres: None,
            tyres_sent: None,
            tyres_baseline: true,
            tyres_entry: false,
            entry_row: false,
            towed: false,
            held_tyres: [const { None }; HELD_TYRES_MAX],
            laps_recorded: 0,
            best_s: None,
            last_s: None,
            pit_mode: 4,
            view: LapView::IDLE,
            race_start_open: false,
            resolved: [(0, false); RESOLVED_KEEP],
            resolved_n: 0,
        }
    }

    /// Whether the player's lap ended by crossing the line, with the simulator's lap number
    /// `sim_lap`, was valid, when it was sent recently. The rival rows of the player use it:
    /// their own rule knows nothing of incidents or cut tracks.
    pub fn lap_valid(&self, sim_lap: i32) -> Option<bool> {
        let n = self.resolved_n.min(RESOLVED_KEEP);
        (0..n)
            .map(|k| self.resolved[(self.resolved_n - 1 - k) % RESOLVED_KEEP])
            .find(|(l, _)| *l == sim_lap)
            .map(|(_, v)| v)
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
    ///
    /// The game going away is an interruption, not a return to the pits: a lap open on the
    /// track stays what its last row says (flying unless it was on the pit road).
    pub fn on_off(&mut self, sink: &mut dyn Sink, clock: &dyn Clock) {
        self.close_session(true, true, false, sink, clock);
        self.state = State::Off;
        self.prev = None;
        self.off_since = None;
        self.view = LapView::IDLE;
    }

    /// The recorder stops. With `keep_open` the session is not ended (it may resume) and its
    /// open lap is dropped, not sent: the recorder that resumes the session closes that lap
    /// as a partial one, and sending it here as well would write it twice.
    pub fn stop(&mut self, keep_open: bool, sink: &mut dyn Sink, clock: &dyn Clock) {
        if keep_open {
            self.close_session(false, false, false, sink, clock);
            self.state = State::Off;
            self.prev = None;
            self.view = LapView::IDLE;
        } else {
            self.on_off(sink, clock);
        }
    }

    /// Sends the open lap as a partial lap and leaves the session open: the end of a replayed
    /// file, where nothing resumes.
    pub fn finish_open_lap(&mut self, sink: &mut dyn Sink) {
        let end_s = self.prev.map_or(self.lap.started_at_s, |p| p.time);
        let on_pit_road = self.prev.is_some_and(|p| p.on_pit_road);
        self.end_partial(end_s, on_pit_road, false, sink);
        self.resolve_pending(self.last_seen_last, sink);
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
            // The session time stops while the simulator is gone: the grace is wall time.
            let wall = clock.mono_s();
            let since = *self.off_since.get_or_insert(wall);
            self.state = State::Off;
            self.view.state = State::Off;
            if wall - since >= OFF_GRACE_S {
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
            // The simulator ended or replaced the session under the car: the car was removed
            // from the track, so a lap open on it is a return to the pits.
            self.close_session(true, true, true, sink, clock);
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
        self.race_start_open = meta.kind == SessionKind::Race;
        self.resolved_n = 0;
        self.buffer.clear();
        self.buffer.set_length(length_m);
        self.incidents.reset();
        self.validity.reset();
        self.lap = LapState::NONE;
        self.pending = None;
        self.finished = false;
        self.late = None;
        self.stall_tyres = None;
        self.tyres_sent = None;
        self.tyres_baseline = true;
        self.tyres_entry = false;
        self.entry_row = false;
        self.towed = false;
        self.held_tyres = [const { None }; HELD_TYRES_MAX];
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
    /// Closes the open session. `send_lap`: the open lap goes out as a partial lap; otherwise it
    /// is dropped.
    /// `removed`: the simulator took the car off the track (the session ended or changed); the
    /// lap it interrupts is typed `in` when it was driven on the track.
    fn close_session(
        &mut self,
        end: bool,
        send_lap: bool,
        removed: bool,
        sink: &mut dyn Sink,
        clock: &dyn Clock,
    ) {
        let Some(local) = self.open.as_ref().map(|o| o.local) else {
            return;
        };
        if send_lap {
            let end_s = self.prev.map_or(self.lap.started_at_s, |p| p.time);
            // An interrupted lap is `in` if the last row was on the pit road or the car was
            // removed from the track; a game that goes away leaves it as it was.
            let on_pit_road = self.prev.is_some_and(|p| p.on_pit_road);
            self.end_partial(end_s, on_pit_road, removed, sink);
        } else {
            self.buffer.clear();
        }
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
        let tow = t.tow_time.unwrap_or(0.0) > 0.0;
        let prev = self.prev;
        if matches!(before, State::Off | State::Idle | State::Garage) {
            self.drive_since = now;
        }
        let after_entry = std::mem::take(&mut self.entry_row);

        if !self.lap.active {
            if tow {
                return;
            }
            if before == State::Garage {
                // Getting into the car from the garage: the readings go out on this row, and
                // the stall row that follows is not a reset.
                self.tyres_entry = true;
                self.entry_row = true;
            }
            let kind = if stall {
                StartKind::PitStall
            } else if before == State::Garage {
                StartKind::Garage
            } else {
                StartKind::Resume
            };
            self.start_lap(t, kind, now);
            // After a tow the car waits in its stall and is serviced there (the readings are of
            // the set that came in, like any stop); only a real entry is a car entry.
            let towed = std::mem::take(&mut self.towed) && before != State::Garage;
            self.visit = PitVisit {
                was_in_stall: stall,
                occasion: if towed {
                    StallOccasion::PitStop
                } else {
                    StallOccasion::CarEntry
                },
            };
        } else if let Some(p) = prev {
            let mut reset_now = false;
            let length = self.open.as_ref().map_or(0.0, |o| o.length_m);
            let mut line = false;
            if let Some(l) = self.late {
                if t.lap < l.lap {
                    self.late = None;
                } else if now - l.crossed >= LATE_CROSSING_HOLD_S {
                    self.late = None;
                    self.end_at_line(t, l.crossed, true, sink);
                    self.start_lap(t, StartKind::Line, l.crossed);
                }
            }
            if tow && !p.tow {
                self.validity
                    .mark_tow(t.lap_dist, (now - self.lap.started_at_s) as f32);
                self.end_partial(now, false, false, sink);
                self.towed = true;
                return;
            } else if t.on_pit_road && !p.on_pit_road && stall && !p.in_stall {
                // A reset to the pits: the car reappears in the stall. The lap it ends is a
                // return to the pits, so `in`. Right after getting into the car the same
                // sequence is the car appearing in its stall: nothing was driving, so no reset.
                self.validity
                    .mark_tow(t.lap_dist, (now - self.lap.started_at_s) as f32);
                self.end_partial(now, !after_entry, false, sink);
                self.start_lap(t, StartKind::PitStall, now);
                self.visit = PitVisit {
                    was_in_stall: true,
                    occasion: if after_entry {
                        StallOccasion::CarEntry
                    } else {
                        StallOccasion::Reset
                    },
                };
                reset_now = true;
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
                    // A jump that lands on the pit road is a return to the pits, and so is a
                    // row with the car out of the world (the reset's first row).
                    let to_pits =
                        (t.on_pit_road && !p.on_pit_road) || t.track_loc == TRK_LOC_NOT_IN_WORLD;
                    self.end_partial(now, to_pits, false, sink);
                    self.start_lap(t, StartKind::Resume, now);
                    self.visit.was_in_stall = stall;
                } else if !fell {
                    let counted = t.lap > p.lap || t.lap_completed > p.completed;
                    let wrapped = p.pct > 0.9 && t.lap_dist_pct < 0.1;
                    let ahead = f64::from(t.lap_dist_pct - p.pct).rem_euclid(1.0);
                    let ahead = if ahead < 0.5 { ahead } else { 0.0 };
                    let from_line = self.lap.start_kind == StartKind::Line;
                    // In the stall, nearly stopped or just after driving started, a counter
                    // that rises without the lap fraction wrapping is not a crossing.
                    let still = stall
                        || t.speed * 3.6 < MOVING_KMH
                        || now - self.drive_since < DRIVE_START_QUIET_S;
                    // The wrap alone ends a lap that started at the line only when most of it
                    // was driven (jitter across the line in the pit lane is not a lap).
                    let wrap_ok =
                        wrapped && (!from_line || self.lap.driven + ahead >= WRAP_MIN_DRIVEN);
                    if counted && !wrapped && still {
                        self.lap.sim_lap = t.lap;
                    } else if counted || wrap_ok {
                        if from_line && now - self.lap.started_at_s < SYNC_WINDOW_S {
                            self.lap.sim_lap = t.lap;
                        } else {
                            line = true;
                        }
                    }
                }
                if line {
                    let crossed = crossing_time(p, t);
                    if self.finished {
                        self.late.get_or_insert(Late {
                            crossed,
                            lap: t.lap,
                        });
                    } else {
                        self.finished = t.session_flags & session_flags::CHECKERED != 0;
                        self.end_at_line(t, crossed, false, sink);
                        self.start_lap(t, StartKind::Line, crossed);
                    }
                }
            }
            if t.on_pit_road && !p.on_pit_road && !reset_now {
                self.visit = PitVisit {
                    was_in_stall: stall,
                    occasion: StallOccasion::PitStop,
                };
            }
            if stall || matches!(t.pit_sv_status, Some(1 | 2)) {
                self.visit.was_in_stall = true;
            }
            if p.on_pit_road && !t.on_pit_road {
                self.pit_exit(t, sink, clock);
            }
        }

        self.observe_tyres(t, sink, clock);
        self.sample(t, rise);
    }

    /// Records the row into the open lap.
    fn sample(&mut self, t: &Tick, rise: Option<&crate::validity::IncidentRise>) {
        let now = t.session_time;
        let clock_s = (now - self.lap.started_at_s) as f32;
        self.validity
            .step(t, self.state == State::Track, clock_s, rise);
        if let Some(p) = self.prev {
            let ahead = f64::from(t.lap_dist_pct - p.pct).rem_euclid(1.0);
            if ahead < 0.5 {
                self.lap.driven += ahead;
            }
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
            driven: 0.0,
            race_start: kind == StartKind::Line && self.race_start_open,
        };
        if kind == StartKind::Line {
            self.race_start_open = false;
        }
        self.last_fuel = t.fuel_l;
        self.validity.reset();
        if kind == StartKind::Resume {
            self.validity.mark_mid_lap_start(t.lap_dist);
        }
        self.sectors.start_lap(at_s, kind != StartKind::Line);
        if kind != StartKind::Line {
            self.sectors.align(f64::from(t.lap_dist_pct));
        }
        self.buffer.clear();
        self.buffer
            .begin_position(at_s, kind == StartKind::Line && !t.on_pit_road);
    }

    fn take_buffer(&mut self) -> LapBuffer {
        let length = self.open.as_ref().map_or(0.0, |o| o.length_m);
        // The sink refills the pool after it takes a lap; only if it does not is one made here.
        let mut fresh = self
            .spare
            .take()
            .unwrap_or_else(|| LapBuffer::with_capacity(self.pool_rows));
        fresh.set_length(length);
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
    /// `late`: the lap ends ten seconds after its crossing (a crossing after the finish), so
    /// its rows do not end at the line.
    fn end_at_line(&mut self, t: &Tick, crossed: f64, late: bool, sink: &mut dyn Sink) {
        self.resolve_pending(t.lap_last_time, sink);
        let duration = crossed - self.lap.started_at_s;
        if self.lap.start_kind != StartKind::Line && duration < MIN_PARTIAL_LAP_S {
            self.buffer.clear();
            return;
        }
        if self.lap.touched_pit {
            self.validity.mark_pit_road();
        }
        self.buffer.finish_position(
            (!late && !t.on_pit_road).then_some(crossed),
            !self.lap.touched_pit,
        );
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
            mid_lap: self.lap.start_kind == StartKind::Resume,
            race_start: self.lap.race_start,
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
        self.buffer.finish_position(None, false);
        let lap_type = self.lap.lap_type(ended_on_pit_road, garage_return);
        let mut lap = self.base_lap(end_s, lap_type, true);
        self.fill_validity(&mut lap, &self.validity.clone());
        if let Some(local) = self.open.as_ref().map(|o| o.local) {
            self.laps_recorded += 1;
            self.send_lap(local, lap, sink);
        }
    }

    /// Hands a lap to the sink and takes the fresh buffer it offers for the pool.
    fn send_lap(&mut self, local: u64, lap: FinishedLap, sink: &mut dyn Sink) {
        sink.lap(local, lap);
        if self.spare.is_none() {
            self.spare = sink.spare_buffer(self.pool_rows);
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
            self.flush_tyres(sink);
            return;
        };
        let changed = last != p.baseline;
        let measured = (p.crossed_at - p.lap.started_at_s) as f32;
        if p.from_line {
            p.lap.measured_time_s = Some(measured);
        }
        let mut official = None;
        if changed && last > 0.0 && !p.mid_lap {
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
        p.lap.sectors = p.sectors.finish_lap(p.crossed_at, official, p.race_start);
        self.fill_validity(&mut p.lap, &p.validity);
        self.resolved[self.resolved_n % RESOLVED_KEEP] = (p.lap.sim_lap, p.lap.valid);
        self.resolved_n += 1;
        if let Some(t) = p.lap.time_s
            && p.lap.valid
        {
            self.best_s = Some(self.best_s.map_or(t, |b| b.min(t)));
        }
        self.last_s = p.lap.time_s.or(self.last_s);
        self.laps_recorded += 1;
        if let Some(local) = self.open.as_ref().map(|o| o.local) {
            self.send_lap(local, p.lap, sink);
        }
        self.flush_tyres(sink);
    }

    // ---- pits ---------------------------------------------------------------------------

    /// Looks at the tyre readings on a row in the pits and sends them when they are a new set.
    ///
    /// The simulator changes its live readings only in the pits (a stop, a reset, getting into
    /// the car), so on track nothing is compared. The first driving row of a session is the
    /// baseline and is not sent unless the car is in its stall; after that a row on the pit road
    /// or in the stall whose readings differ from the last ones sent is a new set.
    fn observe_tyres(&mut self, t: &Tick, sink: &mut dyn Sink, clock: &dyn Clock) {
        let (Some(temp), Some(wear)) = (t.tyre_temp, t.tyre_wear) else {
            return;
        };
        let stall = in_stall(t);
        let entry = std::mem::take(&mut self.tyres_entry);
        let bits = if entry {
            self.tyres_baseline = false;
            tyre_bits(&temp, &wear)
        } else if self.tyres_baseline {
            self.tyres_baseline = false;
            let bits = tyre_bits(&temp, &wear);
            if !stall {
                self.tyres_sent = Some(bits);
                return;
            }
            bits
        } else if t.on_pit_road || stall {
            tyre_bits(&temp, &wear)
        } else {
            return;
        };
        if !entry && self.tyres_sent == Some(bits) {
            return;
        }
        self.tyres_sent = Some(bits);
        let tyres = StallTyres {
            at_s: t.session_time,
            occasion: self.visit.occasion,
            temp_c: temp,
            wear,
        };
        self.stall_tyres = Some(tyres);
        let at = clock.iso();
        if self.pending.is_some() {
            // The lap that ended before the readings goes first.
            let slot = self
                .held_tyres
                .iter()
                .position(Option::is_none)
                .unwrap_or(HELD_TYRES_MAX - 1);
            self.held_tyres[slot] = Some(HeldTyres { tyres, at });
        } else if let Some(local) = self.open.as_ref().map(|o| o.local) {
            sink.stall_tyres(local, &at, &tyres);
        }
    }

    /// Sends readings that waited for a lap.
    fn flush_tyres(&mut self, sink: &mut dyn Sink) {
        let local = self.open.as_ref().map(|o| o.local);
        for h in self.held_tyres.iter_mut().filter_map(Option::take) {
            if let Some(local) = local {
                sink.stall_tyres(local, &h.at, &h.tyres);
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
        t.yaw_north = Some(0.0);
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
        // The menu's "return to pits" ends the lap as an in lap, not a flying one.
        assert_eq!(lap.lap_type, LapType::In);
    }

    /// A line crossing, then 19 s of flying lap; returns the clock and the lap count so far.
    fn crossing_then_driving(
        tracker: &mut Tracker,
        meta: &Arc<SessionMeta>,
        sink: &mut RecordingSink,
    ) -> (f64, f32, i32) {
        let none = (0, usize::MAX);
        let at = drive_flagged(tracker, meta, sink, (0.0, 0.5, 3), 2600, none);
        drive_flagged(tracker, meta, sink, at, 1140, none)
    }

    #[test]
    fn tracker_session_ending_under_a_car_on_track_types_the_lap_in() {
        let (meta, mut sink, clock) = (meta(), RecordingSink::new(), FixedClock::default());
        let mut tracker = Tracker::new(64);
        let (time, pct, _) = crossing_then_driving(&mut tracker, &meta, &mut sink);
        // The menu's return to pits at the end of the session: the next session's identity
        // arrives and the car is out of the world in the same row.
        let mut t = driving(time + 0.02, pct);
        t.session_unique_id = 2;
        t.is_on_track = false;
        t.is_on_track_car = false;
        tracker.step(&t, Some(&meta), &mut sink, &clock);
        let laps = sink.laps();
        assert_eq!(laps.len(), 2, "the crossing and the interrupted lap");
        assert_eq!(
            laps[0].lap_type,
            LapType::Flying,
            "a real crossing is unchanged"
        );
        assert!(!laps[0].partial);
        assert!(laps[1].partial && !laps[1].valid);
        assert_eq!(laps[1].lap_type, LapType::In);
    }

    #[test]
    fn tracker_leaving_the_car_mid_lap_types_the_lap_in() {
        let (meta, mut sink, clock) = (meta(), RecordingSink::new(), FixedClock::default());
        let mut tracker = Tracker::new(64);
        let (time, pct, _) = crossing_then_driving(&mut tracker, &meta, &mut sink);
        let mut t = driving(time + 0.02, pct);
        t.is_on_track = false;
        t.is_on_track_car = false;
        t.is_in_garage = true;
        tracker.step(&t, Some(&meta), &mut sink, &clock);
        let laps = sink.laps();
        assert_eq!(laps.len(), 2);
        assert_eq!(laps[1].lap_type, LapType::In);
    }

    #[test]
    fn tracker_jump_onto_the_pit_road_types_the_lap_in() {
        let (meta, mut sink, clock) = (meta(), RecordingSink::new(), FixedClock::default());
        let mut tracker = Tracker::new(64);
        let (time, _, _) = crossing_then_driving(&mut tracker, &meta, &mut sink);
        // The car reappears on the pit road, not in the stall.
        let mut t = driving(time + 0.1, 0.02);
        t.on_pit_road = true;
        t.track_loc = 2;
        tracker.step(&t, Some(&meta), &mut sink, &clock);
        let laps = sink.laps();
        assert_eq!(laps.len(), 2);
        assert_eq!(laps[1].lap_type, LapType::In);
    }

    /// 20 s of driving at 60 Hz on lap 6 (counter 5), ending at the lap fraction of the real rows.
    fn drive_to_the_reset(
        tracker: &mut Tracker,
        meta: &Arc<SessionMeta>,
        sink: &mut RecordingSink,
    ) -> f64 {
        let clock = FixedClock::default();
        let mut time = 1041.4833;
        for i in 0..1200 {
            time += 1.0 / 60.0;
            let mut t = driving(time, 0.1067 * (i + 1) as f32 / 1200.0);
            t.lap = 6;
            t.lap_completed = 5;
            tracker.step(&t, Some(meta), sink, &clock);
        }
        time
    }

    /// One row of the reset sequence: (pct, speed, lap, completed, on track, on pit road, in
    /// stall, track surface).
    type ResetRow = (f32, f32, i32, i32, bool, bool, bool, i32);

    /// A [`ResetRow`] as a tick at `time`.
    fn reset_row(time: f64, r: ResetRow) -> Box<Tick> {
        let mut t = driving(time, r.0);
        t.speed = r.1;
        t.lap = r.2;
        t.lap_completed = r.3;
        t.is_on_track = r.4;
        t.is_on_track_car = r.4;
        t.on_pit_road = r.5;
        t.in_pit_stall = Some(r.6);
        t.track_loc = r.7;
        t
    }

    // Rows from an iRacing .ibt (60 Hz), a reset to the pits at session time 1061.5:
    // the car is out of the world for one row, then in the stall.
    const REAL_ROWS: [ResetRow; 4] = [
        (0.1067, 0.0001, 6, 5, true, false, false, 3),
        (0.0, 0.0, 0, 0, true, false, false, -1),
        (0.0313, 0.0485, 6, 5, true, true, true, 1),
        (0.0313, 0.0743, 6, 5, true, true, true, 1),
    ];

    #[test]
    fn tracker_reset_through_an_out_of_world_row_types_the_lap_in() {
        let (meta, mut sink, clock) = (meta(), RecordingSink::new(), FixedClock::default());
        let mut tracker = Tracker::new(64);
        let time = drive_to_the_reset(&mut tracker, &meta, &mut sink);
        for (i, r) in REAL_ROWS.iter().enumerate() {
            let t = reset_row(time + (i + 1) as f64 / 60.0, *r);
            tracker.step(&t, Some(&meta), &mut sink, &clock);
        }
        let laps = sink.laps();
        assert_eq!(
            laps.len(),
            1,
            "one lap, the tiny one the reset opens is dropped"
        );
        assert!(laps[0].partial);
        assert_eq!(laps[0].lap_type, LapType::In);
        // The next lap starts from the stall.
        let mut at = time + 5.0 / 60.0;
        for _ in 0..600 {
            at += 1.0 / 60.0;
            let mut t = reset_row(at, (0.0313, 30.0, 6, 5, true, false, false, 3));
            t.lap_dist_pct += 0.0001;
            tracker.step(&t, Some(&meta), &mut sink, &clock);
        }
        tracker.on_off(&mut sink, &clock);
        let laps = sink.laps();
        assert_eq!(laps.len(), 2);
        assert_eq!(laps[1].lap_type, LapType::Out);
    }

    #[test]
    fn tracker_out_of_world_row_then_the_garage_types_one_lap_in() {
        let (meta, mut sink, clock) = (meta(), RecordingSink::new(), FixedClock::default());
        let mut tracker = Tracker::new(64);
        let time = drive_to_the_reset(&mut tracker, &meta, &mut sink);
        let rows = [
            REAL_ROWS[0],
            REAL_ROWS[1],
            (0.0, 0.0, 0, 0, false, false, false, -1),
            (0.0, 0.0, 0, 0, false, false, false, -1),
        ];
        for (i, r) in rows.iter().enumerate() {
            let mut t = reset_row(time + (i + 1) as f64 / 60.0, *r);
            t.is_in_garage = i >= 2;
            tracker.step(&t, Some(&meta), &mut sink, &clock);
        }
        let laps = sink.laps();
        assert_eq!(laps.len(), 1);
        assert!(laps[0].partial);
        assert_eq!(laps[0].lap_type, LapType::In);
    }

    #[test]
    fn tracker_the_game_going_away_mid_lap_leaves_the_lap_as_driven() {
        let (meta, mut sink, clock) = (meta(), RecordingSink::new(), FixedClock::default());
        let mut tracker = Tracker::new(64);
        crossing_then_driving(&mut tracker, &meta, &mut sink);
        tracker.on_off(&mut sink, &clock);
        let laps = sink.laps();
        assert_eq!(laps.len(), 2);
        assert!(laps[1].partial);
        assert_eq!(laps[1].lap_type, LapType::Flying);
    }

    /// Drives `rows` rows at 60 Hz from `(time, pct, lap)` with `flags` up. From row `fall_from`
    /// of the call the lap counters read one lap less (a counter that falls back).
    fn drive_flagged(
        tracker: &mut Tracker,
        meta: &Arc<SessionMeta>,
        sink: &mut RecordingSink,
        (mut time, mut pct, mut lap): (f64, f32, i32),
        rows: usize,
        (flags, fall_from): (u32, usize),
    ) -> (f64, f32, i32) {
        let clock = FixedClock::default();
        for i in 0..rows {
            time += 1.0 / 60.0;
            pct += 0.0002;
            if pct >= 1.0 {
                pct -= 1.0;
                lap += 1;
            }
            let counter = if i >= fall_from { lap - 1 } else { lap };
            let mut t = driving(time, pct);
            t.session_flags = flags;
            t.lap = counter;
            t.lap_completed = counter - 1;
            tracker.step(&t, Some(meta), sink, &clock);
        }
        (time, pct, lap)
    }

    /// A lap, then the finish crossing with the chequered bit up; returns where it ended and
    /// how many laps were sent.
    fn to_the_finish(
        tracker: &mut Tracker,
        meta: &Arc<SessionMeta>,
        sink: &mut RecordingSink,
    ) -> ((f64, f32, i32), usize) {
        let none = (0, usize::MAX);
        let at = drive_flagged(tracker, meta, sink, (0.0, 0.0, 3), 4000, none);
        let flagged = (session_flags::CHECKERED, usize::MAX);
        let at = drive_flagged(tracker, meta, sink, at, 1500, flagged);
        let finished = sink.laps().len();
        assert!(finished >= 1, "the finish crossing ended its lap");
        (at, finished)
    }

    #[test]
    fn tracker_a_crossing_after_the_finish_that_falls_back_is_no_lap() {
        let (meta, mut sink) = (meta(), RecordingSink::new());
        let mut tracker = Tracker::new(64);
        let (at, finished) = to_the_finish(&mut tracker, &meta, &mut sink);
        // The next crossing is at row 4500 of the cool-down; the counter is back a second later.
        let back = (session_flags::CHECKERED, 4560);
        drive_flagged(&mut tracker, &meta, &mut sink, at, 6000, back);
        assert_eq!(
            sink.laps().len(),
            finished,
            "the late crossing was taken back"
        );
    }

    #[test]
    fn tracker_a_crossing_after_the_finish_that_holds_is_a_lap() {
        let (meta, mut sink) = (meta(), RecordingSink::new());
        let mut tracker = Tracker::new(64);
        let (at, finished) = to_the_finish(&mut tracker, &meta, &mut sink);
        let held = (session_flags::CHECKERED, usize::MAX);
        drive_flagged(&mut tracker, &meta, &mut sink, at, 6000, held);
        assert_eq!(
            sink.laps().len(),
            finished + 1,
            "a counter that holds is a lap"
        );
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

    /// Steps `rows` rows at 60 Hz from `pct`, wrapping the lap fraction and counting the lap at
    /// the wrap. Returns the time and the lap fraction reached.
    fn drive_rows(
        tracker: &mut Tracker,
        meta: &Arc<SessionMeta>,
        sink: &mut RecordingSink,
        (mut time, mut pct): (f64, f32),
        rows: usize,
        last: f32,
    ) -> (f64, f32) {
        let clock = FixedClock::default();
        let mut lap = 3;
        for _ in 0..rows {
            time += 1.0 / 60.0;
            pct += 0.0002;
            if pct >= 1.0 {
                pct -= 1.0;
                lap += 1;
            }
            let mut t = driving(time, pct);
            t.lap = lap;
            t.lap_completed = lap - 1;
            t.lap_last_time = last;
            tracker.step(&t, Some(meta), sink, &clock);
        }
        (time, pct)
    }

    /// An in lap that crossed the line on the pit road and still waits for the simulator's time
    /// while the tyre readings change: the readings go after the lap, not before it.
    #[test]
    fn tracker_tyre_readings_wait_for_the_lap_that_ended_before_them() {
        let (meta, mut sink, clock) = (meta(), RecordingSink::new(), FixedClock::default());
        let mut tracker = Tracker::new(64);
        let (old, new) = ([[75.0f32; 3]; 4], [[80.0f32; 3]; 4]);
        let wear = [[1.0f32; 3]; 4];
        let (mut time, mut pct, mut lap, mut last) = (0.0, 0.9f32, 3, -1.0f32);
        for i in 0..700 {
            time += 1.0 / 60.0;
            pct += 0.0002;
            if pct >= 1.0 {
                pct -= 1.0;
                lap += 1;
            }
            let after_line = lap > 3;
            if i == 640 {
                last = 30.0;
            }
            let mut t = driving(time, pct);
            t.lap = lap;
            t.lap_completed = lap - 1;
            t.lap_last_time = last;
            t.on_pit_road = i >= 400;
            t.track_loc = if t.on_pit_road { 2 } else { 3 };
            t.tyre_temp = Some(if after_line && i > 520 { new } else { old });
            t.tyre_wear = Some(wear);
            tracker.step(&t, Some(&meta), &mut sink, &clock);
            if i == 600 {
                assert!(
                    sink.calls("lap").is_empty(),
                    "the lap still waits for its time"
                );
                assert!(
                    sink.calls("stall_tyres").is_empty(),
                    "the readings wait for the lap"
                );
                assert_eq!(tracker.stall_tyres().expect("seen").temp_c, new);
            }
        }
        let kinds: Vec<&str> = sink
            .kinds()
            .into_iter()
            .filter(|k| matches!(*k, "lap" | "stall_tyres"))
            .collect();
        assert_eq!(kinds, ["lap", "stall_tyres"]);
    }

    /// A row of the in lap used by the tyre tests: the car on the pit road from row 400, the
    /// line crossed near row 500, the simulator's lap time not given until `last` changes.
    fn in_lap_row(i: usize, last: f32, temp: [[f32; 3]; 4]) -> Box<Tick> {
        let total = 0.9 + 0.0002 * i as f32;
        let mut t = driving(1.0 / 60.0 * (i as f64 + 1.0), total.fract());
        t.lap = if total >= 1.0 { 4 } else { 3 };
        t.lap_completed = t.lap - 1;
        t.lap_last_time = last;
        t.on_pit_road = i >= 400;
        t.track_loc = if t.on_pit_road { 2 } else { 3 };
        t.tyre_temp = Some(temp);
        t.tyre_wear = Some([[1.0f32; 3]; 4]);
        t
    }

    /// Two changes of readings while the lap still waits for its time: both are sent, in order,
    /// after the lap.
    #[test]
    fn tracker_two_sets_of_readings_wait_for_one_lap_and_both_go() {
        let (meta, mut sink, clock) = (meta(), RecordingSink::new(), FixedClock::default());
        let mut tracker = Tracker::new(64);
        let sets = [[[75.0f32; 3]; 4], [[80.0f32; 3]; 4], [[85.0f32; 3]; 4]];
        for i in 0..700 {
            let temp = match i {
                0..=520 => sets[0],
                521..=560 => sets[1],
                _ => sets[2],
            };
            let last = if i >= 640 { 30.0 } else { -1.0 };
            let t = in_lap_row(i, last, temp);
            tracker.step(&t, Some(&meta), &mut sink, &clock);
            if i == 600 {
                assert!(sink.calls("lap").is_empty(), "the lap still waits");
                assert!(sink.calls("stall_tyres").is_empty());
            }
        }
        let kinds: Vec<&str> = sink
            .kinds()
            .into_iter()
            .filter(|k| matches!(*k, "lap" | "stall_tyres"))
            .collect();
        assert_eq!(kinds, ["lap", "stall_tyres", "stall_tyres"]);
    }

    /// The stall flag going on and off with the readings unchanged is not a new set.
    #[test]
    fn tracker_the_stall_flag_toggling_with_unchanged_readings_sends_nothing() {
        let (meta, mut sink, clock) = (meta(), RecordingSink::new(), FixedClock::default());
        let mut tracker = Tracker::new(64);
        let temp = [[75.0f32; 3]; 4];
        for i in 0..400 {
            let mut t = driving(1.0 / 60.0 * (i as f64 + 1.0), 0.3 + 0.0001 * i as f32);
            t.on_pit_road = i >= 100;
            let stall = (150..200).contains(&i) || (250..300).contains(&i);
            t.in_pit_stall = Some(stall);
            t.track_loc = if stall {
                1
            } else if t.on_pit_road {
                2
            } else {
                3
            };
            t.speed = if stall { 0.0 } else { 20.0 };
            t.tyre_temp = Some(temp);
            t.tyre_wear = Some([[1.0f32; 3]; 4]);
            tracker.step(&t, Some(&meta), &mut sink, &clock);
        }
        assert!(sink.calls("stall_tyres").is_empty());
    }

    /// A row of the live probe `probe-20261111T180035Z` (60 Hz, offline Test Drive): the
    /// readings of `LFtempCM` and `LFwearM` on all wheels, the flags as recorded.
    fn probe_row(time: f64, state: &str, pct: f32, temp: f32, wear: f32) -> Box<Tick> {
        let mut t = driving(time, pct);
        t.lap = 1;
        t.lap_completed = 0;
        t.speed = 0.0;
        t.tyre_temp = Some([[temp; 3]; 4]);
        t.tyre_wear = Some([[wear; 3]; 4]);
        match state {
            // Out of the car in the garage (SessionTime 107.7 to 178.68).
            "garage" => {
                t.is_on_track = false;
                t.is_on_track_car = false;
                t.on_pit_road = false;
                t.in_pit_stall = Some(false);
                t.track_loc = -1;
                t.lap = 0;
            }
            // The entry row (178.6833) and the row out of the world of a reset (334.95,
            // 369.3166): on track, off the pit road, out of the stall, surface -1, lap 0.
            "world" => {
                t.on_pit_road = false;
                t.in_pit_stall = Some(false);
                t.track_loc = -1;
                t.lap = 0;
            }
            // The stall row 17 ms later (178.7, 334.9833, 369.3333).
            "stall" => {
                t.on_pit_road = true;
                t.in_pit_stall = Some(true);
                t.track_loc = 1;
            }
            // Driving on the track.
            _ => {
                t.on_pit_road = false;
                t.in_pit_stall = Some(false);
                t.track_loc = 3;
                t.speed = 5.0;
            }
        }
        t
    }

    fn occasions(sink: &RecordingSink) -> Vec<(String, f64)> {
        sink.calls("stall_tyres")
            .iter()
            .map(|c| {
                (
                    c["occasion"].as_str().expect("occasion").to_owned(),
                    c["temp_c"][0][0].as_f64().expect("temp"),
                )
            })
            .collect()
    }

    /// The car entry, the two resets and leaving the car, from the rows of
    /// `probe-20261111T180035Z`, then a second entry with the readings unchanged.
    #[test]
    fn tracker_car_entry_is_sent_every_time_and_resets_stay_resets() {
        let (meta, mut sink, clock) = (meta(), RecordingSink::new(), FixedClock::default());
        let mut tracker = Tracker::new(64);
        let mut step = |t: Box<Tick>, sink: &mut RecordingSink| {
            tracker.step(&t, Some(&meta), sink, &clock);
        };
        let mut time = 107.7;
        // Out of the car, then the entry (178.6833) and its stall row (178.7).
        while time < 178.66 {
            time += 1.0 / 60.0;
            step(probe_row(time, "garage", 0.0, 42.052, 1.0), &mut sink);
        }
        step(probe_row(178.683, "world", 0.0, 42.052, 1.0), &mut sink);
        assert_eq!(occasions(&sink), [("car_entry".to_owned(), 42.052)]);
        step(probe_row(178.700, "stall", 0.0313, 42.052, 1.0), &mut sink);
        assert_eq!(occasions(&sink).len(), 1, "the stall row sends nothing");
        // Leaves the stall and drives (204.8 to 334.93).
        let mut pct = 0.0313;
        time = 178.7;
        while time < 204.8 {
            time += 1.0 / 60.0;
            step(probe_row(time, "stall", 0.0313, 42.052, 1.0), &mut sink);
        }
        while time < 334.93 {
            time += 1.0 / 60.0;
            pct += 0.00002;
            step(probe_row(time, "track", pct, 42.052, 1.0), &mut sink);
        }
        // Reset from the track (334.95, stall row 334.9833): `reset`.
        step(probe_row(334.95, "world", 0.0, 57.04, 0.996), &mut sink);
        step(probe_row(334.983, "stall", 0.0313, 57.04, 0.996), &mut sink);
        // Out lap, then a reset during it (369.3166, 369.3333): `reset`.
        pct = 0.0313;
        time = 334.99;
        while time < 351.18 {
            time += 1.0 / 60.0;
            step(probe_row(time, "stall", 0.0313, 57.04, 0.996), &mut sink);
        }
        while time < 369.30 {
            time += 1.0 / 60.0;
            pct += 0.00002;
            step(probe_row(time, "track", pct, 57.04, 0.996), &mut sink);
        }
        step(probe_row(369.317, "world", 0.0, 43.08, 0.9998), &mut sink);
        step(
            probe_row(369.333, "stall", 0.0313, 43.08, 0.9998),
            &mut sink,
        );
        // Out of the car to the menu (385.5333): no new readings, nothing written.
        time = 369.34;
        while time < 385.52 {
            time += 1.0 / 60.0;
            step(probe_row(time, "stall", 0.0313, 43.08, 0.9998), &mut sink);
        }
        while time < 399.98 {
            time += 1.0 / 60.0;
            let mut t = probe_row(time, "stall", 0.0313, 43.08, 0.9998);
            t.is_on_track = false;
            step(t, &mut sink);
        }
        let seen = occasions(&sink);
        assert_eq!(
            seen,
            [
                ("car_entry".to_owned(), 42.052),
                ("reset".to_owned(), 57.04),
                ("reset".to_owned(), 43.08),
            ]
        );
        // A second entry in the same session, readings equal to the last ones.
        step(probe_row(400.0, "world", 0.0, 43.08, 0.9998), &mut sink);
        step(
            probe_row(400.017, "stall", 0.0313, 43.08, 0.9998),
            &mut sink,
        );
        let seen = occasions(&sink);
        assert_eq!(seen.len(), 4);
        assert_eq!(seen[3], ("car_entry".to_owned(), 43.08));
    }

    /// After a tow the car waits in its stall while the tow time counts down; the stop that
    /// follows is a pit stop, not a car entry.
    #[test]
    fn tracker_the_stop_after_a_tow_is_a_pit_stop() {
        let (meta, mut sink, clock) = (meta(), RecordingSink::new(), FixedClock::default());
        let mut tracker = Tracker::new(64);
        let mut time = 100.0;
        let mut pct = 0.3f32;
        let mut step = |t: Box<Tick>, sink: &mut RecordingSink| {
            tracker.step(&t, Some(&meta), sink, &clock);
        };
        // Driving; the first row is the baseline.
        for _ in 0..120 {
            time += 1.0 / 60.0;
            pct += 0.0002;
            step(probe_row(time, "track", pct, 42.052, 1.0), &mut sink);
        }
        // The tow is asked for: the tow time is up while on track, then the car is in its stall.
        let mut tow = 141.62f32;
        for i in 0..240 {
            time += 1.0 / 60.0;
            let state = if i < 60 { "track" } else { "stall" };
            let mut t = probe_row(time, state, pct, 42.052, 1.0);
            t.tow_time = Some(tow);
            step(t, &mut sink);
        }
        // The wait in the stall, the tow time counting down to 0.
        while tow > 0.0 {
            time += 1.0 / 60.0;
            tow = (tow - 1.0 / 60.0).max(0.0);
            let mut t = probe_row(time, "stall", pct, 42.052, 1.0);
            t.tow_time = Some(tow);
            step(t, &mut sink);
        }
        assert!(sink.calls("stall_tyres").is_empty());
        // The service starts on the row the tow time reaches 0; the readings change.
        for _ in 0..60 {
            time += 1.0 / 60.0;
            let mut t = probe_row(time, "stall", pct, 55.0, 0.99);
            t.tow_time = Some(0.0);
            step(t, &mut sink);
        }
        assert_eq!(occasions(&sink), [("pit_stop".to_owned(), 55.0)]);
    }

    /// A recorder that starts while the car is already driving takes its first row as the
    /// baseline and records nothing for it.
    #[test]
    fn tracker_a_recorder_started_on_track_sends_no_car_entry() {
        let (meta, mut sink, clock) = (meta(), RecordingSink::new(), FixedClock::default());
        let mut tracker = Tracker::new(64);
        for i in 0..120 {
            let t = probe_row(
                100.0 + f64::from(i) / 60.0,
                "track",
                0.3 + 0.0002 * i as f32,
                42.052,
                1.0,
            );
            tracker.step(&t, Some(&meta), &mut sink, &clock);
        }
        assert!(sink.calls("stall_tyres").is_empty());
    }

    /// The session changes while readings wait for the lap: they go before the old session's
    /// end, and the new session starts with a fresh baseline.
    #[test]
    fn tracker_a_session_end_flushes_held_readings_and_the_next_starts_clean() {
        let (meta, mut sink, clock) = (meta(), RecordingSink::new(), FixedClock::default());
        let mut tracker = Tracker::new(64);
        let (old, new) = ([[75.0f32; 3]; 4], [[80.0f32; 3]; 4]);
        for i in 0..600 {
            let t = in_lap_row(i, -1.0, if i > 520 { new } else { old });
            tracker.step(&t, Some(&meta), &mut sink, &clock);
        }
        assert!(sink.calls("stall_tyres").is_empty(), "held for the lap");
        // A new session: other readings on track are only its baseline.
        for i in 600..700 {
            let mut t = in_lap_row(i, -1.0, [[90.0f32; 3]; 4]);
            t.session_unique_id = 2;
            t.on_pit_road = false;
            t.track_loc = 3;
            tracker.step(&t, Some(&meta), &mut sink, &clock);
        }
        let kinds: Vec<&str> = sink
            .kinds()
            .into_iter()
            .filter(|k| matches!(*k, "stall_tyres" | "end_session" | "start_session"))
            .collect();
        assert_eq!(
            kinds,
            [
                "start_session",
                "stall_tyres",
                "end_session",
                "start_session"
            ]
        );
        assert!(tracker.stall_tyres().is_none(), "nothing carried over");
    }

    #[test]
    fn tracker_a_lap_started_mid_track_is_sent_invalid_with_no_official_time() {
        let (meta, mut sink) = (meta(), RecordingSink::new());
        let mut tracker = Tracker::new(64);
        // Driving starts at 0.9 of the lap, crosses the line, and the simulator times the lap.
        let at = drive_rows(&mut tracker, &meta, &mut sink, (0.0, 0.9), 560, -1.0);
        drive_rows(&mut tracker, &meta, &mut sink, at, 300, 95.5);
        tracker.finish_open_lap(&mut sink);
        let first = &sink.laps()[0];
        assert!(!first.partial, "it ended at the line");
        assert!(!first.valid);
        assert_eq!(first.cause, Some(InvalidCause::Unknown));
        assert!(!first.cause_certain);
        assert_eq!(
            first.time_s, None,
            "the official time is not this trace's time"
        );
        assert_eq!(tracker.view().best_s, None);
        // It began mid-track and ended at the line: the end anchor puts the line at the origin.
        assert!(first.buffer.present().position);
        let c = first.buffer.columns();
        assert!(c.x[c.x.len() - 1].hypot(c.z[c.z.len() - 1]) < 1.0);
        // The next lap began at the line and was cut short: the start anchor is kept.
        let second = &sink.laps()[1];
        assert!(second.partial && second.buffer.present().position);
        assert!(second.buffer.columns().z[0].abs() < 1.0);
    }

    /// A session of `kind` with three sectors; the car starts 0.03 of a lap before the line
    /// and the simulator times the first lap 2 s longer than the drive from the line.
    fn first_lap_after_the_start(kind: &str) -> (Tracker, RecordingSink) {
        let text = YamlBuilder::new()
            .session(0, kind)
            .sectors(3)
            .build_cp1252();
        let meta = Arc::new(meta::build(&SessionDoc::from_raw(1, &text), 0));
        let mut sink = RecordingSink::new();
        let mut tracker = Tracker::new(64);
        let at = drive_rows(&mut tracker, &meta, &mut sink, (0.0, 0.97), 5180, -1.0);
        drive_rows(&mut tracker, &meta, &mut sink, at, 100, 85.33 + 2.0);
        (tracker, sink)
    }

    #[test]
    fn tracker_the_first_race_lap_keeps_its_sectors_when_timed_from_the_start() {
        let (tracker, sink) = first_lap_after_the_start("Race");
        let lap = sink
            .laps()
            .iter()
            .find(|l| l.time_s.is_some_and(|t| (t - 87.33).abs() < 0.01))
            .expect("the race lap");
        let s = lap.sectors.as_slice();
        assert_eq!(s.len(), 3);
        assert!(s.iter().all(Option::is_some), "{s:?}");
        let sum: f32 = s.iter().flatten().sum();
        assert!((sum - 87.33).abs() < 0.01, "{sum}");
        assert!(tracker.lap_valid(lap.sim_lap).is_some());
    }

    #[test]
    fn tracker_the_same_mismatch_outside_a_race_start_drops_the_sectors() {
        let (_, sink) = first_lap_after_the_start("Lone Qualify");
        let lap = sink
            .laps()
            .iter()
            .find(|l| l.time_s.is_some_and(|t| (t - 87.33).abs() < 0.01))
            .expect("the lap");
        assert!(lap.sectors.as_slice().iter().all(Option::is_none));
    }

    #[test]
    fn tracker_a_lap_started_mid_track_shows_the_sector_it_is_in() {
        let text = YamlBuilder::new().sectors(3).build_cp1252();
        let meta = Arc::new(meta::build(&SessionDoc::from_raw(1, &text), 0));
        let mut sink = RecordingSink::new();
        let mut tracker = Tracker::new(64);
        drive_rows(&mut tracker, &meta, &mut sink, (0.0, 0.5), 30, -1.0);
        assert_eq!(tracker.view().sector, Some(1));
    }

    #[test]
    fn tracker_a_lap_with_neither_end_at_the_line_has_no_position() {
        let (meta, mut sink) = (meta(), RecordingSink::new());
        let mut tracker = Tracker::new(64);
        drive_rows(&mut tracker, &meta, &mut sink, (0.0, 0.5), 900, -1.0);
        tracker.finish_open_lap(&mut sink);
        let lap = &sink.laps()[0];
        assert!(lap.partial);
        assert!(!lap.buffer.present().position);
        assert_eq!(lap.buffer.columns().x.len(), lap.buffer.len());
    }

    #[test]
    fn tracker_a_disconnect_ends_the_session_in_wall_time() {
        let (meta, mut sink, clock) = (meta(), RecordingSink::new(), FixedClock::default());
        let mut tracker = Tracker::new(64);
        drive_rows(&mut tracker, &meta, &mut sink, (0.0, 0.5), 600, -1.0);
        assert!(tracker.session_local().is_some());
        // The session time stops while the simulator is gone.
        let mut gone = driving(10.0, 0.62);
        gone.connected = false;
        for i in 0..100 {
            clock.advance(0.05);
            tracker.step(&gone, Some(&meta), &mut sink, &clock);
            assert!(i >= 99 || tracker.session_local().is_some(), "row {i}");
        }
        assert_eq!(tracker.state(), State::Off);
        assert!(
            tracker.session_local().is_some(),
            "4.95 s: still within the grace"
        );
        clock.advance(0.1);
        tracker.step(&gone, Some(&meta), &mut sink, &clock);
        assert!(tracker.session_local().is_none());
        assert_eq!(sink.calls("end_session").len(), 1);
        assert_eq!(sink.laps().len(), 1, "the open lap was sent");
    }

    fn stall_tick(time: f64, lap: i32) -> Box<Tick> {
        let mut t = driving(time, 0.0313);
        t.lap = lap;
        t.lap_completed = 0;
        t.speed = 0.0;
        t.on_pit_road = true;
        t.in_pit_stall = Some(true);
        t.track_loc = 1;
        t
    }

    fn counter_rise_in_the_stall(late_rows: usize) {
        let (meta, mut sink, clock) = (meta(), RecordingSink::new(), FixedClock::default());
        let mut tracker = Tracker::new(64);
        let mut time = 0.0;
        // Out of the car (counters 0), then back in the stall; `Lap` goes to 1 `late_rows`
        // rows after driving starts.
        let mut out = stall_tick(time, 0);
        out.is_on_track = false;
        out.is_on_track_car = false;
        out.is_in_garage = true;
        tracker.step(&out, Some(&meta), &mut sink, &clock);
        for i in 0..90 {
            time += 1.0 / 60.0;
            let lap = if i >= late_rows { 1 } else { 0 };
            tracker.step(&stall_tick(time, lap), Some(&meta), &mut sink, &clock);
        }
        let started = tracker.view().lap_clock_s;
        assert!(
            started > 1.48,
            "the stall lap goes on: {started} (rise at row {late_rows})"
        );
        assert_eq!(tracker.view().lap_type, Some(LapType::Out));
        assert!(sink.laps().is_empty());
        // And leaving the stall onto the pit road does not end it either.
        for _ in 0..60 {
            time += 1.0 / 60.0;
            let mut t = stall_tick(time, 1);
            t.in_pit_stall = Some(false);
            t.track_loc = 2;
            t.speed = 5.0;
            tracker.step(&t, Some(&meta), &mut sink, &clock);
        }
        assert!(tracker.view().lap_clock_s > 2.48);
        assert!(sink.laps().is_empty());
    }

    #[test]
    fn tracker_a_counter_rise_one_row_late_in_the_stall_is_not_a_lap() {
        counter_rise_in_the_stall(1);
    }

    #[test]
    fn tracker_a_counter_rise_five_rows_late_in_the_stall_is_not_a_lap() {
        counter_rise_in_the_stall(5);
    }

    #[test]
    fn tracker_jitter_across_the_line_in_the_pit_lane_is_not_a_lap() {
        let (meta, mut sink, clock) = (meta(), RecordingSink::new(), FixedClock::default());
        let mut tracker = Tracker::new(64);
        // A lap starts at the line after a mid-track start (the first, invalid lap is sent).
        let (mut time, _) = drive_rows(&mut tracker, &meta, &mut sink, (0.0, 0.9), 560, -1.0);
        // Then the car creeps along the pit lane over the line, back and forth, counters still.
        for i in 0..240 {
            time += 1.0 / 60.0;
            let pct = if i % 2 == 0 { 0.9995 } else { 0.0005 };
            let mut t = driving(time, pct);
            t.lap = 4;
            t.lap_completed = 3;
            t.speed = 5.0;
            t.on_pit_road = true;
            t.track_loc = 2;
            tracker.step(&t, Some(&meta), &mut sink, &clock);
        }
        tracker.finish_open_lap(&mut sink);
        assert_eq!(sink.laps().len(), 1, "only the lap before the jitter");
    }

    #[test]
    fn tracker_an_interrupted_lap_is_not_labelled_in() {
        let (meta, mut sink) = (meta(), RecordingSink::new());
        let mut tracker = Tracker::new(64);
        drive_rows(&mut tracker, &meta, &mut sink, (0.0, 0.5), 600, -1.0);
        tracker.finish_open_lap(&mut sink);
        let lap = &sink.laps()[0];
        assert!(lap.partial);
        assert_eq!(lap.lap_type, LapType::Flying);
    }

    #[test]
    fn tracker_a_stop_that_keeps_the_session_open_drops_the_open_lap() {
        let (meta, mut sink, clock) = (meta(), RecordingSink::new(), FixedClock::default());
        let mut tracker = Tracker::new(64);
        drive_rows(&mut tracker, &meta, &mut sink, (0.0, 0.5), 600, -1.0);
        tracker.stop(true, &mut sink, &clock);
        assert!(
            sink.laps().is_empty(),
            "the resuming recorder closes that lap"
        );
        assert_eq!(tracker.session_local(), None);
        assert!(!sink.kinds().contains(&"end_session"));
    }
}
