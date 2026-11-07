//! The car's position on the lap by dead reckoning.
//!
//! The simulator publishes no world position, so each lap integrates the car's velocity over
//! `SessionTime` and anchors the result at the start/finish line. The lap's `x` and `z`
//! channels come out in metres, `x` east and `z` north.
//!
//! # Frame and signs (observed on recorded files)
//!
//! - `VelocityX` is the speed along the car's nose and `VelocityY` is the lateral speed,
//!   positive to the left, both in metres per second.
//! - `Yaw` is a mathematical heading: counter-clockwise positive.
//! - `YawNorth` is a compass heading: clockwise from north, equal to a constant per track minus
//!   `Yaw`. It is the preferred heading: it gives an east/north top-down view with no fitting.
//!   Without it the heading is taken as `pi/2 - Yaw`, a compass heading with an arbitrary but
//!   fixed north (the top-down view stays rigid and right-handed).
//!
//! With heading `psi` (clockwise from north), forward speed `u` and lateral speed `v` (left
//! positive) the velocity in the top-down view is
//!
//! ```text
//! east  = u sin(psi) - v cos(psi)
//! north = u cos(psi) + v sin(psi)
//! ```
//!
//! Without `VelocityX` and `VelocityY` the speed stands in for `u` and `v` is zero (about ten
//! metres of error over a lap, against about two with the velocity). Without any heading a lap
//! has no position.
//!
//! # Integration, anchor and closure
//!
//! The integration is trapezoidal over `SessionTime`, with `f64` accumulators. A step longer
//! than [`MAX_DR_STEP_S`] drops the lap's position; a repeated row (no time passed) adds
//! nothing. At the lap's end [`finish`] anchors the lap at the line and, for a lap that is
//! driven from the line to the line, spreads the closure error over the lap in proportion to
//! the path driven. Nothing here allocates once the columns are reserved.

use std::f64::consts::FRAC_PI_2;

use crate::tick::Tick;

/// The longest step between two rows that is integrated, in seconds. A longer one is a pause or
/// a gap, not a small step to guess across, and the lap sends no position.
pub const MAX_DR_STEP_S: f64 = 0.25;

/// The smallest closure error that is ever refused, in metres.
pub const CLOSURE_MIN_GUARD_M: f64 = 10.0;

/// The closure error that is refused, as a fraction of the track length (when above
/// [`CLOSURE_MIN_GUARD_M`]).
pub const CLOSURE_GUARD_FRAC: f64 = 0.005;

/// How the velocity is known.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    /// `VelocityX` and `VelocityY` (forward and lateral).
    Velocity,
    /// `Speed` along the heading, no sideslip.
    Speed,
}

/// Where the heading comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Heading {
    /// `YawNorth`, a compass heading.
    Compass,
    /// `Yaw`, turned into a compass heading.
    Yaw,
}

/// Whether the end of a lap was at the start/finish line on the track.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PosEnd {
    /// The lap ended at the line; the dead-reckoned point of the line is `(e, n)`.
    AtLine {
        /// East, in metres.
        e: f64,
        /// North, in metres.
        n: f64,
    },
    /// The lap ended somewhere else.
    NotAtLine,
}

/// The integrator of one lap.
#[derive(Clone, Debug)]
pub struct DeadReckoner {
    pos_e: f64,
    pos_n: f64,
    path_m: f64,
    last_time: f64,
    w_prev: (f64, f64),
    first: bool,
    method: Option<Method>,
    heading: Option<Heading>,
    ok: bool,
    start_anchored: bool,
}

impl DeadReckoner {
    /// An integrator with no lap begun.
    pub fn new() -> DeadReckoner {
        DeadReckoner {
            pos_e: 0.0,
            pos_n: 0.0,
            path_m: 0.0,
            last_time: f64::NAN,
            w_prev: (0.0, 0.0),
            first: true,
            method: None,
            heading: None,
            ok: true,
            start_anchored: false,
        }
    }

