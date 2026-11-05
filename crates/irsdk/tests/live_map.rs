//! `TestMap` written and `Live` reading it, over mappings with unique test names.

#![cfg(windows)]

mod common;

use std::io::ErrorKind;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use common::{layout, row};
use irsdk::defs::{DATA_VALID_EVENT_NAME, MEM_MAP_NAME};
use irsdk::live::{Live, OpenError, sim_mapping_exists};
use irsdk::sampler::Poll;
use irsdk::testing::map::TestMap;
use irsdk::vars::Var;

static COUNTER: AtomicU32 = AtomicU32::new(0);

/// A pair of names no other test, process or the simulator uses.
fn names() -> (String, String) {
    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    let id = format!("{}_{n}", std::process::id());
    (
        format!("Local\\StintmateTestMap_{id}"),
        format!("Local\\StintmateTestEvent_{id}"),
    )
}

fn shared_runner() -> bool {
    std::env::var("STINTMATE_SHARED_RUNNER").is_ok_and(|v| v == "1")
}

const SIZE: usize = 256 * 1024;
const WAIT: Duration = Duration::from_millis(500);

fn speed(live: &Live) -> f32 {
    let v: Var<f32> = live.sampler().vars().find("Speed").unwrap();
    v.read(live.sampler().row())
}

#[test]
fn rows_arrive_in_order() {
    let (m, e) = names();
    let l = layout(60, 3);
    let mut map = TestMap::create(&m, &e, &l, b"---\n", SIZE).unwrap();
    let mut live = Live::open_named(&m, &e).unwrap();
    for i in 1..=7 {
        map.push_row(&row(&l, i as f32, i));
        assert_eq!(live.wait(WAIT), Poll::Row);
        assert_eq!(speed(&live), i as f32);
        assert_eq!(live.sampler().row().tick(), i);
    }
    assert_eq!(live.sampler().stats().rows, 7);
}

#[test]
fn wait_returns_within_the_timeout_without_a_signal() {
    let (m, e) = names();
    let l = layout(60, 3);
    let _map = TestMap::create(&m, &e, &l, b"---\n", SIZE).unwrap();
    let mut live = Live::open_named(&m, &e).unwrap();
    let t = Instant::now();
    assert_eq!(live.wait(Duration::from_millis(40)), Poll::NoNewRow);
    let took = t.elapsed();
    let slack = if shared_runner() { 2000 } else { 400 };
    assert!(
        took >= Duration::from_millis(30),
        "returned early: {took:?}"
    );
    assert!(
        took < Duration::from_millis(40 + slack),
        "too slow: {took:?}"
    );
}

#[test]
fn a_poll_takes_a_row_even_when_the_signal_was_taken() {
    let (m, e) = names();
    let l = layout(60, 3);
    let mut map = TestMap::create(&m, &e, &l, b"---\n", SIZE).unwrap();
    let mut live = Live::open_named(&m, &e).unwrap();
    map.push_row(&row(&l, 2.0, 1));
    // Another reader takes the auto-reset signal first.
    let other = irsdk::live::Event::open(&e).unwrap();
    assert!(other.wait(Duration::from_millis(200)));
    assert_eq!(live.wait(Duration::from_millis(10)), Poll::Row);
}

#[test]
fn restart_gives_a_reset_and_a_new_session() {
    let (m, e) = names();
    let l = layout(60, 3);
    let mut map = TestMap::create(&m, &e, &l, b"---\n", SIZE).unwrap();
    let mut live = Live::open_named(&m, &e).unwrap();
    for i in 1..=4 {
        map.push_row(&row(&l, i as f32, i));
        assert_eq!(live.wait(WAIT), Poll::Row);
    }
    let gen_before = live.sampler().vars().generation();
    let l4 = layout(60, 4);
    map.restart(&l4, b"---\n");
    assert_eq!(live.wait(WAIT), Poll::Reset);
    assert!(live.sampler().vars().generation() > gen_before);
    assert_eq!(live.sampler().header().num_buf, 4);
    map.push_row(&row(&l4, 42.0, 1));
    assert_eq!(live.wait(WAIT), Poll::Row);
    assert_eq!(speed(&live), 42.0);
}

#[test]
fn restart_with_the_same_layout_is_seen_by_the_tick_drop() {
    let (m, e) = names();
    let l = layout(60, 3);
    let mut map = TestMap::create(&m, &e, &l, b"---\n", SIZE).unwrap();
    let mut live = Live::open_named(&m, &e).unwrap();
    for i in 1..=5 {
        map.push_row(&row(&l, i as f32, i));
        assert_eq!(live.wait(WAIT), Poll::Row);
    }
    map.restart(&l, b"---\n");
    assert_eq!(live.wait(WAIT), Poll::Reset);
}

#[test]
fn disconnected_and_connected_again() {
    let (m, e) = names();
    let l = layout(60, 3);
    let mut map = TestMap::create(&m, &e, &l, b"---\n", SIZE).unwrap();
    let mut live = Live::open_named(&m, &e).unwrap();
    map.set_connected(false);
    assert_eq!(live.wait(Duration::from_millis(5)), Poll::Disconnected);
    assert!(!live.sampler().connected());
    map.set_connected(true);
    map.push_row(&row(&l, 1.0, 1));
    assert_eq!(live.wait(WAIT), Poll::Row);
}

#[test]
fn session_text_updates_move_the_counter() {
    let (m, e) = names();
    let l = layout(60, 3);
    let mut map = TestMap::create(&m, &e, &l, b"---\n", SIZE).unwrap();
    let live = Live::open_named(&m, &e).unwrap();
    let before = live.sampler().session_info_update();
    map.set_yaml(b"---\nWeekendInfo: {}\n");
    assert_eq!(live.sampler().session_info_update(), before + 1);
}

#[test]
fn rates_above_60_step_the_ticks() {
    let (m, e) = names();
    let l = layout(360, 3);
    let mut map = TestMap::create(&m, &e, &l, b"---\n", SIZE).unwrap();
    let mut live = Live::open_named(&m, &e).unwrap();
    for i in 1..=5 {
        map.push_row(&row(&l, i as f32, i));
        assert_eq!(live.wait(WAIT), Poll::Row);
    }
    assert_eq!(live.sampler().stats().skipped_rate, 0);
}

#[test]
fn the_real_names_are_refused() {
    let l = layout(60, 3);
    let (m, e) = names();
    for (map, event) in [
        (MEM_MAP_NAME, e.as_str()),
        (m.as_str(), DATA_VALID_EVENT_NAME),
        ("Local\\my_irsdk_copy", e.as_str()),
        (m.as_str(), "Local\\IrSdKevent"),
    ] {
        let err = TestMap::create(map, event, &l, b"---\n", SIZE).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::InvalidInput, "{map} {event}");
    }
}

#[test]
fn a_size_too_small_is_refused() {
    let (m, e) = names();
    let l = layout(60, 3);
    let err = TestMap::create(&m, &e, &l, b"---\n", 1024).unwrap_err();
    assert_eq!(err.kind(), ErrorKind::InvalidInput);
}

#[test]
fn a_missing_mapping_is_not_running() {
    let (m, e) = names();
    assert_eq!(Live::open_named(&m, &e).unwrap_err(), OpenError::NotRunning);
}

#[test]
fn the_sim_mapping_does_not_exist_without_the_sim() {
    if sim_mapping_exists() {
        println!("SKIP: the simulator's mapping exists on this machine");
        return;
    }
    assert!(!sim_mapping_exists());
    assert_eq!(Live::open().unwrap_err(), OpenError::NotRunning);
}
