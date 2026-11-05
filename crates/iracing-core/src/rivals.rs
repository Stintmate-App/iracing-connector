//! The laps of the other cars: one record per car per lap the simulator counts, and the rule
//! that sends the catalog again when cars join after the session opened.
//!
//! [`RivalTracker`] reads the per-car arrays of every row. While no lap is counted a step
//! allocates nothing; a counted lap builds one [`ParticipantLap`] (names are copied then).

use std::sync::Arc;

use crate::io::{ParticipantLap, Sink, TimeSource};
use crate::meta::SessionMeta;
use crate::sectors::SectorClock;
use crate::tick::Tick;
use crate::vars::CARS;

/// Longest wait, in seconds of session time, for the simulator to time a lap after the car
/// crossed the line before the time is measured instead.
pub const TIME_WAIT_S: f64 = 3.0;

/// Longest wait, in seconds of session time, for the session text to name a car that already
/// counts laps. The text lags the telemetry when a car joins.
pub const ROSTER_WAIT_S: f64 = 10.0;

/// Longest time, in seconds of session time, rows wait before they are handed over.
pub const FLUSH_INTERVAL_S: f64 = 30.0;

/// Rows kept without growing.
const ROWS_RESERVED: usize = 256;

/// A crossing seen, waiting for the lap time or the car's name.
struct Pending {
    lap: i32,
    /// Interpolated session time of this crossing.
    crossed_at_s: f64,
    /// Session time of the crossing that started the lap, when it was seen.
    started_at_s: Option<f64>,
    touched_pit: bool,
    pit_stops: u32,
    sectors: SectorClock,
    /// `CarIdxLastLapTime` before the crossing; the time is official once it differs.
    last_before: f32,
    /// Session time the crossing was noticed.
    detected_s: f64,
    official: Option<f32>,
    time_done: bool,
}

struct CarTrack {
    /// Whether the car was seen in the world (`CarIdxLapCompleted` of 0 or more).
    seen: bool,
    /// The simulator's `UserID` of the driver in the slot, to tell a reused slot.
    user_id: Option<i64>,
    lap_completed: i32,
    /// Session time of the crossing that started the lap in progress.
    crossed_at_s: Option<f64>,
    from_line: bool,
    prev_pct: f32,
    sectors: SectorClock,
    touched_pit: bool,
    pit_stops: u32,
    prev_on_pit: bool,
    last_seen: f32,
    pending: Option<Pending>,
}

impl CarTrack {
    fn new(starts: &[f64]) -> CarTrack {
        CarTrack {
            seen: false,
            user_id: None,
            lap_completed: -1,
            crossed_at_s: None,
            from_line: false,
            prev_pct: 0.0,
            sectors: SectorClock::new(starts),
            touched_pit: false,
            pit_stops: 0,
            prev_on_pit: false,
            last_seen: -1.0,
            pending: None,
        }
    }
}

/// Counts the laps of every real car of the field, the player's included.
pub struct RivalTracker {
    cars: Vec<CarTrack>,
    rows: Vec<ParticipantLap>,
    last_flush_s: f64,
    starts: Vec<f64>,
    prev_t: f64,
    have_prev: bool,
}

impl Default for RivalTracker {
    fn default() -> Self {
        RivalTracker::new()
    }
}

impl RivalTracker {
    /// A tracker with room for [`CARS`] cars; everything it needs is reserved here.
    pub fn new() -> RivalTracker {
        let starts = Vec::with_capacity(crate::sectors::MAX_SECTORS);
        RivalTracker {
            cars: (0..CARS).map(|_| CarTrack::new(&[])).collect(),
            rows: Vec::with_capacity(ROWS_RESERVED),
            last_flush_s: 0.0,
            starts,
            prev_t: 0.0,
            have_prev: false,
        }
    }

    /// Forgets every car: a new session. Rows not yet taken are dropped.
    pub fn reset(&mut self) {
        for c in &mut self.cars {
            *c = CarTrack::new(&self.starts);
        }
        self.rows.clear();
        self.have_prev = false;
        self.last_flush_s = 0.0;
    }

