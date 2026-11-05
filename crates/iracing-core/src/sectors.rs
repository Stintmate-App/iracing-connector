//! Sector times from the moments the car passes each sector start.

/// Most sectors a track can have here.
pub const MAX_SECTORS: usize = 16;

/// Largest difference, in seconds, between the sum of the sector times and the official lap
/// time before the lap's sectors are dropped.
pub const ABORTED_SECTOR_TOLERANCE_S: f32 = 0.15;

/// A forward step of the lap fraction larger than this is a wrap or a reset, not a crossing.
const MAX_STEP_PCT: f64 = 0.5;

/// The sector times of one finished lap, in a fixed array.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SectorTimes {
    count: usize,
    times: [Option<f32>; MAX_SECTORS],
}

impl SectorTimes {
    /// No sectors.
    pub const NONE: SectorTimes = SectorTimes {
        count: 0,
        times: [None; MAX_SECTORS],
    };

    /// Number of sectors of the track.
    pub fn len(&self) -> usize {
        self.count
    }

    /// Whether the track has no sectors.
    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// The time of each sector in order; `None` for a sector that was not timed.
    pub fn as_slice(&self) -> &[Option<f32>] {
        &self.times[..self.count]
    }
}

/// Records when the car passes each sector start during a lap.
#[derive(Clone, Debug)]
pub struct SectorClock {
    starts: [f64; MAX_SECTORS],
    count: usize,
    /// The session time at which the car passed each start (the first is the lap start).
    crossed_at: [Option<f64>; MAX_SECTORS],
    /// How many starts were passed this lap.
    passed: usize,
}

impl SectorClock {
    /// A clock for the sector starts (fractions of the lap, ascending, the first normally 0).
    /// A missing list or a single entry means the track has no sectors; entries beyond
    /// [`MAX_SECTORS`] are ignored.
    pub fn new(starts: &[f64]) -> SectorClock {
        let n = starts.len().min(MAX_SECTORS);
        let mut s = [0.0; MAX_SECTORS];
        s[..n].copy_from_slice(&starts[..n]);
        SectorClock {
            starts: s,
            count: if n >= 2 { n } else { 0 },
            crossed_at: [None; MAX_SECTORS],
            passed: 0,
        }
    }

    /// Number of sectors.
    pub fn sector_count(&self) -> usize {
        self.count
    }

    /// Begins a lap at session time `t`. `off_line` is true for a lap that did not start at the
    /// line (an out lap from the stall): its first sector stays untimed.
    pub fn start_lap(&mut self, t: f64, off_line: bool) {
        self.crossed_at = [None; MAX_SECTORS];
        self.passed = 0;
        if self.count > 0 {
            self.crossed_at[0] = if off_line { None } else { Some(t) };
            self.passed = 1;
        }
    }

    /// Index of the last sector start passed this lap, 0-based; `None` without sectors.
    pub fn current(&self) -> Option<u8> {
        (self.count > 0 && self.passed > 0).then(|| (self.passed - 1) as u8)
    }

    /// Feeds one step: the lap fraction and session time of the previous and the current row.
    /// Records the interpolated time of each start crossed, in order. Allocates nothing.
    pub fn step(&mut self, prev_pct: f64, pct: f64, prev_t: f64, t: f64) {
        let delta = pct - prev_pct;
        if self.count == 0 || delta <= 0.0 || delta > MAX_STEP_PCT {
            return;
        }
        while self.passed < self.count {
            let s = self.starts[self.passed];
            if prev_pct < s && s <= pct {
                self.crossed_at[self.passed] = Some(prev_t + (s - prev_pct) / delta * (t - prev_t));
                self.passed += 1;
            } else {
                break;
            }
        }
    }

