//! The channels of one lap, one column of `f32` per channel, filled row by row.

use irsdk::defs::surface_class;

use crate::codes::surface_code;
use crate::position::{DeadReckoner, PosEnd, finish};
use crate::tick::Tick;

/// Rows reserved for each column when a buffer is made: 2 min 16 s at 60 Hz. A longer lap grows
/// the columns by doubling.
pub const RESERVE_ROWS: usize = 8192;

/// Standard gravity, in metres per second squared.
const G: f32 = 9.80665;

/// Steering lock to lock, in radians, assumed when the simulator reports none (450 degrees).
const DEFAULT_STEER_LOCK_RAD: f32 = 7.854;

/// What the buffer needs to know about the lap in progress, besides the row.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LapPoint {
    /// Whether the lap is timed. An untimed lap (the formation, a lap that has not crossed the
    /// line) records `-1` as its time.
    pub timed: bool,
    /// Seconds since the lap started; read only when `timed`.
    pub lap_clock_s: f32,
    /// Whether the lap is still valid.
    pub valid: bool,
}

/// Which optional channels were present on every row of the lap.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Present {
    /// The clutch.
    pub clutch: bool,
    /// The vertical acceleration.
    pub g_vert: bool,
    /// The brake lock-up flag (`abs`).
    pub abs: bool,
    /// The surface code under the car.
    pub surface: bool,
    /// The session's incident count.
    pub incidents: bool,
    /// The position (`x` and `z`): every row could be integrated and the lap has an anchor.
    pub position: bool,
}

impl Present {
    const ALL: Present = Present {
        clutch: true,
        g_vert: true,
        abs: true,
        surface: true,
        incidents: true,
        position: true,
    };
}

/// The columns of a lap. Every column has the same length.
///
/// Units are Pitwall's: speed in km/h, steering and pedals as fractions, accelerations in g.
/// Optional columns hold `0` on a row that lacks the variable and are meaningful only when the
/// matching flag of [`LapBuffer::present`] is set.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Columns {
    /// Seconds since the lap start, `-1` while the lap is not timed.
    pub t: Vec<f32>,
    /// Distance along the lap in metres, clamped to the track length.
    pub dist: Vec<f32>,
    /// Speed in km/h.
    pub speed: Vec<f32>,
    /// Throttle, 0 to 1.
    pub throttle: Vec<f32>,
    /// Brake, 0 to 1.
    pub brake: Vec<f32>,
    /// Clutch, 0 to 1, 1 pressed.
    pub clutch: Vec<f32>,
    /// Steering, -1 to 1, positive to the right.
    pub steer: Vec<f32>,
    /// Gear, -1 reverse, 0 neutral.
    pub gear: Vec<f32>,
    /// Engine speed in rpm.
    pub rpm: Vec<f32>,
    /// Lateral acceleration in g, positive to the right.
    pub g_lat: Vec<f32>,
    /// Longitudinal acceleration in g, positive when accelerating.
    pub g_long: Vec<f32>,
    /// Vertical acceleration in g.
    pub g_vert: Vec<f32>,
    /// Fuel in litres.
    pub fuel: Vec<f32>,
    /// Brake lock-up flag, 0 or 1.
    pub abs: Vec<f32>,
    /// 0 until the lap was found invalid, 1 after.
    pub invalid: Vec<f32>,
    /// Pit mode code.
    pub pit_mode: Vec<f32>,
    /// Pitwall's surface code of the car.
    pub ext_surface: Vec<f32>,
    /// The session's incident count.
    pub ext_incidents: Vec<f32>,
    /// Metres east of the start/finish line, by dead reckoning.
    pub x: Vec<f32>,
    /// Metres north of the start/finish line, by dead reckoning.
    pub z: Vec<f32>,
}

impl Columns {
    fn all_mut(&mut self) -> [&mut Vec<f32>; 20] {
        [
            &mut self.t,
            &mut self.dist,
            &mut self.speed,
            &mut self.throttle,
            &mut self.brake,
            &mut self.clutch,
            &mut self.steer,
            &mut self.gear,
            &mut self.rpm,
            &mut self.g_lat,
            &mut self.g_long,
            &mut self.g_vert,
            &mut self.fuel,
            &mut self.abs,
            &mut self.invalid,
            &mut self.pit_mode,
            &mut self.ext_surface,
            &mut self.ext_incidents,
            &mut self.x,
            &mut self.z,
        ]
    }
}

