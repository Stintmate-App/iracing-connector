//! Writes survive a Pitwall that is busy: they wait in the outbox and arrive when it answers.

mod common;
use common::*;

#[test]
fn laps_wait_in_the_outbox_while_pitwall_is_busy() {
    let rig = rig();
    rig.fake.rules.lock().unwrap().refuse_all_with = Some("busy".into());
    let _replay = Replay::start(&rig, scenario(2, 6.0));
    let mut child = rig.spawn();
    let outbox = rig.data_dir().join("outbox");
    // The files hold the encoded writes: the method name is in the bytes.
    let has_lap = || {
        std::fs::read_dir(&outbox)
            .map(|d| {
                d.filter_map(|e| std::fs::read(e.ok()?.path()).ok())
                    .any(|b| b.windows(17).any(|w| w == b"pitwall.write.lap"))
            })
            .unwrap_or(false)
    };
    assert!(wait_for(40, has_lap), "no lap waited in the outbox");
    rig.fake.rules.lock().unwrap().refuse_all_with = None;
    assert!(
        rig.fake
            .wait_for(|c| c.iter().any(|x| x.method.ends_with(".lap")), 30)
    );
    rig.stop(&mut child);
    rig.fake.stop();
}
