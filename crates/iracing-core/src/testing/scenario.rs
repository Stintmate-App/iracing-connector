//! Scripted drives that produce rows and session texts on a 60 Hz clock.

use irsdk::defs::{incident_flags, session_flags};
use irsdk::header::DiskHeader;
use irsdk::ibt::Ibt;
use irsdk::testing::{Layout, RowBuilder, ibt_bytes};

use super::full_layout;
use super::yaml::{TyreReadings, YamlBuilder};
use crate::vars::CARS;

const HZ: f64 = 60.0;
const DT: f64 = 1.0 / HZ;
/// Speed on the pit road, in metres per second.
const PIT_SPEED: f64 = 22.0;
/// Where the pit stall is on the lap, as a fraction.
const STALL_PCT: f64 = 0.98;
/// Where the pit road starts, as a fraction.
const PIT_ENTRY_PCT: f64 = 0.9;
/// Where the pit road ends after the line, as a fraction.
const PIT_EXIT_PCT: f64 = 0.03;
/// `PlayerTrackSurface` values.
const NOT_IN_WORLD: i32 = -1;
const OFF_TRACK: i32 = 0;
const IN_STALL: i32 = 1;
const ON_PIT_ROAD: i32 = 2;
const ON_TRACK: i32 = 3;
/// A grass `PlayerTrackSurfaceMaterial`.
const GRASS: i32 = 15;
const ASPHALT: i32 = 1;

/// How a lap is driven.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LapProfile {
    /// Seconds a whole lap takes (a partial lap takes the same share of it).
    pub time_s: f64,
    /// Litres burnt over a whole lap.
    pub fuel_l: f64,
    /// The time `LapLastLapTime` shows after the line; `None` leaves it as it was.
    pub official: Option<f32>,
    /// Rows between the line and the new `LapLastLapTime`.
    pub official_delay_rows: u32,
}

impl LapProfile {
    /// A lap of `time_s` seconds that burns 2.5 litres and is timed officially at `time_s`
    /// six rows after the line.
    pub fn new(time_s: f64) -> LapProfile {
        LapProfile {
            time_s,
            fuel_l: 2.5,
            official: Some(time_s as f32),
            official_delay_rows: 6,
        }
    }

    /// Sets the litres burnt over a whole lap.
    pub fn fuel(mut self, litres: f64) -> LapProfile {
        self.fuel_l = litres;
        self
    }

    /// Sets the official time shown after the line.
    pub fn official(mut self, time: Option<f32>) -> LapProfile {
        self.official = time;
        self
    }
}

/// Which side a car is alongside.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    /// On the left.
    Left,
    /// On the right.
    Right,
}

/// A change of the session text at a row.
#[derive(Clone, Debug, PartialEq)]
pub struct YamlUpdate {
    /// The index of the first row that follows the change.
    pub at_row: usize,
    /// The new text, Windows-1252.
    pub text: Vec<u8>,
    /// Whether the simulator started a new event (a new layout, ticks from 0) rather than
    /// changing the text.
    pub restart: bool,
}

#[derive(Clone, Copy, Debug)]
enum Effect {
    OffTrack,
    Cut,
}

#[derive(Clone, Copy, Debug)]
enum Scheduled {
    Effect(Effect),
    Incident { points: i32, report: u32 },
}

#[derive(Clone, Copy, Debug)]
struct Event {
    at_pct: f64,
    dur_s: f64,
    what: Scheduled,
}

#[derive(Clone, Copy, Debug)]
struct Car {
    active: bool,
    lap: i32,
    completed: i32,
    pct: f64,
    on_pit_road: bool,
    loc: i32,
    last: f32,
    best: f32,
    flags: u32,
    position: i32,
}

impl Car {
    const OFF: Car = Car {
        active: false,
        lap: -1,
        completed: -1,
        pct: -1.0,
        on_pit_road: false,
        loc: NOT_IN_WORLD,
        last: -1.0,
        best: -1.0,
        flags: 0,
        position: 0,
    };
}

