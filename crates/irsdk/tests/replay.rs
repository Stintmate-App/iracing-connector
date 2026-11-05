//! `replay` of an `.ibt` into a `TestMap` read by `Live`.

#![cfg(windows)]

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::Duration;

use irsdk::live::Live;
use irsdk::sampler::Poll;
use irsdk::testing::map::TestMap;
use irsdk::testing::replay::{layout_of, replay};
use irsdk::testing::synthetic_ibt;

static COUNTER: AtomicU32 = AtomicU32::new(0);

fn names() -> (String, String) {
    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    let id = format!("{}_{n}", std::process::id());
    (
        format!("Local\\StintmateReplayMap_{id}"),
        format!("Local\\StintmateReplayEvent_{id}"),
    )
}

fn shared_runner() -> bool {
    std::env::var("STINTMATE_SHARED_RUNNER").is_ok_and(|v| v == "1")
}

fn map_for(ibt: &irsdk::ibt::Ibt, m: &str, e: &str) -> TestMap {
    let l = layout_of(ibt);
    let size = TestMap::required_size(&l, ibt.yaml_raw().len());
    TestMap::create(m, e, &l, ibt.yaml_raw(), size).unwrap()
}

#[test]
fn replayed_rows_arrive_in_order_with_the_same_bytes() {
    let (m, e) = names();
    let ibt = synthetic_ibt(1, 24);
    let mut map = map_for(&ibt, &m, &e);
    let mut live = Live::open_named(&m, &e).unwrap();
    let stop = AtomicBool::new(false);

    let (pushed, got) = std::thread::scope(|s| {
        let h = s.spawn(|| replay(&ibt, &mut map, 100.0, &stop));
        let mut got: Vec<(i32, Vec<u8>)> = Vec::new();
        while got.len() < ibt.len() {
            match live.wait(Duration::from_millis(500)) {
                Poll::Row => got.push((
                    live.sampler().row().tick(),
                    live.sampler().row().bytes().to_vec(),
                )),
                Poll::NoNewRow if h.is_finished() => break,
                _ => {}
            }
        }
        (h.join().unwrap(), got)
    });

    assert_eq!(pushed, ibt.len());
    // Every received row is a file row, in file order, with rising ticks.
    let mut next = 0;
    let mut last_tick = 0;
    for (tick, bytes) in &got {
        assert!(*tick > last_tick);
        last_tick = *tick;
        let at = (next..ibt.len())
            .find(|&i| ibt.row(i).unwrap() == bytes.as_slice())
            .expect("a row that is not in the file, or out of order");
        next = at + 1;
    }
    if shared_runner() {
        assert!(!got.is_empty());
    } else {
        assert_eq!(
            got.len(),
            ibt.len(),
            "a reader that keeps up sees every row"
        );
    }
}

#[test]
fn the_session_text_is_written_first_and_moves_the_update_counter() {
    let (m, e) = names();
    let ibt = synthetic_ibt(1, 4);
    let mut map = map_for(&ibt, &m, &e);
    let mut live = Live::open_named(&m, &e).unwrap();
    let before = live.sampler().session_info_update();
    let stop = AtomicBool::new(false);
    assert_eq!(replay(&ibt, &mut map, 500.0, &stop), 4);
    assert_eq!(live.wait(Duration::from_millis(500)), Poll::Row);
    assert!(live.sampler().session_info_update() > before);
}

#[test]
fn a_set_stop_flag_ends_the_replay() {
    let (m, e) = names();
    let ibt = synthetic_ibt(1, 10);
    let mut map = map_for(&ibt, &m, &e);
    let stop = AtomicBool::new(true);
    assert_eq!(replay(&ibt, &mut map, 60.0, &stop), 0);
}

#[test]
fn the_real_names_are_refused() {
    let ibt = synthetic_ibt(1, 4);
    let l = layout_of(&ibt);
    let r = TestMap::create(
        irsdk::defs::MEM_MAP_NAME,
        "Local\\StintmateReplayEvent_x",
        &l,
        b"---\n",
        1 << 20,
    );
    assert!(r.is_err());
}
