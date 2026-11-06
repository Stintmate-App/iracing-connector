//! The synthetic session of the replay tool (`iracing-replay --synthetic`) through the
//! recorder into a fake Pitwall: its session text lists the player and its layout holds the
//! variables the recorder needs, so the session opens and the laps are written.

mod common;
use common::*;
use irsdk::testing::map::TestMap;
use irsdk::testing::replay::{layout_of, replay};
use irsdk::testing::synthetic_ibt;
use std::sync::atomic::AtomicBool;

#[test]
fn the_synthetic_replay_opens_a_session_and_its_laps_are_written() {
    let rig = rig();
    let ibt = synthetic_ibt(4, 240);
    let layout = layout_of(&ibt);
    let size = TestMap::required_size(&layout, ibt.yaml_raw().len());
    let mut map =
        TestMap::create(&rig.map, &rig.event, &layout, ibt.yaml_raw(), size).expect("test mapping");
    let stop = AtomicBool::new(false);
    let mut child = rig.spawn();
    let player = std::thread::spawn(move || replay(&ibt, &mut map, 60.0, &stop));
    let rows = player.join().unwrap();
    assert!(rows > 0);
    assert!(
        rig.fake
            .wait_for(|c| c.iter().any(|x| x.method.ends_with(".lap")), 20),
        "no lap was written: {:?}",
        rig.fake
            .calls()
            .iter()
            .map(|c| &c.method)
            .collect::<Vec<_>>()
    );
    assert_eq!(rig.count_suffix("session_start"), 1);
    assert_eq!(rig.stop(&mut child), Some(0));
    rig.fake.stop();
}
