//! The spotter: calls a car alongside from the simulator's own proximity word.
//!
//! `CarLeftRight` says which sides have a car, so no geometry is needed here. [`Spotter`] is a
//! small state machine over it: a side that fills is called, both sides are "in the middle", a
//! side that stays empty for [`CLEAR_DELAY_S`] is cleared, a side that fills again soon after
//! it was cleared is called only if it still holds, and a called overlap is repeated every
//! [`STILL_THERE_S`] while it lasts.
//!
//! The gates: a practice, test, qualifying or race session; the car on the track and
//! not on the pit road; at least [`MIN_SPEED`]; in a race [`RACE_DELAY_S`] after the green; not
//! under a caution; and the simulator's own spotter on (`CarLeftRight` is not 0). A gate that
//! closes forgets what was called, silently.
//!
//! Not observed on the simulator yet: see docs/FINDINGS.md, Spotter.

use irsdk::defs::session_flags as flag;

use super::{Call, Calls};
use crate::codes::SessionKind;
use crate::meta::SessionMeta;
use crate::tick::Tick;

/// A side must stay empty this long before it is cleared (seconds).
pub const CLEAR_DELAY_S: f64 = 0.15;
/// A side that fills again within this long after it was cleared is called only if it still
/// holds when the time is up (seconds).
pub const BOUNCE_S: f64 = 1.5;
/// A called overlap is repeated this often while it holds (seconds).
pub const STILL_THERE_S: f64 = 3.0;
/// Below this speed nothing is called (m/s).
pub const MIN_SPEED: f32 = 10.0;
/// In a race nothing is called until this long after the green (seconds).
pub const RACE_DELAY_S: f64 = 20.0;

/// `PlayerTrackSurface` value of a car on the track.
const ON_TRACK: i32 = 3;
const LEFT: usize = 0;
const RIGHT: usize = 1;

/// The spotter. Allocate once; [`step`](Self::step) allocates nothing.
///
/// Not observed on the simulator yet: see docs/FINDINGS.md, Spotter.
#[derive(Clone, Debug)]
pub struct Spotter {
    told: [bool; 2],
    middle_said: bool,
    /// The last time each side was seen with a car.
    side_since: [f64; 2],
    /// The time each side was last cleared.
    cleared_at: [f64; 2],
    last_call_s: f64,
    still_variant: u8,
    open: bool,
}

impl Default for Spotter {
    fn default() -> Spotter {
        Spotter::new()
    }
}

impl Spotter {
    /// A spotter with nothing called.
    pub fn new() -> Spotter {
        Spotter {
            told: [false; 2],
            middle_said: false,
            side_since: [f64::NEG_INFINITY; 2],
            cleared_at: [f64::NEG_INFINITY; 2],
            last_call_s: f64::NEG_INFINITY,
            still_variant: 0,
            open: false,
        }
    }

    /// Forgets what was called, silently.
    pub fn reset(&mut self) {
        *self = Spotter::new();
    }

    /// Reads one row and adds the calls that fired to `out`. `race_green_s` is the session
    /// time the race went green, when it did. Hot: no allocation.
    pub fn step(
        &mut self,
        t: &Tick,
        meta: Option<&SessionMeta>,
        race_green_s: Option<f64>,
        out: &mut Calls,
    ) {
        let now = t.session_time;
        let Some(word) = Self::gate(t, meta, race_green_s, now) else {
            if self.open {
                self.reset();
            }
            return;
        };
        self.open = true;
        let has = [matches!(word, 2 | 4 | 5), matches!(word, 3 | 4 | 6)];
        for side in [LEFT, RIGHT] {
            if has[side] {
                self.side_since[side] = now;
            }
        }
        // A side counts once it has held past the bounce window of its last clear.
        let ready = [LEFT, RIGHT].map(|s| has[s] && now - self.cleared_at[s] >= BOUNCE_S);
        let cleared = [LEFT, RIGHT]
            .map(|s| self.told[s] && !has[s] && now - self.side_since[s] >= CLEAR_DELAY_S);

        if cleared[LEFT] || cleared[RIGHT] {
            self.clear(cleared, now, out);
            return;
        }
        let new = [LEFT, RIGHT].map(|s| ready[s] && !self.told[s]);
        if new[LEFT] || new[RIGHT] {
            self.rise(new, now, out);
            return;
        }
        if (self.told[LEFT] || self.told[RIGHT]) && now - self.last_call_s >= STILL_THERE_S {
            self.still(now, out);
        }
    }

    /// The `CarLeftRight` word when the gates are open.
    fn gate(
        t: &Tick,
        meta: Option<&SessionMeta>,
        race_green_s: Option<f64>,
        now: f64,
    ) -> Option<i32> {
        let meta = meta.filter(|_| !t.is_replay)?;
        let word = t.car_left_right.filter(|w| *w != 0)?;
        let kind_ok = match meta.kind {
            SessionKind::Practice | SessionKind::Test | SessionKind::Qualifying => true,
            SessionKind::Race => race_green_s.is_some_and(|g| now - g >= RACE_DELAY_S),
            SessionKind::Warmup | SessionKind::Other => false,
        };
        let under_caution = t.session_flags & (flag::CAUTION | flag::CAUTION_WAVING) != 0;
        (kind_ok
            && !under_caution
            && t.is_on_track
            && !t.on_pit_road
            && t.track_loc == ON_TRACK
            && t.speed >= MIN_SPEED)
            .then_some(word)
    }