    /// Feeds one row. Does nothing for a row that is not new or while the simulator is gone.
    pub fn step(&mut self, t: &Tick, meta: &SessionMeta) {
        if !t.connected || !t.fresh {
            return;
        }
        let now = t.session_time;
        if self.have_prev && now < self.prev_t {
            // The session time went back: the run restarted. Cars start over, rows stay.
            let rows = std::mem::take(&mut self.rows);
            self.reset();
            self.rows = rows;
        }
        self.sync_sectors(meta);
        for i in 0..CARS {
            let completed = t.car_lap_completed[i];
            if completed < 0 || i as i32 == meta.pace_car_idx {
                self.cars[i].seen = false;
                self.cars[i].pending = None;
                continue;
            }
            if matches!(&meta.roster[i], Some(d) if !d.is_real) {
                continue;
            }
            self.car_step(i, completed, t, meta, now);
            self.try_resolve(i, t, meta, now);
        }
        self.prev_t = now;
        self.have_prev = true;
    }

    /// The rows waiting, when they are due: the player finished a lap, or
    /// [`FLUSH_INTERVAL_S`] passed since the last hand-over. `now_s` is the session time.
    pub fn take_due(&mut self, now_s: f64, player_lap_ended: bool) -> Option<Vec<ParticipantLap>> {
        if self.rows.is_empty() {
            return None;
        }
        let due = player_lap_ended
            || now_s - self.last_flush_s >= FLUSH_INTERVAL_S
            || now_s < self.last_flush_s;
        if !due {
            return None;
        }
        self.last_flush_s = now_s;
        Some(std::mem::replace(
            &mut self.rows,
            Vec::with_capacity(ROWS_RESERVED),
        ))
    }

    /// The sector starts of the track, in the clock of every car (rebuilt only when they change).
    fn sync_sectors(&mut self, meta: &SessionMeta) {
        if self.starts == meta.sector_starts {
            return;
        }
        self.starts.clear();
        self.starts.extend_from_slice(&meta.sector_starts);
        for c in &mut self.cars {
            c.sectors = SectorClock::new(&self.starts);
            c.seen = false;
            c.pending = None;
        }
    }

    fn car_step(&mut self, i: usize, completed: i32, t: &Tick, meta: &SessionMeta, now: f64) {
        let pct = t.car_pct[i];
        let on_pit = t.car_on_pit_road[i];
        let uid = meta.roster[i].as_ref().map(|d| d.user_id);
        let c = &mut self.cars[i];
        if c.seen && uid.is_some() && c.user_id.is_some() && c.user_id != uid {
            // A slot reused by another driver: nothing of the old one carries over.
            *c = CarTrack::new(&self.starts);
        }
        if uid.is_some() {
            c.user_id = uid;
        }
        let last = t.car_last.map_or(-1.0, |l| l[i]);
        if !c.seen {
            c.seen = true;
            c.lap_completed = completed;
            c.crossed_at_s = None;
            c.from_line = false;
            c.prev_pct = pct;
            c.sectors.start_lap(now, true);
            c.touched_pit = on_pit;
            c.prev_on_pit = on_pit;
            c.last_seen = last;
            return;
        }
        if on_pit && !c.prev_on_pit {
            c.pit_stops += 1;
        }
        c.prev_on_pit = on_pit;
        c.touched_pit |= on_pit;
        if completed > c.lap_completed {
            let crossed = crossing_time(c.prev_pct, pct, self.prev_t, now);
            let started = if c.from_line { c.crossed_at_s } else { None };
            let pending = Pending {
                lap: completed,
                crossed_at_s: crossed,
                started_at_s: started,
                touched_pit: c.touched_pit,
                pit_stops: c.pit_stops,
                sectors: c.sectors.clone(),
                last_before: c.last_seen,
                detected_s: now,
                official: None,
                time_done: false,
            };
            let old = c.pending.replace(pending);
            c.lap_completed = completed;
            c.crossed_at_s = Some(crossed);
            c.from_line = true;
            c.sectors.start_lap(crossed, false);
            c.touched_pit = on_pit;
            if let Some(old) = old {
                // The previous crossing never got its time or its name: send it as it is.
                self.push_row(i, old, t, meta);
            }
        } else if completed < c.lap_completed {
            // The counter went back (the car was reset): the lap in progress starts over.
            c.lap_completed = completed;
            c.crossed_at_s = None;
            c.from_line = false;
            c.sectors.start_lap(now, true);
            c.touched_pit = on_pit;
        } else {
            c.sectors
                .step(f64::from(c.prev_pct), f64::from(pct), self.prev_t, now);
        }
        let c = &mut self.cars[i];
        c.prev_pct = pct;
        c.last_seen = last;
    }

