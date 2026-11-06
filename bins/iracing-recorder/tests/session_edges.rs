//! The recorder loop over a test mapping, with the records collected in memory: two events in a
//! row, and a stop in the middle of a lap that a second recorder resumes.

use iracing_core::testing::{FixedClock, LapProfile, RecordingSink, Scenario, YamlBuilder};
use iracing_core::tracker::State;
use iracing_core::{FinishedLap, ParticipantLap, SessionOpenInfo, SetupReason, Sink};
use iracing_recorder::cli::SdkNames;
use iracing_recorder::recorder::{Pipeline, Reader};
use iracing_recorder::sink::NoteClock;
use irsdk::testing::map::TestMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Default)]
struct Seen {
    /// The track and the car of every session opened, in order.
    starts: Vec<(String, String)>,
    /// The game's lap number and the partial flag of every lap sent.
    laps: Vec<(i32, bool)>,
}

struct Spy {
    inner: RecordingSink,
    seen: Arc<Mutex<Seen>>,
}

impl Spy {
    fn new() -> (Spy, Arc<Mutex<Seen>>) {
        let seen = Arc::new(Mutex::new(Seen::default()));
        let spy = Spy {
            inner: RecordingSink::default(),
            seen: Arc::clone(&seen),
        };
        (spy, seen)
    }
}

impl NoteClock for Spy {}
impl Sink for Spy {
    fn start_session(&mut self, o: &SessionOpenInfo) -> u64 {
        let car = o.meta.player.as_ref().map(|p| p.car_id.clone());
        self.seen
            .lock()
            .unwrap()
            .starts
            .push((o.meta.track.track_id.clone(), car.unwrap_or_default()));
        self.inner.start_session(o)
    }
    fn end_session(&mut self, l: u64, e: &str) {
        self.inner.end_session(l, e)
    }
    fn lap(&mut self, l: u64, lap: FinishedLap) {
        self.seen
            .lock()
            .unwrap()
            .laps
            .push((lap.sim_lap, lap.partial));
        self.inner.lap(l, lap)
    }
    fn setup(
        &mut self,
        l: u64,
        r: SetupReason,
        a: &str,
        m: Arc<iracing_core::meta::SessionMeta>,
        f: f32,
    ) {
        self.inner.setup(l, r, a, m, f)
    }
    fn pit_exit(&mut self, l: u64, a: &str, f: Option<f64>) {
        self.inner.pit_exit(l, a, f)
    }
    fn participants(&mut self, l: u64, r: Vec<ParticipantLap>) {
        self.inner.participants(l, r)
    }
    fn catalog(&mut self, m: Arc<iracing_core::meta::SessionMeta>) {
        self.inner.catalog(m)
    }
}

fn names(test: &str) -> SdkNames {
    let tag = std::process::id();
    SdkNames {
        map: format!("stintmate_edges_{test}_{tag}_map"),
        event: format!("stintmate_edges_{test}_{tag}_event"),
    }
}

/// A replay thread filling a mapping at 60 rows per second.
struct Feed {
    stop: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<usize>>,
}

impl Feed {
    fn start(names: &SdkNames, s: Scenario) -> Feed {
        let size = TestMap::required_size(s.layout(), s.initial_yaml().len() + 64 * 1024);
        let mut map = TestMap::create(&names.map, &names.event, s.layout(), s.initial_yaml(), size)
            .expect("test mapping");
        let stop = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&stop);
        let handle = std::thread::spawn(move || s.replay_into(&mut map, 60.0, &flag));
        Feed {
            stop,
            handle: Some(handle),
        }
    }

    fn done(&self) -> bool {
        self.handle.as_ref().is_none_or(|h| h.is_finished())
    }
}

impl Drop for Feed {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

fn pipe() -> (Pipeline<Spy>, Arc<Mutex<Seen>>) {
    let (spy, seen) = Spy::new();
    (Pipeline::new(spy, Box::new(FixedClock::default())), seen)
}

#[test]
fn a_new_event_never_opens_a_session_with_the_old_events_description() {
    let second = YamlBuilder::new()
        .track(262, "monza full", "Autodromo Nazionale Monza", "Grand Prix")
        .length_m(5793.0);
    let mut s = Scenario::new(YamlBuilder::new());
    s.garage(0.5).to_stall().drive_out();
    s.lap(LapProfile::new(6.0));
    s.new_event(second);
    s.garage(0.5).to_stall().drive_out();
    s.lap(LapProfile::new(6.0));

    let names = names("event");
    let feed = Feed::start(&names, s);
    let mut reader = Reader::open(Some(&names)).expect("reader");
    let (mut pipe, seen) = pipe();
    let deadline = Instant::now() + Duration::from_secs(60);
    while !feed.done() && Instant::now() < deadline {
        reader.poll(Duration::from_millis(25), &mut pipe);
    }
    // The last description may still be on its way.
    for _ in 0..40 {
        reader.poll(Duration::from_millis(25), &mut pipe);
    }
    let starts = seen.lock().unwrap().starts.clone();
    let tracks: Vec<&str> = starts.iter().map(|(t, _)| t.as_str()).collect();
    assert_eq!(
        tracks.len(),
        2,
        "one session for each event, not a third one when the text arrives: {starts:?}"
    );
    assert_ne!(tracks[0], tracks[1], "the second event has its own track");
    assert!(tracks[1].contains("monza"), "{starts:?}");
    assert!(!starts[1].1.is_empty(), "the car of the second event");
}

#[test]
fn a_lap_open_at_a_stop_is_written_once_by_the_recorder_that_resumes() {
    let mut s = Scenario::new(YamlBuilder::new());
    s.garage(0.5).to_stall().drive_out();
    for _ in 0..3 {
        s.lap(LapProfile::new(12.0));
    }
    let names = names("openlap");
    let _feed = Feed::start(&names, s);
    let deadline = Instant::now() + Duration::from_secs(60);

    let mut reader = Reader::open(Some(&names)).expect("reader");
    let (mut first, first_seen) = pipe();
    while first.tracker.view().laps_recorded < 1 && Instant::now() < deadline {
        reader.poll(Duration::from_millis(25), &mut first);
    }
    assert!(
        first.tracker.view().laps_recorded >= 1,
        "no lap before the stop"
    );
    // Five seconds into the next lap: the lap is long enough to be kept as a partial one, and so is what remains of it after the resume.
    let mid = Instant::now() + Duration::from_secs(5);
    while Instant::now() < mid {
        reader.poll(Duration::from_millis(25), &mut first);
    }
    assert_eq!(first.tracker.view().state, State::Track);
    first.stop(true);
    drop(reader);

    let mut reader = Reader::open(Some(&names)).expect("second reader");
    let (mut second, second_seen) = pipe();
    while second.tracker.view().laps_recorded < 1 && Instant::now() < deadline {
        reader.poll(Duration::from_millis(25), &mut second);
    }
    assert!(
        second.tracker.view().laps_recorded >= 1,
        "no lap after the resume"
    );

    let before = first_seen.lock().unwrap().laps.clone();
    let after = second_seen.lock().unwrap().laps.clone();
    assert!(before.iter().all(|(_, partial)| !partial), "{before:?}");
    let mut all: Vec<i32> = before.iter().chain(&after).map(|(n, _)| *n).collect();
    let rows = all.len();
    all.sort_unstable();
    all.dedup();
    assert_eq!(
        all.len(),
        rows,
        "one row for every game lap: {before:?} {after:?}"
    );
}