    fn say(&mut self, now: f64, out: &mut Calls, call: Call) {
        self.last_call_s = now;
        out.push(call);
    }

    /// Sides that filled: a side alone, or both ("in the middle").
    fn rise(&mut self, new: [bool; 2], now: f64, out: &mut Calls) {
        // A side already called counts as present when the other one fills.
        let both = (new[LEFT] || self.told[LEFT]) && (new[RIGHT] || self.told[RIGHT]);
        for s in [LEFT, RIGHT] {
            if new[s] {
                self.told[s] = true;
            }
        }
        self.still_variant = 0;
        if both {
            self.middle_said = true;
            self.say(now, out, Call::new("spotter_in_the_middle"));
        } else if new[LEFT] {
            self.say(now, out, Call::new("spotter_car_left"));
        } else {
            self.say(now, out, Call::new("spotter_car_right"));
        }
    }

    /// Sides that stayed empty long enough.
    fn clear(&mut self, cleared: [bool; 2], now: f64, out: &mut Calls) {
        for s in [LEFT, RIGHT] {
            if cleared[s] {
                self.told[s] = false;
                self.cleared_at[s] = now;
            }
        }
        self.still_variant = 0;
        if self.middle_said {
            self.middle_said = false;
            if self.told[LEFT] {
                self.say(now, out, Call::new("spotter_car_left"));
            } else if self.told[RIGHT] {
                self.say(now, out, Call::new("spotter_car_right"));
            } else {
                self.say(now, out, Call::new("spotter_all_clear"));
            }
            return;
        }
        if cleared[LEFT] {
            self.say(now, out, Call::new("spotter_clear_left"));
        }
        if cleared[RIGHT] {
            self.say(now, out, Call::new("spotter_clear_right"));
        }
    }

    /// The called overlap still holds: the wordings rotate.
    fn still(&mut self, now: f64, out: &mut Calls) {
        let (code, wordings) = if self.middle_said {
            ("spotter_still_middle", 2)
        } else if self.told[LEFT] {
            ("spotter_still_left", 3)
        } else {
            ("spotter_still_right", 3)
        };
        let call = Call::new(code).variant(self.still_variant % wordings);
        self.still_variant = (self.still_variant + 1) % wordings;
        self.say(now, out, call);
    }
}

#[cfg(all(test, feature = "testing"))]
mod tests {
    use super::*;

    fn meta(kind: SessionKind) -> SessionMeta {
        let mut m = SessionMeta::empty(0);
        m.kind = kind;
        m
    }

    fn row(now: f64, word: i32) -> Box<Tick> {
        let mut t = Tick::new();
        t.session_time = now;
        t.car_left_right = Some(word);
        t.is_on_track = true;
        t.track_loc = ON_TRACK;
        t.speed = 40.0;
        t
    }

    /// A seeded generator, so the property test repeats.
    struct Rng(u64);

    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }
    }

    #[test]
    fn ten_random_minutes_stay_under_the_rate_and_every_clear_follows_its_car() {
        let m = meta(SessionKind::Practice);
        let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
        let mut spotter = Spotter::new();
        let mut calls = Calls::new();
        let mut fired: Vec<(f64, &'static str)> = Vec::new();
        let (mut word, mut left_in_row) = (1, 0);
        let mut called = [false; 2];
        let mut middle = false;
        for i in 0..(10 * 60 * 60) {
            if left_in_row == 0 {
                word = [1, 2, 3, 4, 5, 6, 1, 1][(rng.next() % 8) as usize];
                // From a single row to about four seconds.
                left_in_row = 1 + (rng.next() % 240) as i32;
            }
            left_in_row -= 1;
            let now = f64::from(i) / 60.0;
            calls.clear();
            spotter.step(&row(now, word), Some(&m), None, &mut calls);
            for c in calls.as_slice() {
                match c.code {
                    "spotter_car_left" => (called, middle) = ([true, false], false),
                    "spotter_car_right" => (called, middle) = ([false, true], false),
                    "spotter_in_the_middle" => (called, middle) = ([true; 2], true),
                    "spotter_clear_left" => {
                        assert!(called[LEFT] && !middle, "clear left at {now}");
                        called[LEFT] = false;
                    }
                    "spotter_clear_right" => {
                        assert!(called[RIGHT] && !middle, "clear right at {now}");
                        called[RIGHT] = false;
                    }
                    "spotter_all_clear" => {
                        assert!(middle, "all clear at {now}");
                        (called, middle) = ([false; 2], false);
                    }
                    _ => {}
                }
                fired.push((now, c.code));
            }
        }
        assert!(!fired.is_empty());
        let mut start = 0;
        for end in 0..fired.len() {
            while fired[end].0 - fired[start].0 >= 60.0 {
                start += 1;
            }
            assert!(end - start < 120, "{} calls within 60 s", end - start + 1);
        }
    }

    #[test]
    fn a_gate_that_closes_forgets_silently() {
        let m = meta(SessionKind::Practice);
        let mut s = Spotter::new();
        let mut calls = Calls::new();
        s.step(&row(0.0, 2), Some(&m), None, &mut calls);
        assert_eq!(calls.len(), 1);
        let mut slow = row(0.1, 2);
        slow.speed = 2.0;
        s.step(&slow, Some(&m), None, &mut calls);
        s.step(&row(5.0, 1), Some(&m), None, &mut calls);
        assert_eq!(calls.len(), 1, "no clear after the gate closed");
    }
}