    /// Completes the pending lap of car `i` when its time and its name are known or have
    /// waited long enough.
    fn try_resolve(&mut self, i: usize, t: &Tick, meta: &SessionMeta, now: f64) {
        let Some(p) = self.cars[i].pending.as_mut() else {
            return;
        };
        if !p.time_done {
            if let Some(l) = t.car_last {
                let v = l[i];
                if v > 0.0 && v != p.last_before {
                    p.official = Some(v);
                    p.time_done = true;
                }
            }
            if !p.time_done && now - p.detected_s >= TIME_WAIT_S {
                p.time_done = true;
            }
        }
        if !p.time_done {
            return;
        }
        if meta.roster[i].is_none() && now - p.detected_s < ROSTER_WAIT_S {
            return;
        }
        if let Some(p) = self.cars[i].pending.take() {
            self.push_row(i, p, t, meta);
        }
    }

    fn push_row(&mut self, i: usize, p: Pending, t: &Tick, meta: &SessionMeta) {
        let (name, car_id, class_id) = match &meta.roster[i] {
            Some(d) => (d.name.clone(), d.car_id.clone(), d.class_id.clone()),
            None => (format!("Car {i}"), String::new(), String::new()),
        };
        let measured = p
            .started_at_s
            .map(|s| (p.crossed_at_s - s) as f32)
            .filter(|m| *m > 0.0);
        let (time_s, time_source) = match (p.official, measured) {
            (Some(v), _) => (Some(v), Some(TimeSource::Official)),
            (None, Some(m)) => (Some(m), Some(TimeSource::Measured)),
            (None, None) => (None, None),
        };
        let sectors = p.sectors.finish(p.crossed_at_s, p.official);
        let position = t
            .car_position
            .map(|a| a[i])
            .filter(|v| *v > 0)
            // `CarIdxPosition` is 0 until the first timed lap (observed): the session text's
            // result position stands in. Whether the arrays hold every car online is not
            // observed yet.
            .or_else(|| Some(meta.results[i].position).filter(|v| *v > 0));
        let state =
            (meta.results[i].reason_out_id != 0).then(|| meta.results[i].reason_out.clone());
        self.rows.push(ParticipantLap {
            participant: i as u8,
            lap: p.lap,
            name,
            car_id,
            class_id,
            is_player: i as i32 == t.player_idx,
            time_s,
            time_source,
            sectors: sectors.as_slice().to_vec(),
            valid: p.official.is_some() && !p.touched_pit,
            pit: p.touched_pit,
            pit_stops: p.pit_stops,
            from_line: p.started_at_s.is_some(),
            position,
            crossed_at_s: Some(p.crossed_at_s),
            sim_best_lap_s: t.car_best.map(|a| a[i]).filter(|v| *v > 0.0),
            state,
        });
    }
}

/// The session time at which a car crossed the line between two rows, interpolated when the
/// lap fraction wrapped; the current time when it did not (a counter that rose elsewhere).
fn crossing_time(prev_pct: f32, pct: f32, prev_t: f64, t: f64) -> f64 {
    if prev_pct > 0.5 && pct < 0.5 {
        let before = f64::from(1.0 - prev_pct);
        let total = before + f64::from(pct);
        if total > 0.0 {
            return prev_t + before / total * (t - prev_t);
        }
    }
    t
}

/// Decides when the catalog goes to the sink again: the cars of a new session description
/// that differs from the last one sent.
///
/// The recorder offers every new description here before passing it to the
/// [`RivalTracker`], so the catalog is queued before any row that names a new car.
#[derive(Debug, Default)]
pub struct CatalogWatch {
    sent: Option<u64>,
}

impl CatalogWatch {
    /// A watch with nothing sent.
    pub fn new() -> CatalogWatch {
        CatalogWatch::default()
    }

    /// A session opened with `meta`: its catalog goes with the opening.
    pub fn opened(&mut self, meta: &SessionMeta) {
        self.sent = Some(meta.catalog_hash);
    }

    /// Forgets what was sent (the session ended).
    pub fn closed(&mut self) {
        self.sent = None;
    }

