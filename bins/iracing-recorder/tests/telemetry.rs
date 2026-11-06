//! `connector.telemetry` through the recorder process: nothing before a subscription, one
//! sample per row at the declared rate, the rate option, and the keep-alive when rows stop.

mod common;
use common::*;
use iracing_core::SPEC;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use stintmate_connector_wire::telemetry::{FRESH, PLAYER_VIEW, Sample};
use stintmate_ipc::codec;
use stintmate_ipc::node::{Delivery, SubscribeOptions};

type Got = Arc<Mutex<Vec<(i64, Sample)>>>;

fn subscribe(
    rig: &Rig,
    rate: Option<codec::Value>,
) -> Result<(stintmate_ipc::node::Subscription, Got), stintmate_ipc::Error> {
    let got: Got = Arc::default();
    let g = got.clone();
    let opts = SubscribeOptions {
        options: rate.map(|r| codec::map([("rate_hz", r)])),
        ..SubscribeOptions::default()
    };
    let sub = peer(rig).subscribe("connector.telemetry", opts, move |d| {
        if let Delivery::Sample(s) = d {
            let b: [u8; 32] = s.body.as_ref().try_into().unwrap();
            g.lock().unwrap().push((s.t_ns, Sample::from_bytes(&b)));
        }
    })?;
    Ok((sub, got))
}

#[test]
fn samples_follow_the_rows_at_the_declared_rate_and_the_option_lowers_it() {
    let rig = rig();
    let _replay = Replay::start(&rig, scenario(2, 30.0));
    let mut child = rig.spawn();
    for bad in [
        codec::Value::from(5),
        codec::Value::from(500),
        codec::Value::from(true),
        codec::Value::from("fast"),
    ] {
        match subscribe(&rig, Some(bad.clone())) {
            Err(stintmate_ipc::Error::Remote(e)) => assert_eq!(e.code, "bad_request", "{bad:?}"),
            other => panic!("{bad:?}: {:?}", other.map(|_| ())),
        }
    }
    let (sub, got) = subscribe(&rig, None).unwrap();
    assert!(
        wait_for(30, || !got.lock().unwrap().is_empty()),
        "no sample"
    );
    let from = got.lock().unwrap().len();
    std::thread::sleep(Duration::from_millis(2000));
    sub.close();
    let v: Vec<_> = got.lock().unwrap()[from..].to_vec();
    assert!(
        (90..=135).contains(&v.len()),
        "{} samples in 2 s at {} Hz",
        v.len(),
        SPEC.recorder_hz
    );
    assert!(
        v.iter()
            .all(|(_, s)| s.flags & FRESH != 0 && s.flags & PLAYER_VIEW != 0)
    );
    assert!(v.windows(2).all(|w| w[1].0 > w[0].0), "t_ns increases");
    assert!(
        v.windows(2).all(|w| w[1].1.frame > w[0].1.frame),
        "the frame counter advances"
    );
    let (sub, got) = subscribe(&rig, Some(codec::Value::from(20))).unwrap();
    std::thread::sleep(Duration::from_millis(2000));
    sub.close();
    let n = got.lock().unwrap().len();
    assert!((25..=50).contains(&n), "{n} samples in 2 s at a 20 Hz cap");
    let (sub, got) = subscribe(&rig, Some(codec::Value::from(200))).unwrap();
    std::thread::sleep(Duration::from_millis(1500));
    sub.close();
    let n = got.lock().unwrap().len();
    assert!(n <= 110, "{n} samples in 1.5 s: 200 Hz is capped at 60");
    assert_eq!(rig.stop(&mut child), Some(0));
    rig.fake.stop();
}

#[test]
fn the_keep_alive_repeats_when_rows_stop_and_nothing_follows_an_unsubscribe() {
    let rig = rig();
    let replay = Replay::start(&rig, scenario(2, 30.0));
    let mut child = rig.spawn();
    let (sub, got) = subscribe(&rig, None).unwrap();
    assert!(
        wait_for(40, || got
            .lock()
            .unwrap()
            .iter()
            .any(|(_, s)| s.game_state == 3)),
        "never driving"
    );
    // The rows stop (a pause): the last sample repeats about four times a second, not fresh.
    drop(replay);
    std::thread::sleep(Duration::from_millis(1500));
    let before = got.lock().unwrap().len();
    std::thread::sleep(Duration::from_millis(1500));
    let v: Vec<_> = got.lock().unwrap()[before..].to_vec();
    assert!((3..=8).contains(&v.len()), "{} repeats in 1.5 s", v.len());
    assert!(v.iter().all(|(_, s)| s.flags & FRESH == 0));
    assert!(v.iter().all(|(_, s)| s.game_state == 4), "paused: {v:?}");
    sub.close();
    std::thread::sleep(Duration::from_millis(600));
    let after = got.lock().unwrap().len();
    std::thread::sleep(Duration::from_millis(600));
    assert_eq!(
        got.lock().unwrap().len(),
        after,
        "nothing after unsubscribing"
    );
    assert_eq!(rig.stop(&mut child), Some(0));
    rig.fake.stop();
}
