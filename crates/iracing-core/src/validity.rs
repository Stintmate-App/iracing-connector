//! The validity of a lap, inferred from what the telemetry shows.
//!
//! iRacing publishes no per-lap validity flag, so the connector infers one: an incident, a stay
//! off the track, a tow or reset, or a touch of the pit road invalidates the lap.

use irsdk::defs::incident_flags;

use crate::tick::Tick;

/// `PlayerTrackSurface` value of a car off the track.
const TRK_LOC_OFF_TRACK: i32 = 0;

/// `PlayerTrackSurface` value of a car on the track.
#[cfg(test)]
const TRK_LOC_ON_TRACK: i32 = 3;

/// Seconds the car must stay off the track before the lap is invalidated.
pub const OFF_HOLD_S: f64 = 0.1;

/// Seconds within which an incident and a stay off the track belong together.
pub const INCIDENT_LINK_S: f64 = 2.0;

/// Seconds within which two rises of the incident count are one incident.
pub const INCIDENT_MERGE_S: f64 = 2.0;

/// Seconds back from a rise of the incident count in which the last non-zero incident report is
/// still taken as its report.
pub const REPORT_LOOKBACK_S: f64 = 1.0;

/// Why a lap was found invalid.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InvalidCause {
    /// The car left the track.
    OffTrack,
    /// Something invalidated the lap and the cause is not known.
    Unknown,
}

impl InvalidCause {
    /// The cause as Pitwall names it.
    pub fn as_str(self) -> &'static str {
        match self {
            InvalidCause::OffTrack => "off_track",
            InvalidCause::Unknown => "unknown",
        }
    }
}

/// A rise of the session's incident count seen on one row.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct IncidentRise {
    /// Points added.
    pub delta: i32,
    /// The incident report byte (`0` when none was seen).
    pub report: u8,
    /// Session time of the row.
    pub at_s: f64,
    /// The cause the rise points to.
    pub cause: InvalidCause,
}

/// Follows the session's incident count: the points, rises, and a merged pending incident.
#[derive(Clone, Debug)]
pub struct IncidentTracker {
    /// The session's incident points as last read.
    pub points_session: i32,
    /// The incident report byte last seen, `0` when none yet.
    pub last_report: u8,
    last_report_at: f64,
    /// Points and start time of an incident that may still grow.
    pending: Option<(i32, f64)>,
    seen: bool,
    /// Session time at which the car was last seen off the track (or now).
    last_off_s: Option<f64>,
}

impl IncidentTracker {
    /// A tracker with nothing seen.
    pub fn new() -> IncidentTracker {
        IncidentTracker {
            points_session: 0,
            last_report: 0,
            last_report_at: f64::NEG_INFINITY,
            pending: None,
            seen: false,
            last_off_s: None,
        }
    }

    /// Forgets everything (a new session or a reconnection).
    pub fn reset(&mut self) {
        *self = IncidentTracker::new();
    }

    /// Reads one row. Returns the rise of the incident count on this row, if any. Allocates
    /// nothing.
    ///
    /// The cause comes from the report byte of `PlayerIncidents` on this row or up to
    /// [`REPORT_LOOKBACK_S`] before it: report 2 is off track, any other report is unknown.
    /// With no report, a rise within [`INCIDENT_LINK_S`] of the car being off the track is off
    /// track and otherwise unknown. Without the variable the cause is unknown.
    pub fn step(&mut self, t: &Tick) -> Option<IncidentRise> {
        let now = t.session_time;
        if t.track_loc == TRK_LOC_OFF_TRACK {
            self.last_off_s = Some(now);
        }
        if let Some(word) = t.incident_flags {
            let report = incident_flags::report(word) as u8;
            if report != 0 {
                self.last_report = report;
                self.last_report_at = now;
            }
        }
        let count = t.incidents?;
        let prev = std::mem::replace(&mut self.points_session, count);
        let first = !std::mem::replace(&mut self.seen, true);
        if first || count <= prev {
            return None;
        }
        let delta = count - prev;
        let report = if now - self.last_report_at <= REPORT_LOOKBACK_S {
            self.last_report
        } else {
            0
        };
        let linked = self
            .last_off_s
            .is_some_and(|off| now - off <= INCIDENT_LINK_S);
        let by_report = report == incident_flags::REPORT_OFF_TRACK as u8;
        let by_link = report == 0 && t.incident_flags.is_some() && linked;
        let cause = if by_report || by_link {
            InvalidCause::OffTrack
        } else {
            InvalidCause::Unknown
        };
        self.pending = match self.pending {
            Some((points, since)) if now - since <= INCIDENT_MERGE_S => {
                Some((points + delta, since))
            }
            _ => Some((delta, now)),
        };
        Some(IncidentRise {
            delta,
            report,
            at_s: now,
            cause,
        })
    }

