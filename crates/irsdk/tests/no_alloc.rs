//! Polling, reading variables and notifying the session worker allocate nothing.

mod common;

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use common::{Script, layout, row};
use irsdk::sampler::{Poll, Sampler};
use irsdk::vars::Var;
use irsdk::worker::{YamlSource, YamlWorker};

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

fn polls_and_reads_allocate_nothing() {
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

/// A source whose counter never moves.
struct Still;

impl YamlSource for Still {
    fn update(&self) -> i32 {
        1
    }
    fn copy_yaml(&self, dst: &mut Vec<u8>) -> bool {
        dst.clear();
        dst.extend_from_slice(b"---\nWeekendInfo:\n TrackID: 7\n");
        true
    }
}

fn notifying_the_worker_allocates_nothing() {
    let w = YamlWorker::spawn(Arc::new(Still)).unwrap();
    w.notify(1);
    let end = Instant::now() + Duration::from_secs(10);
    while w.latest().is_none() {
        assert!(Instant::now() < end, "no document");
        std::thread::sleep(Duration::from_millis(2));
    }
    // Let the worker settle back to waiting.
    std::thread::sleep(Duration::from_millis(100));
    let before = ALLOCS.load(Ordering::SeqCst);
    for _ in 0..10_000 {
        w.notify(1);
    }
    let after = ALLOCS.load(Ordering::SeqCst);
    assert_eq!(after - before, 0, "notify allocated");
    // The same counter again costs the worker nothing either: no second document.
    std::thread::sleep(Duration::from_millis(50));
    assert_eq!(w.generation(), 1);
    w.close();
}

/// One test, because the counter is global: a second test running at the same time would
/// count its own allocations.
#[test]
fn the_per_tick_paths_allocate_nothing() {
    polls_and_reads_allocate_nothing();
    notifying_the_worker_allocates_nothing();
}
