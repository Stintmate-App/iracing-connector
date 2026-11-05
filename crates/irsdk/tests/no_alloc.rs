//! Polling and reading variables allocate nothing.

mod common;

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

use common::{Script, layout, row};
use irsdk::sampler::{Poll, Sampler};
use irsdk::vars::Var;

struct Counting;

static ALLOCS: AtomicUsize = AtomicUsize::new(0);

// SAFETY: every call is forwarded unchanged to the system allocator; the counter is the only
// addition.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        ALLOCS.fetch_add(1, Ordering::SeqCst);
        // SAFETY: same contract as the caller's.
        unsafe { System.alloc(l) }
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        // SAFETY: same contract as the caller's.
        unsafe { System.dealloc(p, l) }
    }
    unsafe fn alloc_zeroed(&self, l: Layout) -> *mut u8 {
        ALLOCS.fetch_add(1, Ordering::SeqCst);
        // SAFETY: same contract as the caller's.
        unsafe { System.alloc_zeroed(l) }
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 {
        ALLOCS.fetch_add(1, Ordering::SeqCst);
        // SAFETY: same contract as the caller's.
        unsafe { System.realloc(p, l, n) }
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

#[test]
fn ten_thousand_polls_and_reads_allocate_nothing() {
    let l = layout(60, 3);
    let script = Script::from_rows(&l, &[(1, row(&l, 1.0, 1))]);
    let mut s = Sampler::attach(&script).unwrap();
    let speed: Var<f32> = s.vars().find("Speed").unwrap();
    let lap: Var<i32> = s.vars().find("Lap").unwrap();
    let mut sum = 0.0f64;
    let mut rows = 0;
    let before = ALLOCS.load(Ordering::SeqCst);
    for i in 0..10_000usize {
        // A new tick every second poll, so both Row and NoNewRow are exercised.
        script.set_tick(i % 3, 2 + (i as i32) / 2);
        match s.poll() {
            Poll::Row => {
                rows += 1;
                sum += f64::from(speed.read(s.row())) + f64::from(lap.read(s.row()));
            }
            Poll::NoNewRow => {}
            other => panic!("unexpected {other:?}"),
        }
    }
    let after = ALLOCS.load(Ordering::SeqCst);
    assert_eq!(after - before, 0, "the per-tick path allocated");
    assert!(rows > 4000, "rows seen: {rows}");
    assert!(sum > 0.0);
}