    /// Ends the lap at session time `end_t` with the official lap time, and returns the sector
    /// times. A sector is untimed when either of its ends was not seen. When the times do not
    /// add up to the official time within [`ABORTED_SECTOR_TOLERANCE_S`], every sector is
    /// untimed.
    pub fn finish(&self, end_t: f64, official_s: Option<f32>) -> SectorTimes {
        if self.count == 0 {
            return SectorTimes::NONE;
        }
        let mut out = SectorTimes {
            count: self.count,
            times: [None; MAX_SECTORS],
        };
        let mut sum = 0.0_f32;
        let mut timed = 0;
        for i in 0..self.count {
            let end = if i + 1 < self.count {
                self.crossed_at[i + 1]
            } else {
                Some(end_t)
            };
            if let (Some(a), Some(b)) = (self.crossed_at[i], end) {
                let d = (b - a) as f32;
                out.times[i] = Some(d);
                sum += d;
                timed += 1;
            }
        }
        if let Some(official) = official_s {
            let all = timed == self.count;
            let off = if all {
                (sum - official).abs() > ABORTED_SECTOR_TOLERANCE_S
            } else {
                sum > official + ABORTED_SECTOR_TOLERANCE_S
            };
            if off {
                out.times = [None; MAX_SECTORS];
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const STARTS: [f64; 3] = [0.0, 0.3, 0.7];

    /// Drives a lap of `lap_s` seconds at constant speed from `t0`, in `dt` steps.
    fn drive(c: &mut SectorClock, t0: f64, lap_s: f64, dt: f64) -> f64 {
        let mut prev = 0.0;
        let mut t = t0;
        while t - t0 < lap_s - 1e-9 {
            let next_t = (t + dt).min(t0 + lap_s);
            let pct = (next_t - t0) / lap_s;
            c.step(prev, pct, t, next_t);
            prev = pct;
            t = next_t;
        }
        t
    }

    #[test]
    fn times_are_interpolated_and_add_up() {
        let mut c = SectorClock::new(&STARTS);
        c.start_lap(100.0, false);
        let end = drive(&mut c, 100.0, 90.0, 1.0 / 60.0 * 7.0);
        let t = c.finish(end, Some(90.0));
        let s = t.as_slice();
        assert_eq!(s.len(), 3);
        assert!((s[0].unwrap() - 27.0).abs() < 1e-3);
        assert!((s[1].unwrap() - 36.0).abs() < 1e-3);
        assert!((s[2].unwrap() - 27.0).abs() < 1e-3);
    }

    #[test]
    fn current_is_the_last_start_passed() {
        let mut c = SectorClock::new(&STARTS);
        assert_eq!(c.current(), None);
        c.start_lap(0.0, false);
        assert_eq!(c.current(), Some(0));
        c.step(0.29, 0.31, 0.0, 1.0);
        assert_eq!(c.current(), Some(1));
        c.step(0.69, 0.71, 2.0, 3.0);
        assert_eq!(c.current(), Some(2));
    }

    #[test]
    fn out_lap_leaves_the_first_sector_untimed() {
        let mut c = SectorClock::new(&STARTS);
        c.start_lap(50.0, true);
        let end = drive(&mut c, 50.0, 90.0, 0.05);
        let t = c.finish(end, None);
        assert_eq!(t.as_slice()[0], None);
        assert!(t.as_slice()[1].is_some() && t.as_slice()[2].is_some());
    }

    #[test]
    fn mismatch_with_the_official_time_drops_every_sector() {
        let mut c = SectorClock::new(&STARTS);
        c.start_lap(0.0, false);
        let end = drive(&mut c, 0.0, 90.0, 0.05);
        let t = c.finish(end, Some(90.4));
        assert!(t.as_slice().iter().all(Option::is_none));
        assert_eq!(t.len(), 3);
        let t = c.finish(end, Some(90.1));
        assert!(t.as_slice().iter().all(Option::is_some));
    }

    #[test]
    fn a_skipped_start_leaves_its_sectors_untimed() {
        let mut c = SectorClock::new(&STARTS);
        c.start_lap(0.0, false);
        // A tow past the second start: the fraction jumps, which is no crossing.
        c.step(0.2, 0.9, 10.0, 11.0);
        let t = c.finish(20.0, None);
        assert!(t.as_slice().iter().all(Option::is_none));
        assert_eq!(c.current(), Some(0));
    }

    #[test]
    fn no_sectors_without_two_starts() {
        assert_eq!(SectorClock::new(&[]).sector_count(), 0);
        assert_eq!(SectorClock::new(&[0.0]).sector_count(), 0);
        let mut c = SectorClock::new(&[0.0]);
        c.start_lap(0.0, false);
        assert_eq!(c.current(), None);
        assert!(c.finish(10.0, Some(10.0)).is_empty());
    }

    #[test]
    fn more_than_sixteen_starts_are_cut() {
        let starts: Vec<f64> = (0..20).map(|i| f64::from(i) / 20.0).collect();
        assert_eq!(SectorClock::new(&starts).sector_count(), MAX_SECTORS);
    }

    #[test]
    fn a_wrap_is_no_crossing() {
        let mut c = SectorClock::new(&STARTS);
        c.start_lap(0.0, false);
        c.step(0.99, 0.01, 1.0, 1.02);
        assert_eq!(c.current(), Some(0));
    }
}