    /// Sends the catalog when `meta` differs from the last one sent. Returns whether it did.
    /// Before a session opens nothing is sent: the opening carries the catalog.
    pub fn offer(&mut self, meta: &Arc<SessionMeta>, sink: &mut dyn Sink) -> bool {
        match self.sent {
            Some(h) if h == meta.catalog_hash => false,
            Some(_) => {
                sink.catalog(Arc::clone(meta));
                self.sent = Some(meta.catalog_hash);
                true
            }
            None => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::meta::DriverEntry;

    fn entry(name: &str, uid: i64) -> DriverEntry {
        DriverEntry {
            name: name.to_owned(),
            user_id: uid,
            car_id: "car".to_owned(),
            car_name: "Car".to_owned(),
            class_id: "class".to_owned(),
            class_name: "Class".to_owned(),
            class_est_lap_s: None,
            is_real: true,
        }
    }

    fn row(t: &mut Tick, time: f64) {
        t.session_time = time;
        t.connected = true;
        t.fresh = true;
    }

    fn setup() -> (Box<Tick>, SessionMeta) {
        let mut t = Tick::new();
        t.player_idx = 1;
        t.car_lap_completed = [-1; CARS];
        let mut m = SessionMeta::empty(0);
        m.pace_car_idx = 0;
        m.roster[1] = Some(entry("Me", 1001));
        m.roster[2] = Some(entry("Other", 1002));
        (t, m)
    }

    #[test]
    fn crossing_is_interpolated() {
        let x = crossing_time(0.99, 0.01, 10.0, 10.5);
        assert!((x - 10.25).abs() < 1e-6);
        assert_eq!(crossing_time(0.2, 0.3, 10.0, 10.5), 10.5);
    }

    #[test]
    fn counts_a_lap_with_its_official_time_and_pit_stops() {
        let (mut t, m) = setup();
        let mut r = RivalTracker::new();
        t.car_last = Some([-1.0; CARS]);
        t.car_lap_completed[2] = 3;
        t.car_pct[2] = 0.99;
        row(&mut t, 100.0);
        r.step(&t, &m);
        t.car_lap_completed[2] = 4;
        t.car_pct[2] = 0.01;
        t.car_last.as_mut().unwrap()[2] = 81.5;
        row(&mut t, 100.5);
        r.step(&t, &m);
        let rows = r.take_due(101.0, true).expect("rows");
        assert_eq!(rows.len(), 1);
        let l = &rows[0];
        assert_eq!((l.participant, l.lap, l.name.as_str()), (2, 4, "Other"));
        assert_eq!(l.time_s, Some(81.5));
        assert_eq!(l.time_source, Some(TimeSource::Official));
        assert!(l.valid && !l.pit && !l.from_line);
        assert!(r.take_due(102.0, true).is_none());
    }

    #[test]
    fn a_lap_through_the_pits_is_not_valid_and_counts_the_stop() {
        let (mut t, m) = setup();
        let mut r = RivalTracker::new();
        t.car_last = Some([-1.0; CARS]);
        t.car_lap_completed[2] = 1;
        row(&mut t, 1.0);
        r.step(&t, &m);
        t.car_on_pit_road[2] = true;
        row(&mut t, 2.0);
        r.step(&t, &m);
        t.car_on_pit_road[2] = false;
        t.car_lap_completed[2] = 2;
        t.car_last.as_mut().unwrap()[2] = 90.0;
        row(&mut t, 3.0);
        r.step(&t, &m);
        let rows = r.take_due(4.0, true).expect("rows");
        assert!(rows[0].pit && !rows[0].valid);
        assert_eq!(rows[0].pit_stops, 1);
    }

    #[test]
    fn untimed_lap_is_measured_after_the_wait() {
        let (mut t, m) = setup();
        let mut r = RivalTracker::new();
        t.car_last = Some([-1.0; CARS]);
        t.car_lap_completed[2] = 1;
        row(&mut t, 10.0);
        r.step(&t, &m);
        t.car_lap_completed[2] = 2;
        row(&mut t, 70.0); // first crossing seen: no start, no time
        r.step(&t, &m);
        t.car_lap_completed[2] = 3;
        row(&mut t, 130.0);
        r.step(&t, &m);
        row(&mut t, 134.0);
        r.step(&t, &m);
        let rows = r.take_due(134.0, true).expect("rows");
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].time_source, None);
        assert_eq!(rows[1].time_source, Some(TimeSource::Measured));
        assert_eq!(rows[1].time_s, Some(60.0));
        assert!(!rows[1].valid && rows[1].from_line);
    }

