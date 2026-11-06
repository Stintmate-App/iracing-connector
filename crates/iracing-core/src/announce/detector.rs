//! The race announcements: the flags (the yellow family, the clear, the blue train, red, the
//! meatball), the full-course caution, the penalty flags, the finish, disqualifications and
//! retirements, and the player's incident count.
//!
//! [`Detector`] is fed every row (60 Hz) and writes the calls that fired into a [`Calls`]; on
//! most rows it compares a few integers and returns. It reads no clock (time is the row's
//! `SessionTime`), no file and no environment.
//!
//! 1. **Gate.** Only while a session is open, no replay plays and the player is in the car. For
//!    [`QUIET_S`] after a session opens, the session state changes into or out of racing, the
//!    session number changes or the detector is reset, values are followed silently.
//! 2. **Debounce.** A flag counts once it has held for [`FLAG_HOLD_S`].
//! 3. **Context.** Yellow, clear and blue are not announced on pit road; blue and the caution
//!    only in a race (the caution also on the parade lap).
//! 4. **Cooldowns** per code ([`cooldown`]); the blue flag per lapping car.
//! 5. **Rivals** are read from the per-car arrays at the rival rate only; another car's speed is
//!    its movement along the lap between two reads.
//! 6. **Once per session:** the final lap, the chequered flag, the player's disqualification and
//!    retirement, the incident limit warning, and each rival's disqualification and retirement.
//!    The session text ([`Detector::on_meta`]) tells who is out; its first reading of a session is
//!    the baseline and announces nothing.

use std::borrow::Cow;

use irsdk::defs::session_flags as flag;

use super::{Call, Calls, Gate, GateState};
use crate::codes::SessionKind;
use crate::meta::SessionMeta;
use crate::tick::Tick;
use crate::validity::IncidentTracker;
use crate::vars::CARS;

pub use super::QUIET_S;

/// Seconds a flag must hold before it counts.
pub const FLAG_HOLD_S: f64 = 0.5;
/// `flag_clear` after the yellow bits ended and this long held.
pub const CLEAR_HOLD_S: f64 = 3.0;
/// Another car is a candidate cause of a yellow below this speed (m/s).
pub const CAUSE_SLOW_MS: f32 = 6.0;
/// The cause is looked for this far ahead of the player along the lap.
pub const CAUSE_AHEAD_M: f64 = 1500.0;
/// The cause is looked for this far behind the player along the lap.
pub const CAUSE_BEHIND_M: f64 = 350.0;
/// The player counts as slow (m/s, on track) below this: a yellow then is about the player.
pub const SELF_SLOW_MS: f32 = 5.0;
/// A yellow edge within this many seconds of the player being slow or off is the player's own.
pub const SELF_WINDOW_S: f64 = 5.0;
/// The lapping car is looked for this far behind the player along the lap.
pub const BLUE_RANGE_M: f64 = 500.0;
/// A car of a faster class counts as lapping within this distance behind (multiclass).
pub const BLUE_CLASS_RANGE_M: f64 = 150.0;
/// A car is lapping when its race distance exceeds the player's by more than this many laps.
pub const BLUE_LAP_MARGIN: f64 = 0.5;
/// A blue train survives the flag being off this long.
pub const BLUE_TRAIN_GRACE_S: f64 = 20.0;
/// Two blue calls (of different cars) are at least this far apart.
pub const BLUE_NEXT_GAP_S: f64 = 4.0;
/// The same lapping car is not called again within this long.
pub const BLUE_CAR_COOLDOWN_S: f64 = 20.0;
/// Blue flags for the same lapping car per session.
pub const BLUE_MAX_PER_CAR: u8 = 3;
/// The chequered call waits this long after the player's crossing so the position it names is
/// the one the crossing gave: a position changes at the car's own crossing, and whether in the
/// same row as the lap counter is not observed.
pub const POSITION_SETTLE_S: f64 = 0.5;
/// The incident limit warning comes when the points left are at most the larger of this and a
/// quarter of the limit.
pub const LIMIT_WARNING_MIN_LEFT: i64 = 4;

