//! A recorder restarted while the simulator keeps its session continues the session.

mod common;
use common::*;

fn keys(rig: &Rig) -> Vec<String> {
    rig.fake
        .calls()
        .iter()
        .filter(|c| c.method.ends_with(".lap"))
        .filter_map(|c| c.params["session_key"].as_str().map(str::to_owned))
        .collect()
}

#[test]
fn a_restarted_recorder_keeps_the_session_key() {
    let rig = rig();
    let _replay = Replay::start(&rig, scenario(4, 6.0));
    let mut first = rig.spawn();
    assert!(
        rig.fake
            .wait_for(|c| c.iter().any(|x| x.method.ends_with(".lap")), 40)
    );
    assert_eq!(rig.stop(&mut first), Some(0));
    let before = keys(&rig);
    assert!(!before.is_empty());
    assert!(rig.data_dir().join("session-resume.json").exists());
    let mut second = rig.spawn();
    assert!(
        rig.fake.wait_for(
            |c| c.iter().filter(|x| x.method.ends_with(".lap")).count() > before.len(),
            40
        ),
        "no lap after the restart"
    );
    let all = keys(&rig);
    assert!(
        all.iter().all(|k| *k == all[0]),
        "one session key across the restart: {all:?}"
    );
    assert_eq!(rig.count_suffix("session_end"), 0);
    rig.stop(&mut second);
    rig.fake.stop();
}