/// The buffer a lap is recorded into.
#[derive(Clone, Debug)]
pub struct LapBuffer {
    cols: Columns,
    present: Present,
    length_m: f32,
    last_surface: f32,
    dr: DeadReckoner,
    // The address of every row, kept outside the channel set: `SessionTick` and `LapDistPct`.
    ticks: Vec<i32>,
    pct: Vec<f32>,
    start_cross: Option<f64>,
    end_cross: Option<f64>,
}

/// Where the samples of a lap can be found again in the simulator's disk telemetry: the
/// `SessionTick` and `LapDistPct` of every row, and the session time of the lap's line crossings.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LapAddress {
    /// The `SessionTick` of every row.
    pub ticks: Vec<i32>,
    /// The `LapDistPct` of every row.
    pub pct: Vec<f32>,
    /// The session time at which the lap started at the line on the track, when it did.
    pub start_crossing: Option<f64>,
    /// The session time at which the lap ended at the line on the track, when it did.
    pub end_crossing: Option<f64>,
}

impl LapBuffer {
    /// A buffer with [`RESERVE_ROWS`] rows reserved in each column.
    pub fn new() -> LapBuffer {
        LapBuffer::with_capacity(RESERVE_ROWS)
    }

    /// A buffer with `rows` rows reserved in each column.
    pub fn with_capacity(rows: usize) -> LapBuffer {
        let mut cols = Columns::default();
        for c in cols.all_mut() {
            c.reserve_exact(rows);
        }
        LapBuffer {
            cols,
            present: Present::ALL,
            length_m: f32::INFINITY,
            last_surface: 0.0,
            dr: DeadReckoner::new(),
            ticks: Vec::with_capacity(rows),
            pct: Vec::with_capacity(rows),
            start_cross: None,
            end_cross: None,
        }
    }

    /// Sets the track length used to clamp `dist`. Not positive means no upper clamp.
    pub fn set_length(&mut self, length_m: f32) {
        self.length_m = if length_m > 0.0 {
            length_m
        } else {
            f32::INFINITY
        };
    }

    /// Empties every column and keeps the capacity.
    pub fn clear(&mut self) {
        for c in self.cols.all_mut() {
            c.clear();
        }
        self.ticks.clear();
        self.pct.clear();
        self.start_cross = None;
        self.end_cross = None;
        self.present = Present::ALL;
        self.last_surface = 0.0;
        self.dr.reset();
    }

    /// Starts the position of a lap at session time `at_s`. `anchored_start`: the lap starts at
    /// the line on the track.
    pub fn begin_position(&mut self, at_s: f64, anchored_start: bool) {
        self.dr.begin(at_s, anchored_start);
        self.start_cross = anchored_start.then_some(at_s);
    }

    /// Anchors the lap's `x` and `z` (see [`crate::position::finish`]). `crossed`: the session
    /// time the lap ended at the line on the track, `None` when it ended elsewhere.
    /// `closure`: whether a lap from the line to the line may be closed (not after a pit visit);
    /// without it such a lap keeps its start anchor only.
    pub fn finish_position(&mut self, crossed: Option<f64>, closure: bool) {
        self.end_cross = crossed;
        if !self.dr.ok() {
            self.present.position = false;
            return;
        }
        let start = self.dr.start_anchored();
        let (end, path_total) = match crossed {
            Some(c) if closure || !start => match self.dr.end_point(c) {
                Some((e, n)) => {
                    let (pe, pn) = self.dr.pos();
                    (
                        PosEnd::AtLine { e, n },
                        self.dr.path_m() + (e - pe).hypot(n - pn),
                    )
                }
                None => {
                    self.present.position = false;
                    return;
                }
            },
            _ => (PosEnd::NotAtLine, self.dr.path_m()),
        };
        let c = &mut self.cols;
        if !finish(&mut c.x, &mut c.z, start, end, self.length_m, path_total) {
            self.present.position = false;
        }
    }