/// The track surface value of a car on the track.
const ON_TRACK: i32 = 3;
/// The track surface value of a car off the track.
const OFF_TRACK: i32 = 0;

/// Minimum seconds between two calls of the same code (0: edge only).
pub fn cooldown(code: &str) -> f64 {
    match code {
        "flag_yellow"
        | "flag_double_yellow"
        | "flag_yellow_ahead"
        | "flag_double_yellow_ahead"
        | "flag_yellow_self"
        | "flag_yellow_cause"
        | "flag_debris"
        | "flag_red"
        | "flag_black_orange" => 30.0,
        "penalty_black_flag" => 30.0,
        "flag_clear" => 20.0,
        _ => 0.0,
    }
}

const COOLED: [&str; 11] = [
    "flag_yellow",
    "flag_double_yellow",
    "flag_yellow_ahead",
    "flag_double_yellow_ahead",
    "flag_yellow_self",
    "flag_yellow_cause",
    "flag_debris",
    "flag_red",
    "flag_black_orange",
    "penalty_black_flag",
    "flag_clear",
];

/// Calls made once per session.
#[derive(Clone, Copy)]
enum Once {
    FinalLap,
    Chequered,
    Disqualified,
    Retired,
    LimitWarning,
}

const ONCE: usize = 5;

/// No identity announced for a car slot.
const NO_ID: i64 = i64::MIN;

#[derive(Clone, Copy, Debug, PartialEq)]
enum Edge {
    None,
    Rise,
    Fall,
}

/// A debounced boolean.
#[derive(Clone, Copy, Debug, Default)]
struct Held {
    stable: bool,
    cand: bool,
    since: f64,
}

impl Held {
    fn feed(&mut self, v: bool, now: f64, hold: f64) -> Edge {
        if v != self.cand {
            self.cand = v;
            self.since = now;
        }
        if self.cand != self.stable && now - self.since >= hold {
            self.stable = self.cand;
            return if self.stable { Edge::Rise } else { Edge::Fall };
        }
        Edge::None
    }

    fn follow(&mut self, v: bool) {
        self.stable = v;
        self.cand = v;
    }
}

/// Wraps a lap fraction difference into -0.5..=0.5.
fn wrap(d: f64) -> f64 {
    d - d.round()
}

/// The flag announcements detector. Allocate once; [`step`](Self::step) allocates nothing
/// unless a call with a car's name fires.
///
/// Not observed on the simulator yet: see docs/FINDINGS.md, Incidents and flags.
#[derive(Clone, Debug)]
pub struct Detector {
    gate: Gate,
    scan_period_s: f64,
    // debounced flags
    yellow: Held,
    waving: Held,
    debris: Held,
    blue: Held,
    red: Held,
    repair: Held,
    caution: Held,
    green: Held,
    one_lap: Held,
    black: Held,
    disq: Held,
    white: Held,
    checkered: Held,
    pits_open: Option<bool>,
    // the player's own state
    self_slow_at: f64,
    self_off_at: f64,
    // the yellow
    yellow_level: u8,
    yellow_called: bool,
    self_called: bool,
    clear_since: Option<f64>,
    // the caution
    caution_seen: bool,
    await_green: bool,
    // the blue train
    train_live: bool,
    blue_off_at: f64,
    last_blue_call: f64,
    last_blue_car: Option<usize>,
    car_last_blue: [f64; CARS],
    car_blue_count: [u8; CARS],
    // penalties and the finish
    black_pit: bool,
    prev_lap: Option<i32>,
    // The white flag was shown and the player has not crossed the line since.
    white_armed: bool,
    // The chequered flag was shown and the player has not crossed the line since.
    check_armed: bool,
    // Session time at which the chequered call is made, once the player crossed.
    check_at: Option<f64>,
    // the incident count
    incidents: IncidentTracker,
    // once per session
    once: [bool; ONCE],
    ident_session: i32,
    rival_dq_id: [i64; CARS],
    rival_out_id: [i64; CARS],
    // who is out, from the session text
    out_seeded: bool,
    out_base: [i32; CARS],
    on_air: bool,
    last_now: f64,
    // cooldowns
    cooled_at: [f64; COOLED.len()],
    // other cars' speeds
    next_scan_at: f64,
    next_blue_scan_at: f64,
    prev_scan_at: f64,
    prev_pct: [f32; CARS],
    prev_on: [bool; CARS],
    car_speed: [f32; CARS],
}