#[derive(Clone, Debug)]
struct Sim {
    t: f64,
    session_num: i32,
    unique_id: i32,
    state: i32,
    flags: u32,
    lap: i32,
    completed: i32,
    pct: f64,
    last_time: f32,
    best_time: f32,
    lap_start_t: f64,
    speed: f64,
    fuel: f64,
    on_track: bool,
    in_garage: bool,
    in_stall: bool,
    on_pit_road: bool,
    loc: i32,
    material: i32,
    incidents: i32,
    incident_word: u32,
    tow: f32,
    position: i32,
    pits_open: bool,
    car_lr: i32,
    pending_last: Option<(u32, f32)>,
    official: Option<(f32, u32)>,
    events: Vec<Event>,
    effects: Vec<(Effect, f64)>,
    cars: [Car; CARS],
}

impl Sim {
    fn new(session_num: i32) -> Sim {
        Sim {
            t: 0.0,
            session_num,
            unique_id: 1,
            state: 4,
            flags: session_flags::GREEN,
            lap: 0,
            completed: 0,
            pct: STALL_PCT,
            last_time: -1.0,
            best_time: -1.0,
            lap_start_t: 0.0,
            speed: 0.0,
            fuel: 80.0,
            on_track: false,
            in_garage: true,
            in_stall: false,
            on_pit_road: false,
            loc: NOT_IN_WORLD,
            material: ASPHALT,
            incidents: 0,
            incident_word: 0,
            tow: 0.0,
            position: 1,
            pits_open: true,
            car_lr: 1,
            pending_last: None,
            official: None,
            events: Vec::new(),
            effects: Vec::new(),
            cars: [Car::OFF; CARS],
        }
    }
}

/// A script of steps that produces the rows and session texts of a drive.
///
/// The player starts in the garage. Both `Lap` and `LapCompleted` rise at every crossing of the
/// line, the pit lane's included; `LapLastLapTime` changes only after a [`Scenario::lap`] and
/// starts at -1. Steps that take time append rows; an effect scheduled with
/// [`Scenario::off_track`], [`Scenario::cut`] or [`Scenario::incident_at`] happens during the
/// next rows that reach its position on the lap.
#[derive(Clone, Debug)]
pub struct Scenario {
    layout: Layout,
    yaml: YamlBuilder,
    initial_yaml: Vec<u8>,
    updates: Vec<YamlUpdate>,
    gaps: Vec<(usize, f64)>,
    rows: Vec<Vec<u8>>,
    sim: Sim,
    player: usize,
    length_m: f64,
}

impl Scenario {
    /// A scenario on the full layout for the event `yaml` describes.
    pub fn new(yaml: YamlBuilder) -> Scenario {
        let sim = Sim::new(yaml.current_session());
        Scenario {
            layout: full_layout(),
            initial_yaml: yaml.build_cp1252(),
            length_m: yaml.track_length_m(),
            player: usize::try_from(yaml.player()).expect("player slot"),
            yaml,
            updates: Vec::new(),
            gaps: Vec::new(),
            rows: Vec::new(),
            sim,
        }
    }

    // ---- output -------------------------------------------------------------------------

    /// The layout the rows follow.
    pub fn layout(&self) -> &Layout {
        &self.layout
    }

    /// The rows made so far.
    pub fn rows(&self) -> &[Vec<u8>] {
        &self.rows
    }

    /// The session text at the start, Windows-1252.
    pub fn initial_yaml(&self) -> &[u8] {
        &self.initial_yaml
    }

    /// The changes of the session text, in order of row.
    pub fn yaml_updates(&self) -> &[YamlUpdate] {
        &self.updates
    }

    /// The session text as it is now.
    pub fn yaml(&self) -> &YamlBuilder {
        &self.yaml
    }

    /// The gaps in the connection: `(row, seconds)`, where the simulator is gone for
    /// `seconds` before row `row`.
    pub fn disconnects(&self) -> &[(usize, f64)] {
        &self.gaps
    }