    /// Starts a lap at the origin at session time `at_s`. `anchored_start`: the origin is the
    /// point where the car crossed the line on the track.
    pub fn begin(&mut self, at_s: f64, anchored_start: bool) {
        *self = DeadReckoner::new();
        self.last_time = at_s;
        self.start_anchored = anchored_start;
    }

    /// Forgets the lap.
    pub fn reset(&mut self) {
        *self = DeadReckoner::new();
    }

    /// Integrates one row and returns the raw position `(east, north)` after it, in metres.
    pub fn step(&mut self, t: &Tick) -> (f32, f32) {
        let (src, psi) = if let Some(h) = t.yaw_north {
            (Heading::Compass, f64::from(h))
        } else if let Some(y) = t.yaw {
            (Heading::Yaw, FRAC_PI_2 - f64::from(y))
        } else {
            self.ok = false;
            return self.out();
        };
        let have_velocity = t.vel_x.is_some() && t.vel_y.is_some();
        let method = *self.method.get_or_insert(if have_velocity {
            Method::Velocity
        } else {
            Method::Speed
        });
        if method == Method::Velocity && !have_velocity {
            self.ok = false;
        }
        if *self.heading.get_or_insert(src) != src {
            self.ok = false;
        }
        let (u, v) = match (method, t.vel_x, t.vel_y) {
            (Method::Velocity, Some(x), Some(y)) => (f64::from(x), f64::from(y)),
            _ => (f64::from(t.speed), 0.0),
        };
        let (s, c) = psi.sin_cos();
        let w = (u * s - v * c, u * c + v * s);
        if self.first {
            self.w_prev = w;
            self.first = false;
        }
        let dt = if self.last_time.is_nan() {
            0.0
        } else {
            t.session_time - self.last_time
        };
        if dt > MAX_DR_STEP_S {
            self.ok = false;
        } else if dt > 0.0 {
            let de = 0.5 * (self.w_prev.0 + w.0) * dt;
            let dn = 0.5 * (self.w_prev.1 + w.1) * dt;
            self.pos_e += de;
            self.pos_n += dn;
            self.path_m += de.hypot(dn);
        }
        self.last_time = t.session_time;
        self.w_prev = w;
        self.out()
    }

    fn out(&self) -> (f32, f32) {
        (self.pos_e as f32, self.pos_n as f32)
    }

    /// Where dead reckoning puts the line, when the lap ends at session time `crossed`: the last
    /// point carried on at the last velocity. `None` when that is further than a step.
    pub fn end_point(&self, crossed: f64) -> Option<(f64, f64)> {
        let dt = if self.last_time.is_nan() {
            0.0
        } else {
            (crossed - self.last_time).max(0.0)
        };
        (dt <= MAX_DR_STEP_S).then_some({
            (
                self.pos_e + self.w_prev.0 * dt,
                self.pos_n + self.w_prev.1 * dt,
            )
        })
    }

    /// Whether every row so far could be integrated.
    pub fn ok(&self) -> bool {
        self.ok
    }

    /// Integrated path length so far, in metres.
    pub fn path_m(&self) -> f64 {
        self.path_m
    }

    /// The raw position so far, in metres.
    pub fn pos(&self) -> (f64, f64) {
        (self.pos_e, self.pos_n)
    }

    /// Whether the lap's origin is the line on the track.
    pub fn start_anchored(&self) -> bool {
        self.start_anchored
    }

    /// How the velocity was taken, once a row was seen.
    pub fn method(&self) -> Option<Method> {
        self.method
    }
}

impl Default for DeadReckoner {
    fn default() -> DeadReckoner {
        DeadReckoner::new()
    }
}

