//! The hot path allocates nothing: filling a tick, feeding the lap pieces, the announcement
//! detector and the spotter (while nothing fires), row after row.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

use iracing_core::announce::Calls;
use iracing_core::announce::detector::Detector;
use iracing_core::announce::spotter::Spotter;
use iracing_core::codes::SessionKind;
use iracing_core::lap::{LapBuffer, LapPoint};
use iracing_core::meta::SessionMeta;
use iracing_core::sectors::SectorClock;
use iracing_core::testing::{LapProfile, Scenario, YamlBuilder, table_of};
use iracing_core::tick::Tick;
use iracing_core::validity::{IncidentTracker, Validity};
use iracing_core::vars::Vars;
use irsdk::row::Row;

/// Counts the allocations made by the thread that asked for it.
struct Counting;

thread_local! {
    static ALLOCATIONS: Cell<usize> = const { Cell::new(0) };
}

fn bump() {
    // `try_with` so an allocation during thread teardown cannot panic.
    let _ = ALLOCATIONS.try_with(|c| c.set(c.get() + 1));
}

// SAFETY: every call is forwarded unchanged to the system allocator.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        bump();
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        bump();
        unsafe { System.alloc_zeroed(layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        bump();
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static ALLOC: Counting = Counting;

fn allocations() -> usize {
    ALLOCATIONS.with(Cell::get)
}

#[test]
fn filling_a_tick_and_feeding_the_lap_pieces_allocates_nothing() {
    let mut s = Scenario::new(YamlBuilder::new());
    s.garage(1.0).to_stall().drive_out();
    s.lap(LapProfile::new(8.0));
    s.lap(LapProfile::new(8.0));
    let table = table_of(s.layout());
    let vars = Vars::resolve(&table).expect("vars");
    let rows: Vec<Row> = s.rows().iter().map(|b| Row::from_slice(b)).collect();
    assert!(rows.len() > 600);

    let mut tick = Tick::new();
    let mut buffer = LapBuffer::new();
    let mut sectors = SectorClock::new(&[0.0, 0.3, 0.7]);
    let mut incidents = IncidentTracker::new();
    let mut validity = Validity::new();
    let mut detector = Detector::new(2.0);
    let mut spotter = Spotter::new();
    let mut calls = Calls::new();
    let mut meta = SessionMeta::empty(0);
    meta.kind = SessionKind::Race;
    meta.track.length_m = Some(4010.0);
    sectors.start_lap(0.0, false);
    let point = LapPoint {
        timed: true,
        lap_clock_s: 1.0,
        valid: true,
    };

    let before = allocations();
    let mut prev_pct = 0.0;
    let mut prev_t = 0.0;
    for row in &rows {
        tick.fill(&vars, row);
        let pct = f64::from(tick.lap_dist_pct);
        sectors.step(prev_pct, pct, prev_t, tick.session_time);
        prev_pct = pct;
        prev_t = tick.session_time;
        let rise = incidents.step(&tick);
        validity.step(&tick, tick.is_on_track, 1.0, rise.as_ref());
        buffer.push(&tick, &point, 0);
        detector.step(&tick, Some(&meta), &mut calls);
        spotter.step(&tick, Some(&meta), Some(0.0), &mut calls);
    }
    let after = allocations();
    assert_eq!(after - before, 0, "allocations on the hot path");
    assert_eq!(buffer.len(), rows.len());
    assert!(calls.is_empty());
    // The rows carry velocity and a heading: the position was integrated inside the counted
    // region.
    assert!(buffer.present().position);
    assert!(buffer.columns().z.iter().any(|v| *v != 0.0));
}

/// A sink that counts and keeps nothing.
struct Count(usize);

impl iracing_core::Sink for Count {
    fn start_session(&mut self, _: &iracing_core::SessionOpenInfo) -> u64 {
        self.0 += 1;
        1
    }
    fn end_session(&mut self, _: u64, _: &str) {
        self.0 += 1;
    }
    fn lap(&mut self, _: u64, _: iracing_core::FinishedLap) {
        self.0 += 1;
    }
    fn setup(
        &mut self,
        _: u64,
        _: iracing_core::SetupReason,
        _: &str,
        _: std::sync::Arc<SessionMeta>,
        _: f32,
    ) {
        self.0 += 1;
    }
    fn pit_exit(&mut self, _: u64, _: &str, _: Option<f64>) {
        self.0 += 1;
    }
    fn participants(&mut self, _: u64, _: Vec<iracing_core::ParticipantLap>) {
        self.0 += 1;
    }
    fn catalog(&mut self, _: std::sync::Arc<SessionMeta>) {
        self.0 += 1;
    }
}

struct Wall;

impl iracing_core::Clock for Wall {
    fn iso(&self) -> String {
        "2026-01-01T00:00:00Z".to_owned()
    }
    fn mono_s(&self) -> f64 {
        0.0
    }
}

#[test]
fn the_tracker_step_allocates_nothing_while_no_lap_ends() {
    let mut s = Scenario::new(YamlBuilder::new());
    s.garage(1.0).to_stall().drive_out();
    let warm = s.rows().len() + 30;
    s.lap(LapProfile::new(12.0));
    s.lap(LapProfile::new(12.2));
    let table = table_of(s.layout());
    let vars = Vars::resolve(&table).expect("vars");
    let rows: Vec<Row> = s.rows().iter().map(|b| Row::from_slice(b)).collect();
    let text = s.initial_yaml().to_vec();
    let meta = std::sync::Arc::new(iracing_core::meta::build(
        &irsdk::worker::SessionDoc::from_raw(1, &text),
        0,
    ));

    let mut tick = Tick::new();
    let mut tracker = iracing_core::tracker::Tracker::new(2048);
    let (mut sink, clock) = (Count(0), Wall);
    // The session opens and the first lap starts: those allocate.
    for row in &rows[..warm] {
        tick.fill(&vars, row);
        tick.connected = true;
        tick.fresh = true;
        tracker.step(&tick, Some(&meta), &mut sink, &clock);
    }
    let laps = sink.0;

    // About four seconds of a twelve second lap.
    let before = allocations();
    for row in &rows[warm..warm + 240] {
        tick.fill(&vars, row);
        tick.connected = true;
        tick.fresh = true;
        tracker.step(&tick, Some(&meta), &mut sink, &clock);
    }
    let after = allocations();
    assert_eq!(sink.0, laps, "no call to the sink in the window");
    assert_eq!(after - before, 0, "allocations in the tracker step");
    assert_eq!(tracker.view().state, iracing_core::tracker::State::Track);
}

/// A sink that counts laps and hands back buffers made beforehand, as the writer thread does.
struct Pooled {
    laps: usize,
    pool: Vec<LapBuffer>,
}

impl iracing_core::Sink for Pooled {
    fn start_session(&mut self, _: &iracing_core::SessionOpenInfo) -> u64 {
        1
    }
    fn end_session(&mut self, _: u64, _: &str) {}
    fn lap(&mut self, _: u64, _: iracing_core::FinishedLap) {
        self.laps += 1;
    }
    fn setup(
        &mut self,
        _: u64,
        _: iracing_core::SetupReason,
        _: &str,
        _: std::sync::Arc<SessionMeta>,
        _: f32,
    ) {
    }
    fn pit_exit(&mut self, _: u64, _: &str, _: Option<f64>) {}
    fn participants(&mut self, _: u64, _: Vec<iracing_core::ParticipantLap>) {}
    fn catalog(&mut self, _: std::sync::Arc<SessionMeta>) {}
    fn spare_buffer(&mut self, _: usize) -> Option<LapBuffer> {
        self.pool.pop()
    }
}

#[test]
fn a_lap_end_allocates_nothing_when_the_sink_refills_the_pool() {
    let mut s = Scenario::new(YamlBuilder::new());
    s.garage(1.0).to_stall().drive_out();
    let warm = s.rows().len() + 30;
    s.lap(LapProfile::new(12.0));
    s.lap(LapProfile::new(12.2));
    s.lap(LapProfile::new(12.1));
    let table = table_of(s.layout());
    let vars = Vars::resolve(&table).expect("vars");
    let rows: Vec<Row> = s.rows().iter().map(|b| Row::from_slice(b)).collect();
    let text = s.initial_yaml().to_vec();
    let meta = std::sync::Arc::new(iracing_core::meta::build(
        &irsdk::worker::SessionDoc::from_raw(1, &text),
        0,
    ));
    let mut tick = Tick::new();
    let mut tracker = iracing_core::tracker::Tracker::new(2048);
    let mut sink = Pooled {
        laps: 0,
        pool: (0..4).map(|_| LapBuffer::with_capacity(2048)).collect(),
    };
    let clock = Wall;
    for row in &rows[..warm] {
        tick.fill(&vars, row);
        tick.connected = true;
        tick.fresh = true;
        tracker.step(&tick, Some(&meta), &mut sink, &clock);
    }
    // Two lap ends and their sends (the out lap, the first flying lap), one after the other.
    let before = allocations();
    for row in &rows[warm..] {
        tick.fill(&vars, row);
        tick.connected = true;
        tick.fresh = true;
        tracker.step(&tick, Some(&meta), &mut sink, &clock);
    }
    let after = allocations();
    assert!(sink.laps >= 2, "laps in the window: {}", sink.laps);
    assert_eq!(after - before, 0, "allocations across lap ends");
}

#[test]
fn the_rival_step_allocates_nothing_while_no_lap_is_counted() {
    let mut s = Scenario::new(YamlBuilder::new().roster(63, 2));
    s.field(62);
    s.garage(1.0).to_stall().drive_out();
    s.lap(LapProfile::new(8.0));
    let table = table_of(s.layout());
    let vars = Vars::resolve(&table).expect("vars");
    let rows: Vec<Row> = s.rows().iter().map(|b| Row::from_slice(b)).collect();
    let mut meta =
        iracing_core::meta::build(&irsdk::worker::SessionDoc::from_raw(1, s.initial_yaml()), 0);
    meta.sector_starts = vec![0.0, 0.3, 0.7];
    let mut rivals = iracing_core::rivals::RivalTracker::new();
    let mut tick = Tick::new();
    tick.connected = true;
    tick.fresh = true;
    // The first rows set each car's baseline (and the sector clocks); the player's own
    // crossings in the scenario would count a lap, so only rows before the first are timed.
    let steady = rows.len() / 3;
    for row in &rows[..steady] {
        tick.fill(&vars, row);
        rivals.step(&tick, &meta);
    }
    let mut idle = 0;
    let mut counted = 0;
    for row in &rows[steady..] {
        tick.fill(&vars, row);
        let before = allocations();
        rivals.step(&tick, &meta);
        let spent = allocations() - before;
        // A step that counted a lap (the player's own crossing) builds its record.
        if rivals.take_due(1e9, true).is_some() {
            counted += 1;
        } else {
            idle += spent;
        }
    }
    assert_eq!(
        idle, 0,
        "allocations in the rival step while no lap is counted"
    );
    assert!(counted >= 1);
}