    /// Takes the lap's address (see [`LapAddress`]) out of the buffer, which is consumed anyway:
    /// nothing is copied or allocated.
    pub fn take_address(&mut self) -> LapAddress {
        LapAddress {
            ticks: std::mem::take(&mut self.ticks),
            pct: std::mem::take(&mut self.pct),
            start_crossing: self.start_cross,
            end_crossing: self.end_cross,
        }
    }

    /// The `SessionTick` of every row recorded (0 when the variable is absent).
    pub fn ticks(&self) -> &[i32] {
        &self.ticks
    }

    /// The `LapDistPct` of every row recorded.
    pub fn pct(&self) -> &[f32] {
        &self.pct
    }

    /// The session time at which the lap started at the line on the track, when it did.
    pub fn start_crossing(&self) -> Option<f64> {
        self.start_cross
    }

    /// The session time at which the lap ended at the line on the track, when it did.
    pub fn end_crossing(&self) -> Option<f64> {
        self.end_cross
    }

    /// Rows recorded.
    pub fn len(&self) -> usize {
        self.cols.t.len()
    }

    /// Whether no row is recorded.
    pub fn is_empty(&self) -> bool {
        self.cols.t.is_empty()
    }

    /// Rows every column can take without allocating.
    pub fn capacity(&self) -> usize {
        self.cols.t.capacity()
    }

    /// Takes the columns and the presence flags, consuming the buffer (a finished lap hands its
    /// columns to the writer without copying them).
    pub fn into_parts(self) -> (Columns, Present) {
        (self.cols, self.present)
    }

    /// The columns.
    pub fn columns(&self) -> &Columns {
        &self.cols
    }

    /// Which optional channels were present on every row.
    pub fn present(&self) -> Present {
        self.present
    }

    /// Records one row. Allocates nothing while the lap is within the reserved rows.
    pub fn push(&mut self, t: &Tick, lap: &LapPoint, pit_mode: u8) {
        self.ticks.push(t.session_tick.unwrap_or(0));
        self.pct.push(t.lap_dist_pct);
        let c = &mut self.cols;
        c.t.push(if lap.timed { lap.lap_clock_s } else { -1.0 });
        c.dist.push(t.lap_dist.clamp(0.0, self.length_m));
        c.speed.push(t.speed * 3.6);
        c.throttle.push(t.throttle);
        c.brake.push(t.brake);
        c.steer.push(steer(t.steer_rad, t.steer_max_rad));
        c.gear.push(t.gear as f32);
        c.rpm.push(t.rpm);
        c.g_lat.push(-t.lat_accel / G);
        c.g_long.push(t.long_accel / G);
        c.fuel.push(t.fuel_l);
        c.invalid.push(if lap.valid { 0.0 } else { 1.0 });
        c.pit_mode.push(f32::from(pit_mode));

        // The simulator's clutch reads 1 released: the channel is 1 pressed.
        self.present.clutch &= t.clutch.is_some();
        c.clutch.push(t.clutch.map_or(0.0, |v| 1.0 - v));
        self.present.g_vert &= t.vert_accel.is_some();
        c.g_vert.push(t.vert_accel.map_or(0.0, |v| v / G));
        self.present.abs &= t.abs_active.is_some();
        c.abs.push(match t.abs_active {
            Some(true) => 1.0,
            _ => 0.0,
        });
        self.present.surface &= t.surface.is_some();
        if let Some(code) = t.surface.and_then(|raw| surface_code(surface_class(raw))) {
            self.last_surface = f32::from(code);
        }
        c.ext_surface.push(self.last_surface);
        self.present.incidents &= t.incidents.is_some();
        c.ext_incidents.push(t.incidents.map_or(0.0, |v| v as f32));
        let (x, z) = self.dr.step(t);
        c.x.push(x);
        c.z.push(z);
        self.present.position &= self.dr.ok();
    }
}

impl Default for LapBuffer {
    fn default() -> LapBuffer {
        LapBuffer::new()
    }
}