impl Detector {
    /// A detector that reads the other cars `rival_hz` times a second.
    pub fn new(rival_hz: f64) -> Detector {
        let scan_period_s = if rival_hz > 0.0 { 1.0 / rival_hz } else { 0.5 };
        let mut d = Detector {
            gate: Gate::new(),
            scan_period_s,
            yellow: Held::default(),
            waving: Held::default(),
            debris: Held::default(),
            blue: Held::default(),
            red: Held::default(),
            repair: Held::default(),
            caution: Held::default(),
            green: Held::default(),
            one_lap: Held::default(),
            black: Held::default(),
            disq: Held::default(),
            white: Held::default(),
            checkered: Held::default(),
            pits_open: None,
            self_slow_at: f64::NEG_INFINITY,
            self_off_at: f64::NEG_INFINITY,
            yellow_level: 0,
            yellow_called: false,
            self_called: false,
            clear_since: None,
            caution_seen: false,
            await_green: false,
            train_live: false,
            blue_off_at: f64::NEG_INFINITY,
            last_blue_call: f64::NEG_INFINITY,
            last_blue_car: None,
            car_last_blue: [f64::NEG_INFINITY; CARS],
            car_blue_count: [0; CARS],
            black_pit: false,
            prev_lap: None,
            white_armed: false,
            check_armed: false,
            check_at: None,
            incidents: IncidentTracker::new(),
            once: [false; ONCE],
            ident_session: i32::MIN,
            rival_dq_id: [NO_ID; CARS],
            rival_out_id: [NO_ID; CARS],
            out_seeded: false,
            out_base: [0; CARS],
            on_air: false,
            last_now: 0.0,
            cooled_at: [f64::NEG_INFINITY; COOLED.len()],
            next_scan_at: f64::NEG_INFINITY,
            next_blue_scan_at: f64::NEG_INFINITY,
            prev_scan_at: f64::NEG_INFINITY,
            prev_pct: [0.0; CARS],
            prev_on: [false; CARS],
            car_speed: [f32::NAN; CARS],
        };
        d.clear_state();
        d
    }

    /// Forgets the session: the next row starts a quiet time.
    pub fn reset(&mut self) {
        self.gate.reset();
        self.clear_state();
        self.new_session(i32::MIN);
        self.on_air = false;
    }

    /// The once-per-session state, for session `num`.
    fn new_session(&mut self, num: i32) {
        self.ident_session = num;
        self.once = [false; ONCE];
        self.rival_dq_id = [NO_ID; CARS];
        self.rival_out_id = [NO_ID; CARS];
        self.out_seeded = false;
        self.prev_lap = None;
    }

    fn done(&mut self, which: Once) -> bool {
        std::mem::replace(&mut self.once[which as usize], true)
    }

    /// The per-session state, silently.
    fn clear_state(&mut self) {
        self.self_slow_at = f64::NEG_INFINITY;
        self.self_off_at = f64::NEG_INFINITY;
        self.yellow_level = 0;
        self.yellow_called = false;
        self.self_called = false;
        self.clear_since = None;
        self.caution_seen = false;
        self.await_green = false;
        self.train_live = false;
        self.blue_off_at = f64::NEG_INFINITY;
        self.last_blue_call = f64::NEG_INFINITY;
        self.last_blue_car = None;
        self.car_last_blue = [f64::NEG_INFINITY; CARS];
        self.car_blue_count = [0; CARS];
        self.cooled_at = [f64::NEG_INFINITY; COOLED.len()];
        self.next_scan_at = f64::NEG_INFINITY;
        self.next_blue_scan_at = f64::NEG_INFINITY;
        self.prev_scan_at = f64::NEG_INFINITY;
        self.prev_on = [false; CARS];
        self.car_speed = [f32::NAN; CARS];
        self.pits_open = None;
        self.black_pit = false;
        self.white_armed = false;
        self.check_armed = false;
        self.check_at = None;
        self.incidents.reset();
    }

