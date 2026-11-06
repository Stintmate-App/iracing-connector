//! The loop allocates nothing per row while driving, live frame, telemetry ring, detectors and
//! spotter included: a
//! counting allocator on the loop thread
//! only, over a test mapping filled by a replay.

use iracing_core::testing::{FixedClock, LapProfile, RecordingSink, Scenario, YamlBuilder};
use iracing_core::tracker::State;
use iracing_core::{FinishedLap, ParticipantLap, SessionOpenInfo, SetupReason, Sink};
use iracing_recorder::announce;
use iracing_recorder::cli::SdkNames;
use iracing_recorder::live::LiveBuffer;
use iracing_recorder::recorder::{Pipeline, Reader};
use iracing_recorder::sink::NoteClock;
use iracing_recorder::telemetry::Ring;
use irsdk::sampler::Poll;
use irsdk::testing::map::TestMap;
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

struct Counting;
thread_local! {
    static COUNT: Cell<bool> = const { Cell::new(false) };
    static N: Cell<u64> = const { Cell::new(0) };
}
// SAFETY: forwards to the system allocator and only bumps a thread-local counter.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        if COUNT.with(Cell::get) {
            N.with(|n| n.set(n.get() + 1));
        }
        unsafe { System.alloc(l) }
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        unsafe { System.dealloc(p, l) }
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 {
        if COUNT.with(Cell::get) {
            N.with(|c| c.set(c.get() + 1));
        }
        unsafe { System.realloc(p, l, n) }
    }
}
#[global_allocator]
static A: Counting = Counting;

struct Rec(RecordingSink);
impl NoteClock for Rec {}
impl Sink for Rec {
    fn start_session(&mut self, o: &SessionOpenInfo) -> u64 {
        self.0.start_session(o)
    }
    fn end_session(&mut self, l: u64, e: &str) {
        self.0.end_session(l, e)
    }
    fn lap(&mut self, l: u64, lap: FinishedLap) {
        self.0.lap(l, lap)
    }
    fn setup(
        &mut self,
        l: u64,
        r: SetupReason,
        a: &str,
        m: Arc<iracing_core::meta::SessionMeta>,
        f: f32,
    ) {
        self.0.setup(l, r, a, m, f)
    }
    fn pit_exit(&mut self, l: u64, a: &str, f: Option<f64>) {
        self.0.pit_exit(l, a, f)
    }
    fn participants(&mut self, l: u64, r: Vec<ParticipantLap>) {
        self.0.participants(l, r)
    }
    fn catalog(&mut self, m: Arc<iracing_core::meta::SessionMeta>) {
        self.0.catalog(m)
    }
}

#[test]
fn the_loop_allocates_nothing_per_row_while_driving() {
    let mut s = Scenario::new(YamlBuilder::new());
    s.garage(0.5).to_stall().drive_out();
    s.lap(LapProfile::new(40.0));
    let tag = std::process::id();
    let names = SdkNames {
        map: format!("stintmate_alloc_{tag}_map"),
        event: format!("stintmate_alloc_{tag}_event"),
    };
    let size = TestMap::required_size(s.layout(), s.initial_yaml().len() + 64 * 1024);
    let mut map =
        TestMap::create(&names.map, &names.event, s.layout(), s.initial_yaml(), size).unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let flag = stop.clone();
    let replay = std::thread::spawn(move || s.replay_into(&mut map, 60.0, &flag));

    let mut reader = Reader::open(Some(&names)).expect("reader");
    let mut pipe = Pipeline::new(
        Rec(RecordingSink::default()),
        Box::new(FixedClock::default()),
    );
    // The live frame and a subscribed telemetry stream are part of the loop.
    let live = Arc::new(LiveBuffer::default());
    let ring = Arc::new(Ring::default());
    ring.set_subscribed(true);
    pipe.live = Some(live.clone());
    pipe.ring = Some(ring.clone());
    // The detectors and the spotter run on every row; with events quiet they push nothing.
    let (outlet, fired) = announce::channel();
    pipe.announce = Some(outlet);
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut driving = 0;
    while driving < 60 && Instant::now() < deadline {
        reader.poll(Duration::from_millis(25), &mut pipe);
        while ring.pop().is_some() {}
        if pipe.tracker.view().state == State::Track {
            driving += 1;
        }
    }
    assert!(driving >= 60, "the replay never reached the track");
    let (mut rows, mut allocs, mut samples) = (0u32, 0u64, 0u32);
    let end = Instant::now() + Duration::from_secs(5);
    while rows < 200 && Instant::now() < end {
        N.with(|n| n.set(0));
        COUNT.with(|c| c.set(true));
        let poll = reader.poll(Duration::from_millis(25), &mut pipe);
        COUNT.with(|c| c.set(false));
        while ring.pop().is_some() {
            samples += 1;
        }
        if poll == Poll::Row {
            rows += 1;
            allocs += N.with(Cell::get);
        }
    }
    stop.store(true, Ordering::Relaxed);
    let _ = replay.join();
    assert!(rows >= 150, "only {rows} rows were measured");
    assert!(samples >= rows, "{samples} samples for {rows} rows");
    assert_eq!(ring.dropped(), 0, "the ring was drained every row");
    assert!(fired.try_recv().is_err(), "a quiet drive fired a call");
    let mut snap = iracing_core::live::LiveSnapshot::empty();
    assert!(live.read(&mut snap), "the live frame was published");
    assert_eq!(
        allocs, 0,
        "the loop allocated {allocs} times in {rows} rows"
    );
}