    #[test]
    fn unknown_car_is_held_then_named_by_slot() {
        let (mut t, mut m) = setup();
        let mut r = RivalTracker::new();
        t.car_last = Some([-1.0; CARS]);
        t.car_lap_completed[5] = 1;
        row(&mut t, 1.0);
        r.step(&t, &m);
        t.car_lap_completed[5] = 2;
        t.car_last.as_mut().unwrap()[5] = 70.0;
        row(&mut t, 2.0);
        r.step(&t, &m);
        assert!(r.take_due(5.0, true).is_none(), "held");
        m.roster[5] = Some(entry("Late", 1005));
        row(&mut t, 3.0);
        r.step(&t, &m);
        assert_eq!(r.take_due(5.0, true).expect("rows")[0].name, "Late");
        // Never named: sent after ROSTER_WAIT_S with a name from the slot.
        t.car_lap_completed[6] = 1;
        row(&mut t, 4.0);
        r.step(&t, &m);
        t.car_lap_completed[6] = 2;
        t.car_last.as_mut().unwrap()[6] = 70.0;
        row(&mut t, 5.0);
        r.step(&t, &m);
        row(&mut t, 15.5);
        r.step(&t, &m);
        let rows = r.take_due(16.0, true).expect("rows");
        assert_eq!(rows[0].name, "Car 6");
        assert!(rows[0].car_id.is_empty());
    }

    #[test]
    fn a_reused_slot_starts_over() {
        let (mut t, mut m) = setup();
        let mut r = RivalTracker::new();
        t.car_last = Some([-1.0; CARS]);
        t.car_lap_completed[2] = 5;
        row(&mut t, 1.0);
        r.step(&t, &m);
        m.roster[2] = Some(entry("Newcomer", 2002));
        t.car_lap_completed[2] = 6;
        row(&mut t, 2.0);
        r.step(&t, &m);
        // The new driver's first counter is only a baseline: no row, as it is for any new car.
        assert!(r.take_due(3.0, true).is_none());
    }

    #[test]
    fn rows_are_flushed_on_the_interval() {
        let (mut t, m) = setup();
        let mut r = RivalTracker::new();
        t.car_last = Some([-1.0; CARS]);
        t.car_lap_completed[2] = 1;
        row(&mut t, 1.0);
        r.step(&t, &m);
        t.car_lap_completed[2] = 2;
        t.car_last.as_mut().unwrap()[2] = 60.0;
        row(&mut t, 2.0);
        r.step(&t, &m);
        assert!(r.take_due(20.0, false).is_none());
        assert!(r.take_due(31.0, false).is_some());
    }

    #[test]
    fn position_falls_back_to_the_result() {
        let (mut t, mut m) = setup();
        m.results[2].position = 4;
        let mut r = RivalTracker::new();
        t.car_position = Some([0; CARS]);
        t.car_last = Some([-1.0; CARS]);
        t.car_lap_completed[2] = 1;
        row(&mut t, 1.0);
        r.step(&t, &m);
        t.car_lap_completed[2] = 2;
        t.car_last.as_mut().unwrap()[2] = 60.0;
        row(&mut t, 2.0);
        r.step(&t, &m);
        assert_eq!(r.take_due(3.0, true).expect("rows")[0].position, Some(4));
    }

    #[test]
    fn catalog_watch_sends_only_changes_after_the_opening() {
        struct S(usize);
        impl Sink for S {
            fn start_session(&mut self, _: &crate::io::SessionOpenInfo) -> u64 {
                0
            }
            fn end_session(&mut self, _: u64, _: &str) {}
            fn lap(&mut self, _: u64, _: crate::io::FinishedLap) {}
            fn setup(
                &mut self,
                _: u64,
                _: crate::io::SetupReason,
                _: &str,
                _: Arc<SessionMeta>,
                _: f32,
            ) {
            }
            fn pit_exit(&mut self, _: u64, _: &str, _: Option<f64>) {}
            fn participants(&mut self, _: u64, _: Vec<ParticipantLap>) {}
            fn catalog(&mut self, _: Arc<SessionMeta>) {
                self.0 += 1;
            }
        }
        let mut sink = S(0);
        let mut w = CatalogWatch::new();
        let mut m = SessionMeta::empty(0);
        m.catalog_hash = 7;
        let m1 = Arc::new(m.clone());
        assert!(!w.offer(&m1, &mut sink), "no session yet");
        w.opened(&m1);
        assert!(!w.offer(&m1, &mut sink));
        m.catalog_hash = 8;
        let m2 = Arc::new(m);
        assert!(w.offer(&m2, &mut sink));
        assert!(!w.offer(&m2, &mut sink));
        assert_eq!(sink.0, 1);
    }
}