    /// Reads one row and adds the calls that fired to `out`. Hot: no allocation while nothing
    /// fires.
    pub fn step(&mut self, t: &Tick, meta: Option<&SessionMeta>, out: &mut Calls) {
        let g = self.gate.step(t, meta);
        let (Some(kind), Some(meta)) = (g.kind, meta) else {
            self.on_air = false;
            return;
        };
        let now = t.session_time;
        let f = t.session_flags;
        if g.restarted {
            self.clear_state();
        }
        if meta.session_num != self.ident_session {
            self.new_session(meta.session_num);
        }
        let live = !g.quiet;
        self.on_air = live && g.in_car;
        self.last_now = now;
        let race = kind == SessionKind::Race;

        // The player's own state, for a yellow that is about the player.
        if g.driving && !g.pit_road {
            if t.track_loc == ON_TRACK && t.speed < SELF_SLOW_MS {
                self.self_slow_at = now;
            }
            if t.track_loc == OFF_TRACK {
                self.self_off_at = now;
            }
        }
        if now >= self.next_scan_at {
            self.scan_speeds(t, meta, now);
            self.rival_dq_scan(t, meta, race && self.on_air, now, out);
        }

        let e_yellow = feed(&mut self.yellow, f & flag::YELLOW != 0, now, live);
        let e_wave = feed(&mut self.waving, f & flag::YELLOW_WAVING != 0, now, live);
        let e_debris = feed(&mut self.debris, f & flag::DEBRIS != 0, now, live);
        let e_blue = feed(&mut self.blue, f & flag::BLUE != 0, now, live);
        let e_red = feed(&mut self.red, f & flag::RED != 0, now, live);
        let e_repair = feed(&mut self.repair, f & flag::REPAIR != 0, now, live);
        let e_caution = feed(
            &mut self.caution,
            f & (flag::CAUTION | flag::CAUTION_WAVING) != 0,
            now,
            live,
        );
        let _ = feed(&mut self.green, f & flag::GREEN != 0, now, live);
        let e_one_lap = feed(
            &mut self.one_lap,
            f & flag::ONE_LAP_TO_GREEN != 0,
            now,
            live,
        );

        let e_black = feed(&mut self.black, f & flag::BLACK != 0, now, live);
        let e_dq = feed(&mut self.disq, f & flag::DISQUALIFY != 0, now, live);
        let e_white = feed(&mut self.white, f & flag::WHITE != 0, now, live);
        let e_check = feed(&mut self.checkered, f & flag::CHECKERED != 0, now, live);
        self.race_step(
            t,
            meta,
            race && self.on_air,
            [e_black, e_dq, e_white, e_check],
            now,
            out,
        );
        self.incident_step(t, meta, self.on_air, now, out);

        if live {
            if e_red == Edge::Rise && g.in_car {
                self.fire(now, out, Call::new("flag_red"));
            }
            if e_repair == Edge::Rise && g.in_car {
                self.fire(now, out, Call::new("flag_black_orange"));
            }
        }
        if self.caution.stable && race {
            self.caution_seen = true;
        }
        self.caution_step(t, &g, race, e_caution, e_one_lap, now, out);
        let caution_active = self.caution.stable;
        if live && g.driving && !g.pit_road && !caution_active {
            self.yellow_step(t, meta, e_yellow, e_wave, now, out);
            if e_debris == Edge::Rise {
                // Observed: the debris flag is a surface flag with no yellow bit. "Yellow for
                // debris" is said only while a yellow is out.
                let call = if self.yellow.stable || self.waving.stable {
                    Call::new("flag_yellow_cause").text("cause", "debris")
                } else {
                    Call::new("flag_debris")
                };
                self.fire(now, out, call);
            }
            self.clear_step(now, out);
        }
        self.blue_step(t, meta, &g, race, e_blue, now, out);
    }