    /// The scenario as an `.ibt` image with its first session text.
    pub fn ibt(&self) -> Ibt {
        let disk = DiskHeader {
            session_start_date: 1_700_000_000,
            session_start_time: 0.0,
            session_end_time: self.sim.t,
            session_lap_count: self.sim.completed.max(0),
            session_record_count: i32::try_from(self.rows.len()).expect("row count"),
        };
        let bytes = ibt_bytes(&self.layout, &self.initial_yaml, &self.rows, disk);
        Ibt::from_bytes(bytes).expect("a scenario with at least one row makes a valid file")
    }

    /// Replays the scenario into `map` at `hz` rows per second: the first session text, then
    /// the rows, with the text changes, new events and gaps at their rows. Returns the rows
    /// pushed.
    #[cfg(windows)]
    pub fn replay_into(
        &self,
        map: &mut irsdk::testing::map::TestMap,
        hz: f64,
        stop: &std::sync::atomic::AtomicBool,
    ) -> usize {
        use std::sync::atomic::Ordering;
        use std::time::{Duration, Instant};
        assert!(hz.is_finite() && hz > 0.0, "hz must be above 0");
        let step = Duration::from_secs_f64(1.0 / hz);
        let start = Instant::now();
        let mut skew = Duration::ZERO;
        let (mut next_update, mut next_gap) = (0, 0);
        map.set_yaml(&self.initial_yaml);
        for (i, row) in self.rows.iter().enumerate() {
            if stop.load(Ordering::Relaxed) {
                return i;
            }
            while let Some(u) = self.updates.get(next_update).filter(|u| u.at_row == i) {
                if u.restart {
                    map.restart(&self.layout, &u.text);
                } else {
                    map.set_yaml(&u.text);
                }
                next_update += 1;
            }
            while let Some(&(_, secs)) = self.gaps.get(next_gap).filter(|g| g.0 == i) {
                map.set_connected(false);
                let gap = Duration::from_secs_f64(secs * 60.0 / hz);
                std::thread::sleep(gap);
                skew += gap;
                map.set_connected(true);
                next_gap += 1;
            }
            let deadline = start + skew + step.mul_f64(i as f64);
            if let Some(left) = deadline.checked_duration_since(Instant::now()) {
                std::thread::sleep(left);
            }
            map.push_row(row);
        }
        self.rows.len()
    }

    // ---- the player's steps -------------------------------------------------------------

    /// Sits `s` seconds in the garage.
    pub fn garage(&mut self, s: f64) -> &mut Self {
        self.put_in_garage();
        self.hold(s)
    }

    /// Moves from the garage (or the pit road) into the pit stall and waits half a second.
    pub fn to_stall(&mut self) -> &mut Self {
        let sim = &mut self.sim;
        sim.in_garage = false;
        sim.in_stall = true;
        sim.on_pit_road = true;
        sim.on_track = true;
        sim.loc = IN_STALL;
        sim.pct = STALL_PCT;
        sim.speed = 0.0;
        self.hold(0.5)
    }

    /// Drives from the stall along the pit road, over the line, to the end of the pit road.
    pub fn drive_out(&mut self) -> &mut Self {
        {
            let sim = &mut self.sim;
            sim.in_stall = false;
            sim.on_pit_road = true;
            sim.loc = ON_PIT_ROAD;
        }
        let mut crossed = false;
        while !(crossed && self.sim.pct > PIT_EXIT_PCT) {
            crossed |= self.advance(PIT_SPEED, 0.0);
        }
        self.sim.on_pit_road = false;
        self.sim.loc = ON_TRACK;
        self
    }

    /// Drives from where the car is to the end of the lap at `profile`'s pace, and over the
    /// line.
    pub fn lap(&mut self, profile: LapProfile) -> &mut Self {
        let speed = self.length_m / profile.time_s;
        let per_pct = profile.fuel_l;
        self.sim.official = profile.official.map(|t| (t, profile.official_delay_rows));
        while !self.advance(speed, per_pct) {}
        self.sim.official = None;
        self
    }