/// Steering as a fraction of full lock, positive to the right.
///
/// The simulator reports the angle positive to the left and its maximum as the lock to lock
/// angle (observed), so full lock to one side is half of it. A missing or non-positive maximum
/// means 450 degrees lock to lock.
fn steer(angle_rad: f32, max_rad: Option<f32>) -> f32 {
    let lock = match max_rad {
        Some(m) if m > 0.0 => m,
        _ => DEFAULT_STEER_LOCK_RAD,
    };
    (-angle_rad / (lock / 2.0)).clamp(-1.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn point() -> LapPoint {
        LapPoint {
            timed: true,
            lap_clock_s: 12.5,
            valid: true,
        }
    }

    #[test]
    fn conversions() {
        let mut t = Tick::new();
        t.lap_dist = 1500.0;
        t.speed = 50.0;
        t.throttle = 0.4;
        t.brake = 0.1;
        t.steer_rad = -3.5;
        t.steer_max_rad = Some(14.0);
        t.gear = 3;
        t.lat_accel = 9.80665;
        t.long_accel = 4.903325;
        t.vert_accel = Some(9.80665);
        t.fuel_l = 30.0;
        t.clutch = Some(1.0);
        t.abs_active = Some(true);
        t.surface = Some(15); // grass
        t.incidents = Some(4);
        t.yaw_north = Some(0.0);
        let mut b = LapBuffer::new();
        b.set_length(4000.0);
        b.push(&t, &point(), 2);
        let c = b.columns();
        assert_eq!(c.t, [12.5]);
        assert_eq!(c.dist, [1500.0]);
        assert_eq!(c.speed, [180.0]);
        assert!((c.steer[0] - 0.5).abs() < 1e-6);
        assert_eq!(c.g_lat, [-1.0]);
        assert_eq!(c.g_long, [0.5]);
        assert_eq!(c.g_vert, [1.0]);
        assert_eq!(c.clutch, [0.0]);
        assert_eq!(c.abs, [1.0]);
        assert_eq!(c.ext_surface, [2.0]);
        assert_eq!(c.ext_incidents, [4.0]);
        assert_eq!(c.pit_mode, [2.0]);
        assert_eq!(c.invalid, [0.0]);
        assert_eq!(b.present(), Present::ALL);
    }

    #[test]
    fn steering_uses_half_the_lock_to_lock_angle() {
        assert_eq!(steer(7.0, Some(14.0)), -1.0);
        assert_eq!(steer(-7.0, Some(14.0)), 1.0);
        assert_eq!(steer(-20.0, Some(14.0)), 1.0);
        assert!((steer(-3.927, None) - 1.0).abs() < 1e-3);
        assert_eq!(steer(1.0, Some(0.0)), steer(1.0, None));
    }

    #[test]
    fn untimed_and_invalid_rows() {
        let t = Tick::new();
        let mut b = LapBuffer::new();
        b.push(
            &t,
            &LapPoint {
                timed: false,
                lap_clock_s: 3.0,
                valid: false,
            },
            0,
        );
        assert_eq!(b.columns().t, [-1.0]);
        assert_eq!(b.columns().invalid, [1.0]);
    }

    #[test]
    fn distance_is_clamped() {
        let mut t = Tick::new();
        let mut b = LapBuffer::new();
        b.set_length(4000.0);
        t.lap_dist = -3.0;
        b.push(&t, &point(), 0);
        t.lap_dist = 4010.0;
        b.push(&t, &point(), 0);
        assert_eq!(b.columns().dist, [0.0, 4000.0]);
    }

    #[test]
    fn absent_variables_leave_the_channel_out() {
        let mut t = Tick::new();
        t.clutch = Some(1.0);
        let mut b = LapBuffer::new();
        b.push(&t, &point(), 0);
        t.clutch = None;
        b.push(&t, &point(), 0);
        let p = b.present();
        assert!(!p.clutch && !p.g_vert && !p.abs && !p.surface && !p.incidents);
        b.clear();
        assert_eq!(b.present(), Present::ALL);
    }

    #[test]
    fn surface_without_a_meaning_keeps_the_previous_code() {
        let mut t = Tick::new();
        let mut b = LapBuffer::new();
        t.surface = Some(15);
        b.push(&t, &point(), 0);
        t.surface = Some(-1);
        b.push(&t, &point(), 0);
        assert_eq!(b.columns().ext_surface, [2.0, 2.0]);
    }

    #[test]
    fn rows_keep_their_address_and_the_crossings() {
        let mut b = LapBuffer::with_capacity(4);
        let mut t = Tick::new();
        b.begin_position(10.0, true);
        for (tick, pct) in [(100, 0.001_f32), (101, 0.002)] {
            t.session_tick = Some(tick);
            t.lap_dist_pct = pct;
            b.push(&t, &point(), 0);
        }
        t.session_tick = None;
        b.push(&t, &point(), 0);
        assert_eq!(b.ticks(), [100, 101, 0]);
        assert_eq!(b.pct(), [0.001, 0.002, 0.002]);
        assert_eq!(b.start_crossing(), Some(10.0));
        b.finish_position(Some(95.5), true);
        assert_eq!(b.end_crossing(), Some(95.5));
        let cap = b.capacity();
        b.clear();
        assert!(b.ticks().is_empty() && b.pct().is_empty());
        assert_eq!((b.start_crossing(), b.end_crossing()), (None, None));
        b.begin_position(1.0, false);
        assert_eq!(b.start_crossing(), None);
        assert_eq!(b.capacity(), cap);
    }

    #[test]
    fn clear_keeps_capacity_and_long_laps_grow() {
        let t = Tick::new();
        let mut b = LapBuffer::with_capacity(4);
        for _ in 0..10 {
            b.push(&t, &point(), 0);
        }
        assert_eq!(b.len(), 10);
        let cap = b.capacity();
        assert!(cap >= 10);
        b.clear();
        assert!(b.is_empty());
        assert_eq!(b.capacity(), cap);
        assert!(LapBuffer::new().capacity() >= RESERVE_ROWS);
    }
}

#[cfg(test)]
mod position_tests {
    use super::*;

    fn point() -> LapPoint {
        LapPoint {
            timed: true,
            lap_clock_s: 0.0,
            valid: true,
        }
    }

    /// A straight line north at 50 m/s, a row every 0.1 s from `0.0`.
    fn straight(b: &mut LapBuffer, rows: usize) {
        for k in 0..rows {
            let mut t = Tick::new();
            t.session_time = k as f64 * 0.1;
            t.speed = 50.0;
            t.yaw_north = Some(0.0);
            b.push(&t, &point(), 0);
        }
    }

    #[test]
    fn a_lap_without_a_heading_has_no_position() {
        let mut b = LapBuffer::new();
        b.push(&Tick::new(), &point(), 0);
        assert!(!b.present().position);
        assert_eq!((b.columns().x.len(), b.columns().z.len()), (1, 1));
    }

    #[test]
    fn the_end_anchor_puts_the_line_at_the_origin() {
        let mut b = LapBuffer::new();
        b.begin_position(0.0, false);
        straight(&mut b, 11);
        // The line was crossed 0.05 s after the last row: 2.5 m further.
        b.finish_position(Some(1.05), true);
        assert!(b.present().position);
        let z = &b.columns().z;
        assert!((z[10] + 2.5).abs() < 1e-4, "{}", z[10]);
        assert!((z[0] + 52.5).abs() < 1e-3);
    }

    #[test]
    fn a_pit_visit_keeps_the_start_anchor_and_skips_the_closure() {
        let mut b = LapBuffer::new();
        b.begin_position(0.0, true);
        straight(&mut b, 11);
        b.finish_position(Some(1.0), false);
        assert!(b.present().position);
        assert!((b.columns().z[10] - 50.0).abs() < 1e-4);
    }

    #[test]
    fn a_lap_with_neither_end_at_the_line_has_no_position() {
        let mut b = LapBuffer::new();
        b.begin_position(0.0, false);
        straight(&mut b, 5);
        b.finish_position(None, false);
        assert!(!b.present().position);
    }

    #[test]
    fn a_closure_beyond_the_guard_drops_the_position() {
        let mut b = LapBuffer::new();
        b.set_length(4000.0);
        b.begin_position(0.0, true);
        straight(&mut b, 11);
        b.finish_position(Some(1.0), true);
        assert!(!b.present().position);
    }
}