/// Anchors a lap's columns at the line, in place. `false`: the lap has no usable position and
/// its channels are dropped.
///
/// - Start and end at the line: the closure error `-end` is spread over the lap by path length
///   (`path_total` is the integrated length up to the line). Refused when it exceeds
///   `max(CLOSURE_MIN_GUARD_M, CLOSURE_GUARD_FRAC * length_m)`.
/// - Start at the line, end elsewhere: kept as integrated.
/// - Start elsewhere, end at the line: the whole lap is shifted so that the line is `(0, 0)`.
/// - Neither: `false`.
pub fn finish(
    x: &mut [f32],
    z: &mut [f32],
    start_anchored: bool,
    end: PosEnd,
    length_m: f32,
    path_total: f64,
) -> bool {
    match (start_anchored, end) {
        (true, PosEnd::AtLine { e, n }) => {
            let guard = if length_m.is_finite() && length_m > 0.0 {
                CLOSURE_MIN_GUARD_M.max(CLOSURE_GUARD_FRAC * f64::from(length_m))
            } else {
                CLOSURE_MIN_GUARD_M
            };
            if e.hypot(n) > guard {
                return false;
            }
            if path_total <= 0.0 {
                return true;
            }
            let (mut pe, mut pn, mut cum) = (0.0, 0.0, 0.0);
            for (xi, zi) in x.iter_mut().zip(z.iter_mut()) {
                let (ce, cn) = (f64::from(*xi), f64::from(*zi));
                cum += (ce - pe).hypot(cn - pn);
                (pe, pn) = (ce, cn);
                let w = (cum / path_total).min(1.0);
                *xi = (ce - w * e) as f32;
                *zi = (cn - w * n) as f32;
            }
            true
        }
        (true, PosEnd::NotAtLine) => true,
        (false, PosEnd::AtLine { e, n }) => {
            for (xi, zi) in x.iter_mut().zip(z.iter_mut()) {
                *xi = (f64::from(*xi) - e) as f32;
                *zi = (f64::from(*zi) - n) as f32;
            }
            true
        }
        (false, PosEnd::NotAtLine) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::{PI, TAU};

    const R: f64 = 100.0;
    const V: f64 = 25.0;

    /// A tick at `time` with the given compass heading and velocity.
    fn tick(time: f64, psi: f64, u: f64, v: Option<f64>) -> Box<Tick> {
        let mut t = Tick::new();
        t.session_time = time;
        t.speed = u.hypot(v.unwrap_or(0.0)) as f32;
        t.yaw_north = Some(psi as f32);
        if let Some(v) = v {
            t.vel_x = Some(u as f32);
            t.vel_y = Some(v as f32);
        }
        t
    }

    struct Lap {
        x: Vec<f32>,
        z: Vec<f32>,
        dr: DeadReckoner,
    }

    /// One lap: `rows + 1` rows in equal steps over `lap_s`; `pose(s)` gives the heading, the
    /// forward speed and the lateral speed at the fraction `s` of the lap.
    fn run(rows: usize, lap_s: f64, pose: impl Fn(f64) -> (f64, f64, Option<f64>)) -> Lap {
        let mut dr = DeadReckoner::new();
        dr.begin(0.0, true);
        let (mut x, mut z) = (Vec::new(), Vec::new());
        for k in 0..=rows {
            let s = k as f64 / rows as f64;
            let (psi, u, v) = pose(s);
            let (e, n) = dr.step(&tick(s * lap_s, psi, u, v));
            x.push(e);
            z.push(n);
        }
        Lap { x, z, dr }
    }

    fn circle(s: f64) -> (f64, f64, Option<f64>) {
        (TAU * s, V, Some(0.0))
    }

    fn circle_lap() -> Lap {
        let lap_s = TAU * R / V;
        run((lap_s * 60.0) as usize, lap_s, circle)
    }

    fn off_circle(e: f32, n: f32) -> f64 {
        (f64::from(e) - R).hypot(f64::from(n)) - R
    }

    #[test]
    fn circle_closes_and_matches() {
        let lap = circle_lap();
        let (e, n) = lap.dr.pos();
        assert!(e.hypot(n) < 0.05, "closure {}", e.hypot(n));
        for (x, z) in lap.x.iter().zip(&lap.z) {
            assert!(off_circle(*x, *z).abs() < 0.05);
        }
        assert!((lap.dr.path_m() / (TAU * R) - 1.0).abs() < 5e-4);
        // A clockwise lap heading north first turns right: the centre is to the east.
        let q = lap.x.len() / 8;
        assert!(lap.x[q] > 0.0 && lap.z[q] > 0.0);
        assert!(lap.dr.ok());
        assert_eq!(lap.dr.method(), Some(Method::Velocity));
    }

    fn worst_gap(a: &Lap, b: &Lap) -> f64 {
        a.x.iter()
            .zip(&a.z)
            .zip(b.x.iter().zip(&b.z))
            .map(|((ax, az), (bx, bz))| f64::from(ax - bx).hypot(f64::from(az - bz)))
            .fold(0.0, f64::max)
    }

    #[test]
    fn sideslip_is_the_same_trace_and_the_sign_matters() {
        let beta = 5.0_f64.to_radians();
        let lap_s = TAU * R / V;
        let rows = (lap_s * 60.0) as usize;
        let slip = |sign: f64| {
            run(rows, lap_s, |s| {
                // The nose points inward (clockwise of the velocity); the velocity is to the
                // left of the nose.
                (TAU * s + beta, V * beta.cos(), Some(sign * V * beta.sin()))
            })
        };
        let plain = circle_lap();
        assert!(worst_gap(&slip(1.0), &plain) < 0.05);
        assert!(worst_gap(&slip(-1.0), &plain) > 5.0);
    }

    /// A stadium: 400 m straights joined by 100 m half circles, clockwise from the line at the
    /// start of the first straight, heading north. The heading after `d` metres.
    fn oval_pose(d: f64) -> f64 {
        let half = PI * R;
        if d < 400.0 {
            0.0
        } else if d < 400.0 + half {
            (d - 400.0) / R
        } else if d < 800.0 + half {
            PI
        } else {
            PI + (d - 800.0 - half) / R
        }
    }

    /// Distance to the true stadium (core: the segment x = 100, z in 0..400, radius 100).
    fn off_oval(e: f32, n: f32) -> f64 {
        let z = f64::from(n).clamp(0.0, 400.0);
        ((f64::from(e) - 100.0).hypot(f64::from(n) - z) - R).abs()
    }

    fn worst_off_oval(lap: &Lap) -> f64 {
        lap.x
            .iter()
            .zip(&lap.z)
            .map(|(x, z)| off_oval(*x, *z))
            .fold(0.0, f64::max)
    }

    #[test]
    fn oval_with_drift_is_closed_by_the_spread() {
        let len = 800.0 + 2.0 * PI * R;
        let speed = 50.0;
        let lap_s = len / speed;
        // The compass drifts from +0.1 to +1.3 degrees over the lap and the speed reads 0.2 %
        // high: a rigid bias alone would close the loop and prove nothing.
        let mut lap = run((lap_s * 60.0) as usize, lap_s, |s| {
            let bias = (0.1 + 1.2 * s).to_radians();
            (oval_pose(s * len) + bias, speed * 1.002, Some(0.0))
        });
        let (e, n) = lap.dr.pos();
        let closure = e.hypot(n);
        assert!((2.0..6.0).contains(&closure), "raw closure {closure}");
        let raw_worst = worst_off_oval(&lap);
        let total = lap.dr.path_m();
        assert!(finish(
            &mut lap.x,
            &mut lap.z,
            true,
            PosEnd::AtLine { e, n },
            1500.0,
            total
        ));
        let (lx, lz) = (*lap.x.last().unwrap(), *lap.z.last().unwrap());
        assert!(f64::from(lx).hypot(f64::from(lz)) < 1e-3, "{lx} {lz}");
        assert_eq!((lap.x[0], lap.z[0]), (0.0, 0.0));
        let worst = worst_off_oval(&lap);
        assert!(worst < closure, "{worst} against the closure {closure}");
        assert!(worst < raw_worst, "{worst} against raw {raw_worst}");
    }

    #[test]
    fn path_weights_stay_flat_across_a_stop() {
        // A circle with a heading drift (a closure of a few metres), the car stopped for 10 s
        // half way.
        let lap_s = TAU * R / V;
        let rows = (lap_s * 60.0) as usize;
        let dt = lap_s / rows as f64;
        let mut dr = DeadReckoner::new();
        dr.begin(0.0, true);
        let (mut x, mut z) = (Vec::new(), Vec::new());
        let mut stop_at = 0;
        let mut clock = 0.0;
        for k in 0..=rows {
            let s = k as f64 / rows as f64;
            let psi = TAU * s + 0.02 * s;
            if k == rows / 2 {
                stop_at = x.len();
                for _ in 0..600 {
                    let (e, n) = dr.step(&tick(clock, psi, 0.0, Some(0.0)));
                    x.push(e);
                    z.push(n);
                    clock += 1.0 / 60.0;
                }
            }
            let (e, n) = dr.step(&tick(clock, psi, V, Some(0.0)));
            x.push(e);
            z.push(n);
            clock += dt;
        }
        let (e, n) = dr.pos();
        assert!(e.hypot(n) > 1.0, "closure {}", e.hypot(n));
        let total = dr.path_m();
        assert!(finish(
            &mut x,
            &mut z,
            true,
            PosEnd::AtLine { e, n },
            1000.0,
            total
        ));
        let jump = f64::from(x[stop_at + 598] - x[stop_at + 1])
            .hypot(f64::from(z[stop_at + 598] - z[stop_at + 1]));
        assert!(
            jump < 1e-3,
            "the correction moved by {jump} m across the stop"
        );
    }

    #[test]
    fn anchors() {
        let mut x = vec![1.0, 2.0, 3.0];
        let mut z = vec![0.5, 0.5, 0.5];
        let end = PosEnd::AtLine { e: 3.0, n: 0.5 };
        assert!(finish(&mut x, &mut z, false, end, 4000.0, 3.0));
        assert_eq!(x, [-2.0, -1.0, 0.0]);
        assert_eq!(z, [0.0, 0.0, 0.0]);
        let (mut x, mut z) = (vec![1.0, 2.0], vec![5.0, 6.0]);
        assert!(finish(&mut x, &mut z, true, PosEnd::NotAtLine, 4000.0, 3.0));
        assert_eq!((x, z), (vec![1.0, 2.0], vec![5.0, 6.0]));
        let (mut x, mut z) = (vec![1.0], vec![1.0]);
        assert!(!finish(
            &mut x,
            &mut z,
            false,
            PosEnd::NotAtLine,
            4000.0,
            3.0
        ));
    }

    #[test]
    fn the_guard_refuses_a_large_closure() {
        let cols = || (vec![0.0, 10.0, 20.0], vec![0.0; 3]);
        let (mut x, mut z) = cols();
        let bad = PosEnd::AtLine { e: 24.0, n: 0.0 };
        assert!(!finish(&mut x, &mut z, true, bad, 4000.0, 4000.0));
        let (mut x, mut z) = cols();
        let ok = PosEnd::AtLine { e: 16.0, n: 0.0 };
        assert!(finish(&mut x, &mut z, true, ok, 4000.0, 4000.0));
        // A short track: the floor is 10 m.
        let (mut x, mut z) = cols();
        let short = PosEnd::AtLine { e: 11.0, n: 0.0 };
        assert!(!finish(&mut x, &mut z, true, short, 1000.0, 1000.0));
    }

    #[test]
    fn repeated_rows_add_nothing_and_a_gap_drops_the_position() {
        let mut dr = DeadReckoner::new();
        dr.begin(0.0, true);
        dr.step(&tick(0.0, 0.0, 20.0, Some(0.0)));
        let a = dr.step(&tick(0.1, 0.0, 20.0, Some(0.0)));
        let b = dr.step(&tick(0.1, 0.0, 20.0, Some(0.0)));
        assert_eq!(a, b);
        assert!((dr.path_m() - 2.0).abs() < 1e-9);
        assert!(dr.ok());
        dr.step(&tick(0.1 + 0.3, 0.0, 20.0, Some(0.0)));
        assert!(!dr.ok());
    }

    #[test]
    fn a_step_of_a_quarter_second_is_still_integrated() {
        let mut dr = DeadReckoner::new();
        dr.begin(0.0, true);
        dr.step(&tick(0.0, 0.0, 20.0, Some(0.0)));
        dr.step(&tick(0.25, 0.0, 20.0, Some(0.0)));
        assert!(dr.ok());
    }

    #[test]
    fn fallbacks() {
        let lap_s = TAU * R / V;
        let rows = (lap_s * 60.0) as usize;
        // No velocity: the speed along the heading.
        let a = run(rows, lap_s, |s| (TAU * s, V, None));
        assert!(a.dr.ok());
        assert_eq!(a.dr.method(), Some(Method::Speed));
        let c = circle_lap();
        assert!(worst_gap(&a, &c) < 0.05);
        // Yaw only: psi = pi/2 - Yaw, the same circle turned about the start.
        let mut dr = DeadReckoner::new();
        dr.begin(0.0, true);
        let (mut x, mut z) = (Vec::new(), Vec::new());
        for k in 0..=rows {
            let s = k as f64 / rows as f64;
            let mut t = tick(s * lap_s, 0.0, V, Some(0.0));
            t.yaw_north = None;
            t.yaw = Some((FRAC_PI_2 - TAU * s) as f32);
            let (e, n) = dr.step(&t);
            x.push(e);
            z.push(n);
        }
        assert!(dr.ok());
        let dist = |p: (f32, f32), q: (f32, f32)| f64::from(p.0 - q.0).hypot(f64::from(p.1 - q.1));
        for i in (0..x.len()).step_by(97) {
            for j in (0..x.len()).step_by(211) {
                let d1 = dist((x[i], z[i]), (x[j], z[j]));
                let d2 = dist((c.x[i], c.z[i]), (c.x[j], c.z[j]));
                assert!((d1 - d2).abs() < 0.05);
            }
        }
        // No heading at all.
        let mut dr = DeadReckoner::new();
        dr.begin(0.0, true);
        let mut t = tick(0.0, 0.0, 20.0, Some(0.0));
        t.yaw_north = None;
        dr.step(&t);
        assert!(!dr.ok());
        // A heading that changes source half way.
        let mut dr = DeadReckoner::new();
        dr.begin(0.0, true);
        dr.step(&tick(0.0, 0.0, 20.0, Some(0.0)));
        let mut t = tick(0.01, 0.0, 20.0, Some(0.0));
        t.yaw_north = None;
        t.yaw = Some(0.0);
        dr.step(&t);
        assert!(!dr.ok());
        // The velocity lost half way.
        let mut dr = DeadReckoner::new();
        dr.begin(0.0, true);
        dr.step(&tick(0.0, 0.0, 20.0, Some(0.0)));
        dr.step(&tick(0.01, 0.0, 20.0, None));
        assert!(!dr.ok());
    }

    #[test]
    fn the_trapezoid_beats_forward_euler() {
        let lap_s = TAU * R / V;
        let rows = (lap_s * 60.0) as usize;
        let trap = circle_lap();
        let (mut e, mut n) = (0.0_f64, 0.0_f64);
        let mut euler_worst = 0.0_f64;
        let dt = lap_s / rows as f64;
        for k in 1..=rows {
            let psi = TAU * (k - 1) as f64 / rows as f64;
            e += V * psi.sin() * dt;
            n += V * psi.cos() * dt;
            euler_worst = euler_worst.max(((e - R).hypot(n) - R).abs());
        }
        let trap_worst = trap
            .x
            .iter()
            .zip(&trap.z)
            .map(|(x, z)| off_circle(*x, *z).abs())
            .fold(0.0, f64::max);
        assert!(
            trap_worst * 5.0 < euler_worst,
            "{trap_worst} against {euler_worst}"
        );
    }

    #[test]
    fn the_end_point_carries_on_at_the_last_velocity() {
        let mut dr = DeadReckoner::new();
        dr.begin(0.0, true);
        dr.step(&tick(0.0, 0.0, 20.0, Some(0.0)));
        dr.step(&tick(0.1, 0.0, 20.0, Some(0.0)));
        let (e, n) = dr.end_point(0.1125).unwrap();
        assert!(e.abs() < 1e-9 && (n - 2.25).abs() < 1e-9);
        assert!(dr.end_point(1.0).is_none());
    }
}
