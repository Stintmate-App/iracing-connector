//! The calls of a countdown: the tow time and the pit repair times fall by a second per second
//! to 0, and the driver is told at each whole minute below the starting value, then at 30 s, at
//! 10 s and at 0.

/// One step of a countdown that is called out.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Step {
    /// A whole number of minutes (2 or more) is left.
    Minutes(i64),
    /// A number of seconds (60, 30 or 10) is left.
    Seconds(i64),
    /// The value reached 0.
    Done,
}

/// A value that rises to a starting value and counts down to 0. [`feed`](Self::feed) it every
/// row: it compares a few floats and allocates nothing.
#[derive(Clone, Copy, Debug)]
pub struct Schedule {
    /// The previous value; 0 or less while no countdown runs.
    last: f32,
    /// The next threshold to call, in seconds; 0 when only the end is left.
    next: f32,
}

/// A rise of more than this between two rows (seconds) is a new value, not the count: a
/// caution changing a tow in progress, or a new repair.
const JUMP_S: f32 = 1.0;

/// The largest threshold (a whole minute, 30 or 10) strictly below `v`, or 0.
fn threshold_below(v: f32) -> f32 {
    if v > 60.0 {
        60.0 * ((v / 60.0).ceil() - 1.0)
    } else if v > 30.0 {
        30.0
    } else if v > 10.0 {
        10.0
    } else {
        0.0
    }
}

impl Default for Schedule {
    fn default() -> Schedule {
        Schedule::new()
    }
}

impl Schedule {
    /// No countdown running.
    pub fn new() -> Schedule {
        Schedule {
            last: 0.0,
            next: 0.0,
        }
    }

    /// Forgets the countdown, silently.
    pub fn reset(&mut self) {
        *self = Schedule::new();
    }

    /// Whether a countdown is running.
    pub fn running(&self) -> bool {
        self.last > 0.0
    }

    /// Reads one row's value (`None`: the simulator does not publish it) and returns the step
    /// that was reached, if any. A countdown that starts is followed from its value: steps above
    /// it are not called. A value that jumps up starts a new schedule.
    pub fn feed(&mut self, v: Option<f32>) -> Option<Step> {
        let v = v?;
        if !v.is_finite() {
            return None;
        }
        if v <= 0.0 {
            let done = self.last > 0.0;
            self.reset();
            return done.then_some(Step::Done);
        }
        if self.last <= 0.0 || v > self.last + JUMP_S {
            self.next = threshold_below(v);
            self.last = v;
            return None;
        }
        self.last = v;
        if self.next > 0.0 && v <= self.next {
            let at = self.next;
            self.next = threshold_below(v.min(at));
            return Some(if at >= 120.0 {
                Step::Minutes((at / 60.0).round() as i64)
            } else {
                Step::Seconds(at.round() as i64)
            });
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Feeds `from` falling at 60 Hz for `seconds` (to 0 if it gets there); the steps with the
    /// value they were called at.
    fn run(s: &mut Schedule, from: f32, seconds: f32) -> Vec<(Step, f32)> {
        let mut out = Vec::new();
        let mut v = from;
        for _ in 0..(seconds * 60.0) as u32 {
            if let Some(step) = s.feed(Some(v)) {
                out.push((step, v));
            }
            v = (v - 1.0 / 60.0).max(0.0);
        }
        out
    }

    fn steps(r: &[(Step, f32)]) -> Vec<Step> {
        r.iter().map(|(s, _)| *s).collect()
    }

    #[test]
    fn a_countdown_calls_the_minutes_then_30_10_and_the_end() {
        let mut s = Schedule::new();
        let r = run(&mut s, 141.6, 150.0);
        assert_eq!(
            steps(&r),
            [
                Step::Minutes(2),
                Step::Seconds(60),
                Step::Seconds(30),
                Step::Seconds(10),
                Step::Done
            ]
        );
        // Each step is called on the row it is reached.
        for ((_, at), want) in r.iter().zip([120.0, 60.0, 30.0, 10.0]) {
            assert!((at - want).abs() < 0.02, "{at} vs {want}");
        }
    }

    #[test]
    fn a_countdown_that_starts_late_skips_the_steps_already_passed() {
        let mut s = Schedule::new();
        assert_eq!(
            steps(&run(&mut s, 25.0, 30.0)),
            [Step::Seconds(10), Step::Done]
        );
        let mut s = Schedule::new();
        assert_eq!(steps(&run(&mut s, 5.0, 10.0)), [Step::Done]);
    }

    #[test]
    fn an_exact_minute_is_not_called_at_the_start() {
        let mut s = Schedule::new();
        let r = run(&mut s, 120.0, 130.0);
        assert_eq!(
            steps(&r),
            [
                Step::Seconds(60),
                Step::Seconds(30),
                Step::Seconds(10),
                Step::Done
            ]
        );
    }

    #[test]
    fn a_value_that_jumps_up_restarts_the_schedule() {
        let mut s = Schedule::new();
        let first = run(&mut s, 100.0, 50.0);
        assert_eq!(steps(&first), [Step::Seconds(60)]);
        // A caution adds time: 50 s left becomes 200 s.
        let again = run(&mut s, 200.0, 201.0);
        assert_eq!(
            steps(&again),
            [
                Step::Minutes(3),
                Step::Minutes(2),
                Step::Seconds(60),
                Step::Seconds(30),
                Step::Seconds(10),
                Step::Done
            ]
        );
    }

    #[test]
    fn nothing_is_called_without_a_value_or_a_countdown() {
        let mut s = Schedule::new();
        assert_eq!(s.feed(None), None);
        assert_eq!(s.feed(Some(0.0)), None);
        assert!(!s.running());
        s.feed(Some(50.0));
        assert!(s.running());
        assert_eq!(s.feed(None), None);
        assert!(s.running());
    }
}