    /// The penalty flags, the finish and the player's disqualification.
    fn race_step(
        &mut self,
        t: &Tick,
        meta: &SessionMeta,
        on_air: bool,
        edges: [Edge; 4],
        now: f64,
        out: &mut Calls,
    ) {
        let [e_black, e_dq, e_white, e_check] = edges;
        let crossed = self.prev_lap.is_some_and(|p| t.lap > p);
        self.prev_lap = Some(t.lap);
        if e_black == Edge::Rise {
            self.black_pit = t.on_pit_road;
        } else if self.black.stable && t.on_pit_road {
            self.black_pit = true;
        }
        if !on_air {
            if e_black == Edge::Fall {
                self.black_pit = false;
            }
            return;
        }
        if e_dq == Edge::Rise && !self.done(Once::Disqualified) {
            self.fire(now, out, Call::new("disqualified"));
        }
        // The black flag of a disqualification is not a penalty to serve.
        if e_black == Edge::Rise && !self.disq.stable {
            self.fire(now, out, Call::new("penalty_black_flag"));
        }
        if e_black == Edge::Fall && std::mem::take(&mut self.black_pit) {
            self.fire(now, out, Call::new("penalty_served"));
        }
        // Observed: the white and chequered bits rise about 12 s before the player's crossing, so
        // both calls wait for it (docs/FINDINGS.md, Race end and rivals).
        if e_white == Edge::Rise {
            self.white_armed = true;
        }
        if e_check == Edge::Rise {
            self.check_armed = true;
        }
        let leader_last_lap = crossed && t.laps_remain == Some(1) && t.position == Some(1);
        if ((crossed && self.white_armed) || leader_last_lap) && !self.done(Once::FinalLap) {
            self.white_armed = false;
            self.fire(now, out, Call::new("final_lap"));
        }
        if crossed && self.check_armed && self.check_at.is_none() {
            self.check_at = Some(now + POSITION_SETTLE_S);
        }
        if self.check_at.is_some_and(|at| now >= at) && !self.done(Once::Chequered) {
            self.check_armed = false;
            self.check_at = None;
            let place = if meta.multiclass {
                t.class_position
            } else {
                t.position
            };
            let call = match place.filter(|p| *p > 0) {
                Some(p) => Call::new("chequered").int("position", i64::from(p)),
                None => Call::new("chequered_no_position"),
            };
            self.fire(now, out, call);
        }
    }

    /// The player's incident count: told once the rises of two seconds are merged.
    fn incident_step(
        &mut self,
        t: &Tick,
        meta: &SessionMeta,
        on_air: bool,
        now: f64,
        out: &mut Calls,
    ) {
        // The cause of a rise is `IncidentTracker`'s; the announcement does not name it.
        let _ = self.incidents.step(t);
        let Some(points) = self.incidents.take_settled(now) else {
            return;
        };
        if !on_air {
            return;
        }
        let total = i64::from(self.incidents.points_session);
        self.fire(
            now,
            out,
            Call::new("incident")
                .int("points", i64::from(points))
                .int("total", total),
        );
        if let Some(limit) = meta.incident_limit.filter(|l| *l > 0) {
            let limit = i64::from(limit);
            if limit - total <= LIMIT_WARNING_MIN_LEFT.max(limit / 4)
                && !self.done(Once::LimitWarning)
            {
                self.fire(
                    now,
                    out,
                    Call::new("incident_limit_warning")
                        .int("total", total)
                        .int("limit", limit),
                );
            }
        }
    }

    /// Rivals shown the disqualification flag, by identity, at the rival rate. While nothing
    /// may be announced the flag is only noted.
    fn rival_dq_scan(
        &mut self,
        t: &Tick,
        meta: &SessionMeta,
        on_air: bool,
        now: f64,
        out: &mut Calls,
    ) {
        let Some(flags) = t.car_flags.as_ref() else {
            return;
        };
        let me = usize::try_from(t.player_idx).ok();
        for (i, car_flags) in flags.iter().enumerate() {
            if Some(i) == me || *car_flags & flag::DISQUALIFY == 0 {
                continue;
            }
            let Some(entry) = meta.roster[i].as_ref().filter(|d| d.is_real) else {
                continue;
            };
            if self.rival_dq_id[i] == entry.user_id {
                continue;
            }
            self.rival_dq_id[i] = entry.user_id;
            if on_air {
                let call = Call::new("rival_disqualified").text("driver", name_of(meta, i));
                self.fire(now, out, call);
            }
        }
    }