    /// Schedules an excursion off the track at `at_pct` of the lap, `s` seconds long.
    pub fn off_track(&mut self, at_pct: f64, s: f64) -> &mut Self {
        self.schedule(at_pct, s, Scheduled::Effect(Effect::OffTrack))
    }

    /// Schedules a cut at `at_pct` of the lap: `s` seconds on grass with the car still on the
    /// track.
    pub fn cut(&mut self, at_pct: f64, s: f64) -> &mut Self {
        self.schedule(at_pct, s, Scheduled::Effect(Effect::Cut))
    }

    /// Schedules an incident of `points` at `at_pct` of the lap, reported as `report` (one of
    /// the `incident_flags::REPORT_*` numbers).
    pub fn incident_at(&mut self, at_pct: f64, points: i32, report: u32) -> &mut Self {
        self.schedule(at_pct, 0.0, Scheduled::Incident { points, report })
    }

    /// An incident of `points` now: the counter rises and the report shows for one row.
    pub fn incident(&mut self, points: i32, report: u32) -> &mut Self {
        self.sim.incidents += points;
        self.sim.incident_word = incident_word(points, report);
        self.hold_rows(1);
        self.sim.incident_word = 0;
        self
    }

    /// A tow: the clock shows a tow for one second, then the car is in its pit stall.
    pub fn tow(&mut self) -> &mut Self {
        self.sim.tow = 5.0;
        self.hold(1.0);
        let sim = &mut self.sim;
        sim.tow = 0.0;
        sim.pct = STALL_PCT;
        sim.in_stall = true;
        sim.on_pit_road = true;
        sim.loc = IN_STALL;
        sim.speed = 0.0;
        self.hold(0.5)
    }

    /// Drives from where the car is to the pit entry, along the pit road and into the stall.
    pub fn pit_in(&mut self) -> &mut Self {
        while self.sim.pct < PIT_ENTRY_PCT {
            self.advance(40.0, 0.0);
        }
        self.sim.on_pit_road = true;
        self.sim.loc = ON_PIT_ROAD;
        while self.sim.pct < STALL_PCT {
            self.advance(PIT_SPEED, 0.0);
        }
        self.sim.in_stall = true;
        self.sim.loc = IN_STALL;
        self.sim.speed = 0.0;
        self.hold(0.5)
    }

    /// Drives from where the car is along the pit road, past the stall without stopping, over
    /// the line and out of the pit road.
    pub fn drive_through(&mut self) -> &mut Self {
        while self.sim.pct < PIT_ENTRY_PCT {
            self.advance(40.0, 0.0);
        }
        self.sim.on_pit_road = true;
        self.sim.loc = ON_PIT_ROAD;
        let mut crossed = false;
        while !(crossed && self.sim.pct > PIT_EXIT_PCT) {
            crossed |= self.advance(PIT_SPEED, 0.0);
        }
        self.sim.on_pit_road = false;
        self.sim.loc = ON_TRACK;
        self
    }

    /// Stands `s` seconds in the stall taking `refuel_l` litres. With `readings` the setup's
    /// `UpdateCount` rises and its tyre readings change.
    pub fn stop(&mut self, s: f64, refuel_l: f64, readings: Option<TyreReadings>) -> &mut Self {
        let rows = (s * HZ).ceil() as u32;
        let per_row = refuel_l / f64::from(rows.max(1));
        for _ in 0..rows {
            self.sim.fuel += per_row;
            self.hold_rows(1);
        }
        if let Some(r) = readings {
            let count = self.yaml.setup_update_count().unwrap_or(0) + 1;
            self.yaml = self.yaml.clone().setup(Some(count), Some(r));
            self.push_update(false);
        }
        self
    }

    /// Leaves the car for the garage and waits half a second.
    pub fn to_garage(&mut self) -> &mut Self {
        self.put_in_garage();
        self.hold(0.5)
    }

