//! The recorder against a fake Pitwall: it says hello, reports whether iRacing runs and
//! leaves within two seconds of the stop event, whether or not iRacing runs on the machine. A
//! unique development tag keeps its kernel objects apart from any real recorder's.

use iracing_core::SPEC;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use stintmate_connector_platform::single::{Names, signal_stop};
use stintmate_connector_testing::{FakePitwall, TempWorkspace, pitwall_pipe, to_json, unique_user};
use stintmate_ipc::blocking::Delivery;

const WAITING: &str = "Waiting for iRacing";
const RUNNING: &str = "iRacing is running; laps are not recorded by this version";

struct Kill(Child);
impl Drop for Kill {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn wait_for(secs: u64, mut f: impl FnMut() -> bool) -> bool {
    let end = Instant::now() + Duration::from_secs(secs);
    while Instant::now() < end {
        if f() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    f()
}

#[test]
fn the_recorder_says_hello_reports_the_status_and_stops() {
    if std::env::var_os("STINTMATE_SHARED_RUNNER").is_some() {
        return;
    }
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
    let child = Command::new(env!("CARGO_BIN_EXE_iracing-recorder"))
        .args(["--workspace", ws.path().to_str().unwrap()])
        .args(["--pitwall-pipe", &pipe])
        .env("STINTMATE_DEV_INSTANCE", &tag)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("start the recorder");
    let mut child = Kill(child);

    assert!(wait_for(15, || fake.peer().is_some()), "no hello");
    let peer = fake.peer().unwrap();
    let info = peer.info().clone();
    assert_eq!(info.app, "connector-iracing");
    assert_eq!(info.process.as_deref(), Some("recorder"));
    let meta = to_json(info.meta.as_ref().unwrap());
    assert_eq!(meta["connector_id"], "iracing");
    assert_eq!(meta["product"], SPEC.product);

    let last: Arc<Mutex<Option<serde_json::Value>>> = Arc::default();
    let l = last.clone();
    let _sub = peer
        .subscribe("connector.status", Default::default(), move |d| {
            if let Delivery::Event(ev) = d {
                *l.lock().unwrap() = Some(to_json(&ev.data));
            }
        })
        .expect("subscribe to connector.status");
    // Either state is right: iRacing may or may not run on the machine that runs this test.
    // Each state must come with its own message and phase.
    assert!(
        wait_for(10, || last.lock().unwrap().as_ref().is_some_and(
            |s| s["message"] == WAITING || s["message"] == RUNNING
        )),
        "no status: {:?}",
        last.lock().unwrap()
    );
    let status = last.lock().unwrap().clone().unwrap();
    if status["game_running"] == true {
        assert_eq!(status["message"], RUNNING);
        assert_eq!(status["phase"], "menus");
    } else {
        assert_eq!(status["game_running"], false);
        assert_eq!(status["message"], WAITING);
        assert!(status["phase"].is_null());
    }

    let stop = Names::tagged(&SPEC, &tag).recorder_stop;
    assert!(signal_stop(&stop), "the stop event exists");
    let end = Instant::now() + Duration::from_secs(2);
    let code = loop {
        if let Some(s) = child.0.try_wait().unwrap() {
            break s.code();
        }
        assert!(Instant::now() < end, "the recorder did not stop in 2 s");
        std::thread::sleep(Duration::from_millis(20));
    };
    assert_eq!(code, Some(0));
    fake.stop();
}

#[test]
fn an_unknown_option_is_exit_2() {
    let o = Command::new(env!("CARGO_BIN_EXE_iracing-recorder"))
        .arg("--nope")
        .output()
        .expect("run iracing-recorder");
    assert_eq!(o.status.code(), Some(2));
}