    /// A new session text: who is out of the race. The first text of a session is the baseline
    /// and says nothing; later ones announce the player's retirement and a rival's, once per
    /// identity, unless the rival was disqualified.
    ///
    /// Not observed on the simulator yet: see docs/FINDINGS.md, Race end and rivals.
    pub fn on_meta(&mut self, meta: &SessionMeta, out: &mut Calls) {
        if meta.session_num != self.ident_session {
            self.new_session(meta.session_num);
        }
        let announce = self.out_seeded && self.on_air && meta.kind == SessionKind::Race;
        let me = usize::try_from(meta.player_idx).ok();
        let now = self.last_now;
        for i in 0..CARS {
            let reason = meta.results[i].reason_out_id;
            let was = std::mem::replace(&mut self.out_base[i], reason);
            if !(announce && reason != 0 && was == 0) {
                continue;
            }
            if Some(i) == me {
                if !self.once[Once::Disqualified as usize] && !self.done(Once::Retired) {
                    self.fire(now, out, Call::new("retired"));
                }
                continue;
            }
            let Some(entry) = meta.roster[i].as_ref().filter(|d| d.is_real) else {
                continue;
            };
            if self.rival_dq_id[i] == entry.user_id || self.rival_out_id[i] == entry.user_id {
                continue;
            }
            self.rival_out_id[i] = entry.user_id;
            let call = Call::new("rival_out").text("driver", name_of(meta, i));
            self.fire(now, out, call);
        }
        self.out_seeded = true;
    }

    fn fire(&mut self, now: f64, out: &mut Calls, call: Call) -> bool {
        let slot = COOLED.iter().position(|c| *c == call.code);
        if let Some(i) = slot {
            if now - self.cooled_at[i] < cooldown(call.code) {
                return false;
            }
            self.cooled_at[i] = now;
        }
        out.push(call)
    }

    #[allow(clippy::too_many_arguments)]
    fn caution_step(
        &mut self,
        t: &Tick,
        g: &GateState,
        race: bool,
        e_caution: Edge,
        e_one_lap: Edge,
        now: f64,
        out: &mut Calls,
    ) {
        let live = !g.quiet && race && g.racing && g.in_car;
        if e_caution == Edge::Rise && live {
            self.fire(now, out, Call::new("fcy_start"));
            self.await_green = true;
            // The yellow bits of a caution are not a local yellow.
            self.yellow_called = false;
            self.yellow_level = 0;
            self.self_called = false;
            self.clear_since = None;
        }
        if e_one_lap == Edge::Rise && live && self.caution_seen {
            self.fire(now, out, Call::new("fcy_last_lap"));
        }
        let open = t.pits_open;
        if let (Some(false), Some(true)) = (self.pits_open, open)
            && self.caution.stable
            && live
        {
            self.fire(now, out, Call::new("fcy_pits_open"));
        }
        self.pits_open = open;
        if self.await_green && !self.caution.stable && self.green.stable && live {
            self.fire(now, out, Call::new("fcy_restart"));
            self.await_green = false;
            self.caution_seen = false;
        }
    }

