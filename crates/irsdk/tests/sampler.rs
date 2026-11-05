//! The sampler over synthetic images: slot choice, retries, resets, decimation.

mod common;

use common::{Script, layout, row};
use irsdk::sampler::{Poll, Sampler};
use irsdk::testing::live_image;
use irsdk::vars::Var;
use irsdk::view::{SliceView, View};

const OFF_NUM_BUF: usize = 32;

fn speed(s: &Sampler<impl View>) -> f32 {
    let v: Var<f32> = s.vars().find("Speed").unwrap();
    v.read(s.row())
}

#[test]
fn newest_slot_is_chosen() {
    let l = layout(60, 3);
    let rows = vec![
        (5, row(&l, 1.0, 1)),
        (7, row(&l, 2.0, 2)),
        (6, row(&l, 3.0, 3)),
    ];
    let image = live_image(&l, "---\n", &rows);
    let mut s = Sampler::attach(SliceView::new(&image)).unwrap();
    assert_eq!(s.poll(), Poll::Row);
    assert_eq!(speed(&s), 2.0);
    assert_eq!(s.row().tick(), 7);
    assert_eq!(s.stats().rows, 1);
}

#[test]
fn no_new_row_when_the_tick_did_not_move() {
    let l = layout(60, 3);
    let image = live_image(&l, "---\n", &[(4, row(&l, 1.0, 1))]);
    let mut s = Sampler::attach(SliceView::new(&image)).unwrap();
    assert_eq!(s.poll(), Poll::Row);
    assert_eq!(s.poll(), Poll::NoNewRow);
    assert_eq!(s.poll(), Poll::NoNewRow);
    assert_eq!(s.stats().rows, 1);
}

#[test]
fn an_unwritten_map_gives_no_row() {
    let l = layout(60, 3);
    let image = live_image(&l, "---\n", &[]);
    let mut s = Sampler::attach(SliceView::new(&image)).unwrap();
    assert_eq!(s.poll(), Poll::NoNewRow);
}

#[test]
fn torn_after_two_retries() {
    let l = layout(60, 3);
    let script = Script::from_rows(&l, &[(10, row(&l, 1.0, 1))]);
    let mut s = Sampler::attach(&script).unwrap();
    script.tear.set(true);
    assert_eq!(s.poll(), Poll::Torn);
    let st = s.stats();
    assert_eq!((st.torn, st.retries, st.rows), (1, 2, 0));
    // The row is not delivered; once the writer settles, the next poll gets it.
    script.tear.set(false);
    assert_eq!(s.poll(), Poll::Row);
    assert_eq!(s.row().tick(), 13);
}

#[test]
fn reset_when_the_tick_drops() {
    let l = layout(60, 3);
    let script = Script::from_rows(&l, &[(100, row(&l, 1.0, 1))]);
    let mut s = Sampler::attach(&script).unwrap();
    assert_eq!(s.poll(), Poll::Row);
    let before = s.vars().generation();
    script.replace(live_image(&l, "---\n", &[(3, row(&l, 9.0, 1))]));
    assert_eq!(s.poll(), Poll::Reset);
    assert!(s.vars().generation() > before);
    assert_eq!(s.stats().resets, 1);
    // The new session's first row is delivered, read through a handle resolved again.
    assert_eq!(s.poll(), Poll::Row);
    assert_eq!(speed(&s), 9.0);
}

#[test]
fn reset_when_the_layout_changes() {
    let l = layout(60, 3);
    let script = Script::from_rows(&l, &[(100, row(&l, 1.0, 1))]);
    let mut s = Sampler::attach(&script).unwrap();
    assert_eq!(s.poll(), Poll::Row);
    // Same tick, but four buffers now: the layout guard fires before the tick is looked at.
    let l4 = layout(60, 4);
    script.replace(live_image(&l4, "---\n", &[(100, row(&l4, 5.0, 1))]));
    assert_eq!(s.poll(), Poll::Reset);
    assert_eq!(s.header().num_buf, 4);
    assert_eq!(s.poll(), Poll::Row);
    assert_eq!(speed(&s), 5.0);
}

#[test]
fn a_layout_that_cannot_be_read_is_disconnected() {
    let l = layout(60, 3);
    let script = Script::from_rows(&l, &[(100, row(&l, 1.0, 1))]);
    let mut s = Sampler::attach(&script).unwrap();
    script.set_i32(OFF_NUM_BUF, 9);
    assert_eq!(s.poll(), Poll::Disconnected);
}

#[test]
fn disconnected_when_the_status_bit_is_clear() {
    let l = layout(60, 3);
    let script = Script::from_rows(&l, &[(100, row(&l, 1.0, 1))]);
    let mut s = Sampler::attach(&script).unwrap();
    assert!(s.connected());
    script.set_status(0);
    assert!(!s.connected());
    assert_eq!(s.poll(), Poll::Disconnected);
    script.set_status(1);
    assert_eq!(s.poll(), Poll::Row);
}

#[test]
fn session_info_update_is_read_live() {
    let l = layout(60, 3);
    let script = Script::from_rows(&l, &[]);
    let s = Sampler::attach(&script).unwrap();
    assert_eq!(s.session_info_update(), 1);
    script.set_i32(12, 4);
    assert_eq!(s.session_info_update(), 4);
}

fn feed(script: &Script, s: &mut Sampler<&Script>, ticks: impl Iterator<Item = i32>) -> usize {
    let mut kept = 0;
    for t in ticks {
        script.set_tick(0, t);
        if s.poll() == Poll::Row {
            kept += 1;
        }
    }
    kept
}

#[test]
fn decimation_keeps_one_row_in_six_at_360() {
    let l = layout(360, 3);
    let script = Script::from_rows(&l, &[(0, row(&l, 1.0, 1))]);
    let mut s = Sampler::attach(&script).unwrap();
    let kept = feed(&script, &mut s, 1..=60);
    assert_eq!(kept, 10);
    assert_eq!(s.stats().skipped_rate, 50);
    // Kept ticks are 1, 7, 13, ... 55.
    assert_eq!(s.row().tick(), 55);
}

#[test]
fn no_decimation_at_60() {
    let l = layout(60, 3);
    let script = Script::from_rows(&l, &[(0, row(&l, 1.0, 1))]);
    let mut s = Sampler::attach(&script).unwrap();
    assert_eq!(feed(&script, &mut s, 1..=60), 60);
    assert_eq!(s.stats().skipped_rate, 0);
}