    /// Starts session `num` of type `kind`: the text changes, the unique id rises and the car
    /// is in the garage.
    pub fn session_change(&mut self, num: i32, kind: &str) -> &mut Self {
        self.yaml = self.yaml.clone().session(num, kind);
        self.push_update(false);
        let sim = &mut self.sim;
        sim.session_num = num;
        sim.unique_id += 1;
        sim.state = 4;
        sim.flags = session_flags::GREEN;
        sim.lap = 0;
        sim.completed = 0;
        sim.pct = STALL_PCT;
        sim.last_time = -1.0;
        self.put_in_garage();
        self.hold(0.5)
    }

    /// Changes the session text during the drive (cars join, a slot is reused): `edit` gets
    /// the current builder and the new text is published at the next row.
    pub fn update_yaml(&mut self, edit: impl FnOnce(YamlBuilder) -> YamlBuilder) -> &mut Self {
        self.yaml = edit(self.yaml.clone());
        self.push_update(false);
        self
    }

    /// Starts a new event: the layout and the text restart, as when the simulator loads
    /// another session.
    pub fn new_event(&mut self, yaml: YamlBuilder) -> &mut Self {
        self.length_m = yaml.track_length_m();
        self.player = usize::try_from(yaml.player()).expect("player slot");
        self.yaml = yaml;
        self.push_update(true);
        let mut sim = Sim::new(self.yaml.current_session());
        sim.t = self.sim.t;
        sim.unique_id = self.sim.unique_id + 1;
        self.sim = sim;
        self.hold(0.5)
    }

    /// Raises `bits` of `SessionFlags` for `s` seconds while the car keeps going.
    pub fn flags(&mut self, bits: u32, s: f64) -> &mut Self {
        self.sim.flags |= bits;
        self.run(s);
        self.sim.flags &= !bits;
        self
    }

    /// A caution of `s` seconds: yellow and caution flags, pits closed.
    pub fn caution(&mut self, s: f64) -> &mut Self {
        self.sim.pits_open = false;
        self.flags(
            session_flags::YELLOW | session_flags::CAUTION | session_flags::CAUTION_WAVING,
            s,
        );
        self.sim.pits_open = true;
        self
    }

    /// Puts rival `car` one lap ahead just behind the player and shows the blue flag for
    /// three seconds.
    pub fn blue(&mut self, car: usize) -> &mut Self {
        let (lap, completed) = (self.sim.lap + 1, self.sim.completed + 1);
        let pct = (self.sim.pct - 0.01).rem_euclid(1.0);
        let c = &mut self.sim.cars[car];
        c.active = true;
        c.loc = ON_TRACK;
        c.lap = lap;
        c.completed = completed;
        c.pct = pct;
        self.flags(session_flags::BLUE, 3.0)
    }

    /// Shows a car `side` of the player for `s` seconds.
    pub fn alongside(&mut self, side: Side, s: f64) -> &mut Self {
        self.sim.car_lr = if side == Side::Left { 2 } else { 3 };
        self.run(s);
        self.sim.car_lr = 1;
        self
    }

    /// Holds `CarLeftRight` at `word` (1 clear, 2 left, 3 right, 4 both) for `s` seconds while
    /// the car keeps going, then back to clear.
    pub fn car_left_right(&mut self, word: i32, s: f64) -> &mut Self {
        self.sim.car_lr = word;
        self.run(s);
        self.sim.car_lr = 1;
        self
    }

    /// Sets `SessionState` (3 is the parade laps, 4 racing) from the next row on.
    pub fn session_state(&mut self, state: i32) -> &mut Self {
        self.sim.state = state;
        self
    }

    /// The chequered flag: the race ends with the player in position `pos`.
    pub fn chequered(&mut self, pos: i32) -> &mut Self {
        self.sim.state = 5;
        self.sim.flags |= session_flags::CHECKERED;
        self.sim.position = pos;
        self.run(2.0)
    }

    /// The simulator is gone for `s` seconds: no rows, the session clock moves on.
    pub fn disconnect(&mut self, s: f64) -> &mut Self {
        self.gaps.push((self.rows.len(), s));
        self.sim.t += s;
        self
    }