    fn yellow_step(
        &mut self,
        t: &Tick,
        meta: &SessionMeta,
        e_yellow: Edge,
        e_wave: Edge,
        now: f64,
        out: &mut Calls,
    ) {
        if e_yellow != Edge::Rise && e_wave != Edge::Rise {
            return;
        }
        let level = if self.waving.stable { 2 } else { 1 };
        if level <= self.yellow_level {
            return;
        }
        self.yellow_level = level;
        self.yellow_called = true;
        self.clear_since = None;
        if self.self_called {
            return;
        }
        let situation = if now - self.self_slow_at <= SELF_WINDOW_S {
            Some("stopped")
        } else if now - self.self_off_at <= SELF_WINDOW_S {
            Some("off_track")
        } else {
            None
        };
        if let Some(situation) = situation {
            self.self_called = true;
            self.fire(
                now,
                out,
                Call::new("flag_yellow_self").text("situation", situation),
            );
            return;
        }
        let sector = self.slow_car_sector(t, meta);
        let call = match (level, sector) {
            (1, Some(s)) => Call::new("flag_yellow").int("sector", s),
            (1, None) => Call::new("flag_yellow_ahead"),
            (_, Some(s)) => Call::new("flag_double_yellow").int("sector", s),
            (_, None) => Call::new("flag_double_yellow_ahead"),
        };
        self.fire(now, out, call);
    }