    /// The merged incident that is still open: points and the session time it began.
    pub fn pending(&self) -> Option<(i32, f64)> {
        self.pending
    }

    /// Takes the pending incident once [`INCIDENT_MERGE_S`] passed since it began, so a pair of
    /// rises (iRacing can add a 4x as 2 + 2) is told as one incident. Returns its points.
    pub fn take_settled(&mut self, now_s: f64) -> Option<i32> {
        match self.pending {
            Some((points, since)) if now_s - since > INCIDENT_MERGE_S => {
                self.pending = None;
                Some(points)
            }
            _ => None,
        }
    }
}

impl Default for IncidentTracker {
    fn default() -> IncidentTracker {
        IncidentTracker::new()
    }
}

/// The validity of the lap in progress. Reset at each lap start.
#[derive(Clone, Debug, PartialEq)]
pub struct Validity {
    /// Whether the lap is still valid.
    pub valid: bool,
    /// The first cause found, if any.
    pub cause: Option<InvalidCause>,
    /// Whether the cause is certain.
    pub certain: bool,
    /// Distance along the lap at which the first cause was found.
    pub at_m: Option<f32>,
    /// Lap clock at which the first cause was found.
    pub at_s: Option<f32>,
    off_since: Option<f64>,
    off_fired: bool,
}

impl Validity {
    /// A valid lap.
    pub fn new() -> Validity {
        Validity {
            valid: true,
            cause: None,
            certain: false,
            at_m: None,
            at_s: None,
            off_since: None,
            off_fired: false,
        }
    }

    /// Starts a new lap.
    pub fn reset(&mut self) {
        *self = Validity::new();
    }

    /// Reads one row of a lap in progress.
    ///
    /// `tracking` is true while the car is on the track and not on the pit road; nothing is
    /// judged otherwise. `lap_clock_s` is the lap clock. `rise` is the rise of the incident
    /// count on this row from [`IncidentTracker::step`], which runs on every row. Allocates
    /// nothing.
    pub fn step(
        &mut self,
        t: &Tick,
        tracking: bool,
        lap_clock_s: f32,
        rise: Option<&IncidentRise>,
    ) {
        if !tracking {
            self.off_since = None;
            return;
        }
        if let Some(r) = rise {
            self.set(r.cause, true, t.lap_dist, lap_clock_s);
        }
        if t.track_loc == TRK_LOC_OFF_TRACK {
            let since = *self.off_since.get_or_insert(t.session_time);
            if !self.off_fired && t.session_time - since >= OFF_HOLD_S - 1e-9 {
                self.off_fired = true;
                self.set(InvalidCause::OffTrack, false, t.lap_dist, lap_clock_s);
            }
        } else {
            self.off_since = None;
            self.off_fired = false;
        }
    }

    /// The car was towed or reset during the lap.
    pub fn mark_tow(&mut self, at_m: f32, lap_clock_s: f32) {
        self.set(InvalidCause::Unknown, false, at_m, lap_clock_s);
    }

    /// The lap touched the pit road. It is sent invalid, with no cause.
    pub fn mark_pit_road(&mut self) {
        self.valid = false;
    }

    /// iRacing did not time the lap (`LapLastLapTime` was -1, or the lap ended before the
    /// line). It is invalid. The cause stays whatever the lap found (a tow or reset sets it);
    /// with none, it is `unknown`, not certain.
    pub fn mark_not_timed(&mut self) {
        self.valid = false;
        if self.cause.is_none() {
            self.cause = Some(InvalidCause::Unknown);
            self.certain = false;
        }
    }

    /// The lap did not start at the line (the recorder joined mid-lap, or the car jumped on the
    /// track): its trace covers part of the lap and the simulator's time is not its time. It is
    /// invalid with cause `unknown`.
    pub fn mark_mid_lap_start(&mut self, at_m: f32) {
        self.set(InvalidCause::Unknown, false, at_m, 0.0);
    }