    // ---- the field ----------------------------------------------------------------------

    /// Puts `n` rivals on the track, in the first real slots other than the player's, spread
    /// along the lap and ahead of the player in position.
    pub fn field(&mut self, n: usize) -> &mut Self {
        let slots: Vec<usize> = self
            .yaml
            .slots()
            .into_iter()
            .map(|i| i as usize)
            .filter(|i| *i != self.player)
            .take(n)
            .collect();
        for (k, idx) in slots.into_iter().enumerate() {
            self.sim.cars[idx] = Car {
                active: true,
                loc: ON_TRACK,
                lap: 0,
                completed: 0,
                pct: (STALL_PCT + 0.01 * (k as f64 + 1.0)).rem_euclid(1.0),
                position: k as i32 + 2,
                ..Car::OFF
            };
        }
        self
    }

    /// A rival to script.
    pub fn rival(&mut self, idx: usize) -> Rival<'_> {
        Rival { s: self, idx }
    }

    // ---- internals ----------------------------------------------------------------------

    fn schedule(&mut self, at_pct: f64, dur_s: f64, what: Scheduled) -> &mut Self {
        self.sim.events.push(Event {
            at_pct,
            dur_s,
            what,
        });
        self
    }

    fn put_in_garage(&mut self) {
        let sim = &mut self.sim;
        sim.in_garage = true;
        sim.in_stall = false;
        sim.on_pit_road = false;
        sim.on_track = false;
        sim.loc = NOT_IN_WORLD;
        sim.speed = 0.0;
    }

    fn push_update(&mut self, restart: bool) {
        self.updates.push(YamlUpdate {
            at_row: self.rows.len(),
            text: self.yaml.build_cp1252(),
            restart,
        });
    }

    fn hold(&mut self, s: f64) -> &mut Self {
        self.hold_rows((s * HZ).ceil() as u32);
        self
    }

    /// Rows with the car where it is.
    fn hold_rows(&mut self, rows: u32) {
        for _ in 0..rows {
            self.sim.t += DT;
            self.finish_row();
        }
    }

    /// `s` seconds at the speed the car has, along the track.
    fn run(&mut self, s: f64) -> &mut Self {
        let speed = self.sim.speed.max(self.length_m / 100.0);
        for _ in 0..(s * HZ).ceil() as u32 {
            self.advance(speed, 0.0);
        }
        self
    }

    /// One row of driving at `speed` m/s burning `fuel_per_lap` litres per lap; whether the
    /// car crossed the line.
    fn advance(&mut self, speed: f64, fuel_per_lap: f64) -> bool {
        let prev = self.sim.pct;
        let sim = &mut self.sim;
        sim.speed = speed;
        sim.t += DT;
        let step = speed * DT / self.length_m;
        sim.pct += step;
        sim.fuel -= fuel_per_lap * step;
        let wrapped = sim.pct >= 1.0;
        if wrapped {
            sim.pct -= 1.0;
            sim.lap += 1;
            sim.completed += 1;
            sim.lap_start_t = sim.t;
            if let Some((time, delay)) = sim.official.take() {
                sim.pending_last = Some((delay, time));
            }
        }
        self.trigger_events(prev, wrapped);
        self.finish_row();
        wrapped
    }

    fn trigger_events(&mut self, prev: f64, wrapped: bool) {
        let now = self.sim.pct;
        let mut fired = Vec::new();
        self.sim.events.retain(|e| {
            let crossed = if wrapped {
                e.at_pct > prev || e.at_pct <= now
            } else {
                prev < e.at_pct && e.at_pct <= now
            };
            if crossed {
                fired.push(*e);
            }
            !crossed
        });
        for e in fired {
            match e.what {
                Scheduled::Effect(kind) => self.sim.effects.push((kind, self.sim.t + e.dur_s)),
                Scheduled::Incident { points, report } => {
                    self.sim.incidents += points;
                    self.sim.incident_word = incident_word(points, report);
                }
            }
        }
    }

    /// Settles the per-row state and writes the row.
    fn finish_row(&mut self) {
        let t = self.sim.t;
        self.sim.effects.retain(|(_, until)| *until > t);
        if let Some((left, value)) = self.sim.pending_last {
            if left == 0 {
                self.sim.last_time = value;
                if self.sim.best_time < 0.0 || value < self.sim.best_time {
                    self.sim.best_time = value;
                }
                self.sim.pending_last = None;
            } else {
                self.sim.pending_last = Some((left - 1, value));
            }
        }
        let row = self.write_row();
        self.sim.incident_word = 0;
        self.rows.push(row);
    }

    fn write_row(&self) -> Vec<u8> {
        let s = &self.sim;
        let (mut loc, mut material) = (s.loc, s.material);
        for (kind, _) in &s.effects {
            match kind {
                Effect::OffTrack => {
                    loc = OFF_TRACK;
                    material = GRASS;
                }
                Effect::Cut => material = GRASS,
            }
        }
        let moving = s.speed > 1.0;
        let curve = (s.pct * std::f64::consts::TAU * 5.0).sin();
        let lat = (6.0 * curve) as f32;
        let mut b = RowBuilder::new(&self.layout);
        b.set_f64("SessionTime", s.t)
            .set_i32("SessionNum", s.session_num)
            .set_i32("SessionUniqueID", s.unique_id)
            .set_i32("SessionState", s.state)
            .set_u32("SessionFlags", s.flags)
            .set_f64("SessionTimeRemain", (3600.0 - s.t).max(0.0))
            .set_i32("SessionLapsRemainEx", 32767)
            .set_f64("SessionTimeTotal", 3600.0)
            .set_i32("SessionLapsTotal", 32767)
            .set_bool("IsOnTrack", s.on_track)
            .set_bool("IsOnTrackCar", s.on_track)
            .set_bool("IsInGarage", s.in_garage)
            .set_bool("IsReplayPlaying", false)
            .set_i32("PlayerCarIdx", self.player as i32)
            .set_i32("CamCarIdx", self.player as i32)
            .set_i32("Lap", s.lap)
            .set_i32("LapCompleted", s.completed)
            .set_f32("LapDist", (s.pct * self.length_m) as f32)
            .set_f32("LapDistPct", s.pct as f32)
            .set_f32("LapCurrentLapTime", (s.t - s.lap_start_t) as f32)
            .set_f32("LapLastLapTime", s.last_time)
            .set_f32("LapBestLapTime", s.best_time)
            .set_f32("Speed", s.speed as f32)
            .set_f32("RPM", (1000.0 + 60.0 * s.speed) as f32)
            .set_i32(
                "Gear",
                if moving {
                    ((s.speed / 12.0).ceil() as i32).min(6)
                } else {
                    0
                },
            )
            .set_f32("SteeringWheelAngle", lat * 0.05)
            .set_f32("SteeringWheelAngleMax", 7.0)
            .set_f32("LatAccel", lat)
            .set_f32("LongAccel", 0.0)
            .set_f32("VertAccel", 9.8)
            .set_f32("VelocityX", s.speed as f32)
            .set_f32("VelocityY", 0.0)
            .set_f32("YawNorth", (s.pct * std::f64::consts::TAU) as f32)
            .set_f32("FuelLevel", s.fuel as f32)
            .set_bool("OnPitRoad", s.on_pit_road)
            .set_bool("PlayerCarInPitStall", s.in_stall)
            .set_bool("IsGarageVisible", s.in_garage)
            .set_i32("ReplayFrameNumEnd", 1)
            .set_i32("PlayerTrackSurface", loc)
            .set_i32("PlayerTrackSurfaceMaterial", material)
            .set_i32("PlayerCarMyIncidentCount", s.incidents)
            .set_u32("PlayerIncidents", s.incident_word)
            .set_f32("PlayerCarTowTime", s.tow)
            .set_i32("PlayerCarPosition", s.position)
            .set_i32("PlayerCarClassPosition", s.position)
            .set_bool("PitsOpen", s.pits_open)
            .set_i32("PaceMode", 4)
            .set_i32("CarLeftRight", s.car_lr)
            .set_f32("AirTemp", 21.0)
            .set_f32("TrackTempCrew", 30.0)
            .set_f32("Precipitation", 0.0)
            .set_f32("WindVel", 2.0);
        for name in ["ThrottleRaw", "Throttle"] {
            b.set_f32(name, if moving { 0.7 } else { 0.0 });
        }
        for name in ["BrakeRaw", "Brake", "ClutchRaw", "Clutch"] {
            b.set_f32(name, 0.0);
        }
        for wheel in ["LF", "RF", "LR", "RR"] {
            for pos in ["L", "M", "R"] {
                b.set_f32(&format!("{wheel}tempC{pos}"), 75.0)
                    .set_f32(&format!("{wheel}wear{pos}"), 1.0);
            }
        }
        for (i, c) in s.cars.iter().enumerate() {
            let me = i == self.player;
            let (lap, completed, pct, pit, car_loc, last, best, flags, pos) = if me {
                (
                    s.lap,
                    s.completed,
                    s.pct as f32,
                    s.on_pit_road,
                    loc,
                    s.last_time,
                    s.best_time,
                    s.flags,
                    s.position,
                )
            } else {
                (
                    c.lap,
                    c.completed,
                    c.pct as f32,
                    c.on_pit_road,
                    c.loc,
                    c.last,
                    c.best,
                    c.flags,
                    c.position,
                )
            };
            if !me && !c.active {
                b.set_i32_at("CarIdxLap", i, -1)
                    .set_i32_at("CarIdxLapCompleted", i, -1)
                    .set_f32_at("CarIdxLapDistPct", i, -1.0)
                    .set_i32_at("CarIdxTrackSurface", i, NOT_IN_WORLD);
                continue;
            }
            b.set_i32_at("CarIdxLap", i, lap)
                .set_i32_at("CarIdxLapCompleted", i, completed)
                .set_f32_at("CarIdxLapDistPct", i, pct)
                .set_bool_at("CarIdxOnPitRoad", i, pit)
                .set_i32_at("CarIdxTrackSurface", i, car_loc)
                .set_i32_at("CarIdxPosition", i, pos)
                .set_i32_at("CarIdxClassPosition", i, pos)
                .set_f32_at("CarIdxLastLapTime", i, last)
                .set_f32_at("CarIdxBestLapTime", i, best)
                .set_u32_at("CarIdxSessionFlags", i, flags);
        }
        b.into_bytes()
    }
}