    fn clear_step(&mut self, now: f64, out: &mut Calls) {
        let bits_off = !self.yellow.stable && !self.waving.stable;
        if !(self.yellow_called && bits_off) {
            self.clear_since = None;
            return;
        }
        let since = *self.clear_since.get_or_insert(now);
        if now - since >= CLEAR_HOLD_S {
            if !self.self_called {
                self.fire(now, out, Call::new("flag_clear"));
            }
            self.yellow_called = false;
            self.yellow_level = 0;
            self.self_called = false;
            self.clear_since = None;
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn blue_step(
        &mut self,
        t: &Tick,
        meta: &SessionMeta,
        g: &GateState,
        race: bool,
        e_blue: Edge,
        now: f64,
        out: &mut Calls,
    ) {
        if e_blue == Edge::Fall {
            self.blue_off_at = now;
        }
        if !(race && !g.quiet && g.driving && !g.pit_road) {
            return;
        }
        if e_blue == Edge::Rise {
            let new_train = !self.train_live || now - self.blue_off_at > BLUE_TRAIN_GRACE_S;
            let car = self.lapping_car(t, meta);
            if new_train {
                self.train_live = true;
                self.last_blue_car = None;
                match car {
                    Some(i) if self.car_ok(i, now) => {
                        let call = Call::new("flag_blue").text("driver", name_of(meta, i));
                        self.fire(now, out, call);
                        self.mark(i, now);
                    }
                    Some(_) => {}
                    None => {
                        self.fire(now, out, Call::new("flag_blue_unknown"));
                    }
                }
            } else {
                self.next_blue(car, meta, now, out);
            }
            self.next_blue_scan_at = now + self.scan_period_s;
        } else if self.blue.stable && self.train_live && now >= self.next_blue_scan_at {
            self.next_blue_scan_at = now + self.scan_period_s;
            let car = self.lapping_car(t, meta);
            self.next_blue(car, meta, now, out);
        }
    }

    fn next_blue(&mut self, car: Option<usize>, meta: &SessionMeta, now: f64, out: &mut Calls) {
        let Some(i) = car else { return };
        if Some(i) != self.last_blue_car
            && self.car_ok(i, now)
            && now - self.last_blue_call >= BLUE_NEXT_GAP_S
        {
            let call = Call::new("flag_blue_next").text("driver", name_of(meta, i));
            self.fire(now, out, call);
            self.mark(i, now);
        }
    }

    fn car_ok(&self, i: usize, now: f64) -> bool {
        now - self.car_last_blue[i] >= BLUE_CAR_COOLDOWN_S
            && self.car_blue_count[i] < BLUE_MAX_PER_CAR
    }

    fn mark(&mut self, i: usize, now: f64) {
        self.car_last_blue[i] = now;
        self.car_blue_count[i] += 1;
        self.last_blue_car = Some(i);
        self.last_blue_call = now;
    }

    /// Updates the other cars' speeds from their movement since the last read.
    fn scan_speeds(&mut self, t: &Tick, meta: &SessionMeta, now: f64) {
        self.next_scan_at = now + self.scan_period_s;
        let dt = now - self.prev_scan_at;
        let length = track_length(meta);
        for i in 0..CARS {
            let on = t.car_loc[i] == ON_TRACK;
            let pct = t.car_pct[i];
            self.car_speed[i] = match length {
                Some(len) if on && self.prev_on[i] && dt > 0.0 => {
                    let d = wrap(f64::from(pct) - f64::from(self.prev_pct[i])) * len;
                    (d.abs() / dt) as f32
                }
                _ => f32::NAN,
            };
            self.prev_on[i] = on;
            self.prev_pct[i] = pct;
        }
        self.prev_scan_at = now;
    }

    /// The sector (1-based) of the slow car most likely to cause a yellow, when one is near.
    fn slow_car_sector(&self, t: &Tick, meta: &SessionMeta) -> Option<i64> {
        let len = track_length(meta)?;
        if meta.sector_starts.is_empty() {
            return None;
        }
        let me = usize::try_from(t.player_idx).ok();
        let mut best: Option<(f64, f64)> = None;
        for i in 0..CARS {
            if Some(i) == me
                || i32::try_from(i).ok() == Some(meta.pace_car_idx)
                || t.car_loc[i] != ON_TRACK
                || t.car_on_pit_road[i]
                || self.car_speed[i].is_nan()
                || self.car_speed[i] >= CAUSE_SLOW_MS
            {
                continue;
            }
            let pct = f64::from(t.car_pct[i]);
            let d = wrap(pct - f64::from(t.lap_dist_pct)) * len;
            if !(-CAUSE_BEHIND_M..=CAUSE_AHEAD_M).contains(&d) {
                continue;
            }
            if best.is_none_or(|(bd, _)| d.abs() < bd) {
                best = Some((d.abs(), pct));
            }
        }
        let (_, pct) = best?;
        let n = meta.sector_starts.iter().filter(|s| **s <= pct).count();
        Some(n.max(1) as i64)
    }

    /// The car that should be let through: the nearest real car behind a lap or more ahead in
    /// race distance, else (multiclass) the nearest faster-class car just behind.
    fn lapping_car(&self, t: &Tick, meta: &SessionMeta) -> Option<usize> {
        let len = track_length(meta)?;
        let me = usize::try_from(t.player_idx).ok();
        let mine = f64::from(t.lap) + f64::from(t.lap_dist_pct);
        let my_class = me
            .and_then(|m| meta.roster.get(m))
            .and_then(Option::as_ref)
            .and_then(|d| d.class_est_lap_s);
        let mut lapping: Option<(f64, usize)> = None;
        let mut faster: Option<(f64, usize)> = None;
        for i in 0..CARS {
            let Some(entry) = meta.roster[i].as_ref().filter(|d| d.is_real) else {
                continue;
            };
            if Some(i) == me || t.car_loc[i] != ON_TRACK || t.car_on_pit_road[i] {
                continue;
            }
            let behind = -wrap(f64::from(t.car_pct[i]) - f64::from(t.lap_dist_pct)) * len;
            if behind <= 0.0 {
                continue;
            }
            let theirs = f64::from(t.car_lap[i]) + f64::from(t.car_pct[i]);
            if behind <= BLUE_RANGE_M
                && theirs > mine + BLUE_LAP_MARGIN
                && lapping.is_none_or(|(b, _)| behind < b)
            {
                lapping = Some((behind, i));
            }
            if meta.multiclass
                && behind <= BLUE_CLASS_RANGE_M
                && matches!((entry.class_est_lap_s, my_class), (Some(c), Some(m)) if c < m)
                && faster.is_none_or(|(b, _)| behind < b)
            {
                faster = Some((behind, i));
            }
        }
        lapping.or(faster).map(|(_, i)| i)
    }
}

fn feed(h: &mut Held, v: bool, now: f64, live: bool) -> Edge {
    if live {
        h.feed(v, now, FLAG_HOLD_S)
    } else {
        h.follow(v);
        Edge::None
    }
}

fn track_length(meta: &SessionMeta) -> Option<f64> {
    meta.track.length_m.filter(|l| *l > 0.0)
}

fn name_of(meta: &SessionMeta, i: usize) -> Cow<'static, str> {
    Cow::Owned(
        meta.roster[i]
            .as_ref()
            .map(|d| d.name.clone())
            .unwrap_or_default(),
    )
}
