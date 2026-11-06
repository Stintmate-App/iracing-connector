//! Shared by the end-to-end tests: a recorder process reading a test mapping that a replay
//! thread fills, writing into a fake Pitwall, with unique names and a temporary workspace.
#![allow(dead_code)]

use iracing_core::SPEC;
use iracing_core::testing::{LapProfile, Scenario, YamlBuilder};
use irsdk::testing::map::TestMap;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use stintmate_connector_platform::single::{Names, signal_stop};
use stintmate_connector_testing::{FakePitwall, TempWorkspace, pitwall_pipe, unique_user};

pub struct Kill(pub Child);
impl Drop for Kill {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

pub fn wait_for(secs: u64, mut f: impl FnMut() -> bool) -> bool {
    let end = Instant::now() + Duration::from_secs(secs);
    while Instant::now() < end {
        if f() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    f()
}

pub struct Rig {
    pub fake: FakePitwall,
    pub ws: TempWorkspace,
    pub tag: String,
    pub map: String,
    pub event: String,
    pub pipe: String,
}

pub fn rig() -> Rig {
    let user = unique_user();
    let tag = format!(
        "t{}",
        user.replace(|c: char| !c.is_ascii_alphanumeric(), "")
    );
    let ws = TempWorkspace::new();
    std::fs::write(
        ws.path().join("stintmate.json"),
        r#"{"schema": 1, "created_at": "2026-11-05T10:00:00+01:00"}"#,
    )
    .unwrap();
    let pipe = pitwall_pipe(&user);
    let fake = FakePitwall::start(&pipe);
    Rig {
        fake,
        ws,
        map: format!("stintmate_test_{tag}_map"),
        event: format!("stintmate_test_{tag}_event"),
        tag,
        pipe,
    }
}

impl Rig {
    pub fn spawn(&self) -> Kill {
        let child = Command::new(env!("CARGO_BIN_EXE_iracing-recorder"))
            .args(["--workspace", self.ws.path().to_str().unwrap()])
            .args(["--pitwall-pipe", &self.pipe])
            .args(["--sdk-map", &self.map, "--sdk-event", &self.event])
            .env("STINTMATE_DEV_INSTANCE", &self.tag)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("start the recorder");
        Kill(child)
    }

    /// Asks the recorder to stop and returns its exit code, waiting at most 10 s.
    pub fn stop(&self, child: &mut Kill) -> Option<i32> {
        assert!(signal_stop(&Names::tagged(&SPEC, &self.tag).recorder_stop));
        let end = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(s) = child.0.try_wait().unwrap() {
                return s.code();
            }
            assert!(Instant::now() < end, "the recorder did not stop");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    pub fn data_dir(&self) -> std::path::PathBuf {
        self.ws.path().join("connectors").join("iracing")
    }

    pub fn count_suffix(&self, suffix: &str) -> usize {
        self.fake
            .calls()
            .iter()
            .filter(|c| c.method.ends_with(suffix))
            .count()
    }
}

/// The recorder's connection in the fake Pitwall, once it is up.
pub fn peer(rig: &Rig) -> stintmate_ipc::blocking::Peer {
    assert!(wait_for(15, || rig.fake.peer().is_some()), "no hello");
    rig.fake.peer().unwrap()
}

/// A call from the fake Pitwall to the recorder, answered as JSON.
pub fn call(
    rig: &Rig,
    method: &str,
    params: serde_json::Value,
) -> Result<serde_json::Value, stintmate_ipc::Error> {
    peer(rig)
        .call(
            method,
            stintmate_connector_link::link::to_value(&params),
            stintmate_ipc::connection::CallOptions::default(),
        )
        .map(|v| stintmate_connector_testing::to_json(&v))
}

/// The code of a refused call.
pub fn remote_code(r: Result<serde_json::Value, stintmate_ipc::Error>) -> String {
    match r {
        Err(stintmate_ipc::Error::Remote(e)) => e.code,
        other => panic!("expected a remote error, got {other:?}"),
    }
}

/// A session with `laps` laps of `lap_s` seconds, after the garage and the pit exit.
pub fn scenario(laps: u32, lap_s: f64) -> Scenario {
    let mut s = Scenario::new(YamlBuilder::new());
    s.garage(0.5).to_stall().drive_out();
    for _ in 0..laps {
        s.lap(LapProfile::new(lap_s));
    }
    s
}

/// The test mapping filled by a replay thread at 60 rows per second.
pub struct Replay {
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<usize>>,
}

impl Replay {
    pub fn start(rig: &Rig, s: Scenario) -> Replay {
        let size = TestMap::required_size(s.layout(), s.initial_yaml().len() + 64 * 1024);
        let map = TestMap::create(&rig.map, &rig.event, s.layout(), s.initial_yaml(), size)
            .expect("test mapping");
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let handle = std::thread::spawn(move || {
            let mut map = map;
            s.replay_into(&mut map, 60.0, &flag)
        });
        Replay {
            stop,
            handle: Some(handle),
        }
    }
}

impl Drop for Replay {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}