    /// The first cause makes the lap invalid; a later one of the same cause only raises
    /// `certain`.
    fn set(&mut self, cause: InvalidCause, certain: bool, at_m: f32, at_s: f32) {
        match self.cause {
            None => {
                self.valid = false;
                self.cause = Some(cause);
                self.certain = certain;
                self.at_m = Some(at_m);
                self.at_s = Some(at_s);
            }
            Some(first) if first == cause && certain => self.certain = true,
            Some(_) => {}
        }
    }
}

impl Default for Validity {
    fn default() -> Validity {
        Validity::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tick(time: f64) -> Box<Tick> {
        let mut t = Tick::new();
        t.session_time = time;
        t.track_loc = TRK_LOC_ON_TRACK;
        t.incidents = Some(0);
        t.incident_flags = Some(0);
        t
    }

    /// Runs rows at 60 Hz from a closure that edits the tick; returns the validity.
    fn run(
        rows: usize,
        mut edit: impl FnMut(usize, &mut Tick),
    ) -> (Validity, IncidentTracker, Vec<IncidentRise>) {
        let mut v = Validity::new();
        let mut inc = IncidentTracker::new();
        let mut rises = Vec::new();
        for i in 0..rows {
            let mut t = tick(i as f64 / 60.0);
            t.lap_dist = i as f32;
            edit(i, &mut t);
            let rise = inc.step(&t);
            if let Some(r) = rise {
                rises.push(r);
            }
            v.step(&t, true, i as f32 / 60.0, rise.as_ref());
        }
        (v, inc, rises)
    }

    #[test]
    fn clean_lap_stays_valid() {
        let (v, _, _) = run(600, |_, _| {});
        assert!(v.valid && v.cause.is_none());
    }

    #[test]
    fn incident_with_off_track_report_is_certain() {
        let (v, _, r) = run(120, |i, t| {
            if i >= 60 {
                t.incidents = Some(1);
                t.incident_flags = Some(0x200 | 2);
            }
        });
        assert_eq!(r.len(), 1);
        assert_eq!(
            (v.valid, v.cause, v.certain),
            (false, Some(InvalidCause::OffTrack), true)
        );
        assert_eq!(v.at_m, Some(60.0));
    }

    #[test]
    fn other_reports_are_unknown_and_certain() {
        for report in [1u32, 4, 5, 7, 8] {
            let (v, _, _) = run(120, |i, t| {
                if i >= 60 {
                    t.incidents = Some(2);
                    t.incident_flags = Some(0x300 | report);
                }
            });
            assert_eq!(
                (v.cause, v.certain),
                (Some(InvalidCause::Unknown), true),
                "{report}"
            );
        }
    }

    #[test]
    fn report_a_moment_before_the_rise_is_used() {
        let (_, _, r) = run(240, |i, t| {
            if i == 60 {
                t.incident_flags = Some(2);
            }
            if i >= 90 {
                t.incidents = Some(1);
            }
        });
        assert_eq!(r[0].report, 2);
        assert_eq!(r[0].cause, InvalidCause::OffTrack);
        // Older than the look-back: no report.
        let (_, _, r) = run(400, |i, t| {
            if i == 60 {
                t.incident_flags = Some(2);
            }
            if i >= 300 {
                t.incidents = Some(1);
            }
        });
        assert_eq!(r[0].report, 0);
    }

    #[test]
    fn report_zero_near_off_track_is_off_track_otherwise_unknown() {
        // Off the track at 30..45, the points rise 1 s later with no report.
        let (_, _, r) = run(300, |i, t| {
            if (30..45).contains(&i) {
                t.track_loc = 0;
            }
            if i >= 105 {
                t.incidents = Some(1);
            }
        });
        assert_eq!(r[0].cause, InvalidCause::OffTrack);
        // Too late.
        let (_, _, r) = run(400, |i, t| {
            if (30..45).contains(&i) {
                t.track_loc = 0;
            }
            if i >= 300 {
                t.incidents = Some(1);
            }
        });
        assert_eq!(r[0].cause, InvalidCause::Unknown);
    }

    #[test]
    fn without_the_report_variable_the_cause_is_unknown() {
        let (_, _, r) = run(120, |i, t| {
            t.incident_flags = None;
            if i >= 60 {
                t.incidents = Some(1);
            }
        });
        assert_eq!(r[0].cause, InvalidCause::Unknown);
    }

    #[test]
    fn off_track_needs_the_hold_time_and_is_uncertain() {
        // 5 rows off the track (83 ms): nothing.
        let (v, _, _) = run(120, |i, t| {
            if (10..15).contains(&i) {
                t.track_loc = 0;
            }
        });
        assert!(v.valid);
        // 7 rows: the sixth row after the first reaches 0.1 s.
        let (v, _, _) = run(120, |i, t| {
            if (10..17).contains(&i) {
                t.track_loc = 0;
            }
        });
        assert_eq!(
            (v.valid, v.cause, v.certain),
            (false, Some(InvalidCause::OffTrack), false)
        );
        assert_eq!(v.at_m, Some(16.0));
    }

    #[test]
    fn an_off_track_incident_that_follows_makes_it_certain() {
        let (v, _, _) = run(240, |i, t| {
            if (10..40).contains(&i) {
                t.track_loc = 0;
            }
            if i >= 60 {
                t.incidents = Some(1);
                t.incident_flags = Some(2);
            }
        });
        assert_eq!((v.cause, v.certain), (Some(InvalidCause::OffTrack), true));
        assert_eq!(v.at_m, Some(16.0));
    }

    #[test]
    fn the_first_cause_wins() {
        let (v, _, _) = run(240, |i, t| {
            if i >= 20 {
                t.incidents = Some(1);
                t.incident_flags = Some(7);
            }
            if (100..130).contains(&i) {
                t.track_loc = 0;
            }
        });
        assert_eq!(v.cause, Some(InvalidCause::Unknown));
        assert_eq!(v.at_m, Some(20.0));
    }

    #[test]
    fn nothing_is_judged_off_the_track_proper() {
        let mut v = Validity::new();
        let mut t = tick(0.0);
        t.track_loc = 0;
        for i in 0..100 {
            t.session_time = i as f64 / 60.0;
            v.step(&t, false, 0.0, None);
        }
        assert!(v.valid);
    }

    #[test]
    fn tow_pit_road_and_untimed_laps() {
        let mut v = Validity::new();
        v.mark_tow(100.0, 5.0);
        assert_eq!(
            (v.valid, v.cause, v.certain),
            (false, Some(InvalidCause::Unknown), false)
        );
        let mut v = Validity::new();
        v.mark_pit_road();
        assert_eq!((v.valid, v.cause), (false, None));
        let mut v = Validity::new();
        v.mark_not_timed();
        assert!(!v.valid);
        assert_eq!((v.cause, v.certain), (Some(InvalidCause::Unknown), false));
        // A cause found earlier (a reset, a tow) stays.
        let mut v = Validity::new();
        v.mark_tow(10.0, 1.0);
        v.mark_not_timed();
        assert_eq!(v.cause, Some(InvalidCause::Unknown));
        assert_eq!(v.at_m, Some(10.0));
        let mut v = Validity::new();
        v.mark_mid_lap_start(250.0);
        assert_eq!(
            (v.valid, v.cause, v.at_m),
            (false, Some(InvalidCause::Unknown), Some(250.0))
        );
        v.reset();
        assert!(v.valid);
    }

    #[test]
    fn incident_tracker_merges_rises_within_two_seconds() {
        let (_, mut inc, rises) = run(300, |i, t| {
            t.incidents = Some(match i {
                0..=59 => 0,
                60..=119 => 2,
                _ => 4,
            });
        });
        assert_eq!(rises.iter().map(|r| r.delta).collect::<Vec<_>>(), [2, 2]);
        assert_eq!(inc.points_session, 4);
        assert_eq!(inc.pending(), Some((4, 1.0)));
        assert_eq!(inc.take_settled(3.0), None);
        assert_eq!(inc.take_settled(4.5), Some(4));
        assert_eq!(inc.pending(), None);
    }

    #[test]
    fn incident_tracker_starts_from_the_first_value_and_ignores_falls() {
        let mut inc = IncidentTracker::new();
        let mut t = tick(0.0);
        t.incidents = Some(8);
        assert_eq!(inc.step(&t), None);
        t.incidents = Some(2);
        assert_eq!(inc.step(&t), None);
        assert_eq!(inc.points_session, 2);
        t.incidents = None;
        assert_eq!(inc.step(&t), None);
    }
}
