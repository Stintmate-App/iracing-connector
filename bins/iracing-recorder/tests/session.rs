//! A scripted session replayed through the recorder process into a fake Pitwall.

mod common;
use common::*;
use stintmate_connector_testing::to_json;

#[test]
fn a_replayed_session_reaches_pitwall_and_is_kept_open_on_stop() {
    let rig = rig();
    let replay = Replay::start(&rig, scenario(3, 6.0));
    let mut child = rig.spawn();
    assert!(
        rig.fake.wait_for(
            |c| c.iter().filter(|x| x.method.ends_with(".lap")).count() >= 2,
            40
        ),
        "laps did not arrive: {:?}",
        rig.fake
            .calls()
            .iter()
            .map(|c| &c.method)
            .collect::<Vec<_>>()
    );
    let calls = rig.fake.calls();
    let start = calls
        .iter()
        .find(|c| c.method.ends_with("session_start"))
        .expect("a session start");
    assert!(start.ik.as_deref().unwrap().starts_with("iracing/"));
    let sp = to_json(&start.params);
    assert!(sp["car_id"].is_string() && sp["track_id"].is_string());
    let first_catalog = calls.iter().position(|c| c.method.ends_with("catalog"));
    let first_start = calls
        .iter()
        .position(|c| c.method.ends_with("session_start"));
    assert!(first_catalog < first_start, "the catalog goes first");
    let lap = calls.iter().find(|c| c.method.ends_with(".lap")).unwrap();
    assert_eq!(to_json(&lap.params)["channels"]["speed"]["encoding"], "f32");
    assert!(lap.ik.as_deref().unwrap().contains("/lap/"));
    drop(replay);
    // Stopping while the simulator runs and a session is open keeps the session open.
    assert_eq!(rig.stop(&mut child), Some(0));
    assert_eq!(rig.count_suffix("session_end"), 0);
    assert!(rig.data_dir().join("session-resume.json").exists());
    rig.fake.stop();
}
