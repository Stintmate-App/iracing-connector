//! The synthetic session of the replay tool (`iracing-replay --synthetic`) through the
//! recorder into a fake Pitwall. Its session text lists no drivers, so no session can open: the
//! recorder reads the rows, writes nothing and stops cleanly.

mod common;
use common::*;
use irsdk::testing::map::TestMap;
use irsdk::testing::replay::{layout_of, replay};
use irsdk::testing::synthetic_ibt;
use std::sync::atomic::AtomicBool;

#[test]
fn the_synthetic_replay_opens_no_session_and_the_recorder_stays_quiet() {
    let rig = rig();
    let ibt = synthetic_ibt(4, 120);
    let layout = layout_of(&ibt);
    let size = TestMap::required_size(&layout, ibt.yaml_raw().len());
    let mut map =
        TestMap::create(&rig.map, &rig.event, &layout, ibt.yaml_raw(), size).expect("test mapping");
    let stop = AtomicBool::new(false);
    let mut child = rig.spawn();
    let player = std::thread::spawn(move || replay(&ibt, &mut map, 60.0, &stop));
    let rows = player.join().unwrap();
    assert!(rows > 0);
    let writes: Vec<String> = rig
        .fake
        .calls()
        .into_iter()
        .map(|c| c.method)
        .filter(|m| m.starts_with("pitwall.write."))
        .collect();
    assert_eq!(rig.stop(&mut child), Some(0));
    rig.fake.stop();
    assert!(writes.is_empty(), "unexpected writes: {writes:?}");
}
