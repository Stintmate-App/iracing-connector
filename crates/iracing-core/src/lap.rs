//! The channels of one lap, one column of `f32` per channel, filled row by row.

use irsdk::defs::surface_class;

use crate::codes::surface_code;
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
}

impl Present {
    const ALL: Present = Present {
        clutch: true,
        g_vert: true,
        abs: true,
        surface: true,
        incidents: true,
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
}

impl Columns {
    fn all_mut(&mut self) -> [&mut Vec<f32>; 18] {
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
        self.present = Present::ALL;
        self.last_surface = 0.0;
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
