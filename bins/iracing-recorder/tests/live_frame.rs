//! `connector.live_frame` through the recorder process: refused before the simulator, answered
//! from the last row during a session, paused once the rows stop.

mod common;
use common::*;
use serde_json::json;
use std::time::{Duration, Instant};

#[test]
fn the_live_frame_follows_the_simulator() {
    let rig = rig();
    let mut child = rig.spawn();
    // The simulator is not there yet.
    assert_eq!(
        remote_code(call(&rig, "connector.live_frame", json!({}))),
        "game_not_running"
    );
    let replay = Replay::start(&rig, scenario(2, 30.0));
    let mut frame = None;
    assert!(
        wait_for(40, || {
            frame = call(&rig, "connector.live_frame", json!({}))
                .ok()
                .filter(|f| f["phase"] == "on_track");
            frame.is_some()
        }),
        "no frame on track"
    );
    let frame = frame.unwrap();
    assert_eq!(frame["sim"], "iracing");
    for key in [
        "taken_at",
        "data_age_s",
        "session",
        "track",
        "car",
        "lap",
        "conditions",
        "participants",
    ] {
        assert!(!frame[key].is_null(), "{key} is missing: {frame:#}");
    }
    assert!(frame["data_age_s"].as_f64().unwrap() < 1.0);
    // The answer comes from memory: well inside the protocol's 0.3 s.
    let t = Instant::now();
    call(&rig, "connector.live_frame", json!({})).expect("a frame");
    assert!(
        t.elapsed() < Duration::from_millis(300),
        "{:?}",
        t.elapsed()
    );
    // The rows stop: the frame says paused, with a growing age.
    drop(replay);
    let mut paused = None;
    assert!(
        wait_for(10, || {
            paused = call(&rig, "connector.live_frame", json!({}))
                .ok()
                .filter(|f| f["phase"] == "paused");
            paused.is_some()
        }),
        "never paused"
    );
    assert!(paused.unwrap()["data_age_s"].as_f64().unwrap() >= 1.0);
    assert_eq!(rig.stop(&mut child), Some(0));
    rig.fake.stop();
}