fn incident_word(points: i32, report: u32) -> u32 {
    let penalty = match points {
        0 => incident_flags::PENALTY_0X,
        1 => incident_flags::PENALTY_1X,
        2 => incident_flags::PENALTY_2X,
        _ => incident_flags::PENALTY_4X,
    };
    report | penalty
}

/// A rival car of a [`Scenario`]: its changes show in the next row.
pub struct Rival<'a> {
    s: &'a mut Scenario,
    idx: usize,
}

impl Rival<'_> {
    fn car(&mut self) -> &mut Car {
        &mut self.s.sim.cars[self.idx]
    }

    /// Finishes a lap of `time_s` seconds: the line is crossed and the times update.
    pub fn lap(&mut self, time_s: f32) {
        let c = self.car();
        c.active = true;
        if c.loc == NOT_IN_WORLD {
            c.loc = ON_TRACK;
        }
        c.lap += 1;
        c.completed += 1;
        c.pct = 0.0;
        c.last = time_s;
        if c.best < 0.0 || time_s < c.best {
            c.best = time_s;
        }
    }

    /// Enters the pit road.
    pub fn pit(&mut self) {
        let c = self.car();
        c.active = true;
        c.on_pit_road = true;
        c.loc = ON_PIT_ROAD;
    }

    /// Leaves the pit road.
    pub fn leave_pits(&mut self) {
        let c = self.car();
        c.on_pit_road = false;
        c.loc = ON_TRACK;
    }
}
