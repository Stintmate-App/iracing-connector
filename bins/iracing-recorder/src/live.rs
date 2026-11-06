//! The live frame: a double buffer the loop publishes into and `connector.live_frame` reads.
//!
//! Two preallocated [`LiveSnapshot`] slots and an atomic index; each slot is a seqlock (its
//! sequence number is odd while it is written). The loop copies the row into the slot the
//! readers are not directed to: no lock, no JSON, no allocation. A reader that is lapped by
//! two publishes while it copies simply copies again. The JSON frame is built on the handler's
//! thread by [`frame_json`].

use std::cell::UnsafeCell;
use std::sync::atomic::{AtomicU8, AtomicU32, AtomicU64, Ordering, fence};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Instant;

use iracing_core::io::StallTyres;
use iracing_core::live::{LiveSnapshot, frame_json};
use iracing_core::meta::SessionMeta;
use iracing_core::tick::Tick;
use iracing_core::tracker::LapView;
use stintmate_connector_link::link::to_value;
use stintmate_connector_platform::logging::local_iso;
use stintmate_ipc::codec::Value;
use stintmate_ipc::error::RemoteError;

/// The index while nothing is published.
const NONE: u8 = 2;
/// How many times a reader copies before it gives up.
const TRIES: usize = 200;

struct Slot {
    seq: AtomicU32,
    snap: Box<UnsafeCell<LiveSnapshot>>,
}

/// The double buffer: one writer (the loop), any number of readers.
pub struct LiveBuffer {
    slots: [Slot; 2],
    index: AtomicU8,
    generation: AtomicU64,
    meta: Mutex<Option<Arc<SessionMeta>>>,
    start: Instant,
}

// SAFETY: the single writer and the readers synchronise through the slot's sequence number and
// the index (a seqlock); a reader keeps a copy only when the sequence did not change.
unsafe impl Sync for LiveBuffer {}
// SAFETY: the buffer owns its slots; nothing in it is tied to a thread.
unsafe impl Send for LiveBuffer {}

impl Default for LiveBuffer {
    fn default() -> Self {
        let slot = || Slot {
            seq: AtomicU32::new(0),
            snap: Box::new(UnsafeCell::new(LiveSnapshot::empty())),
        };
        LiveBuffer {
            slots: [slot(), slot()],
            index: AtomicU8::new(NONE),
            generation: AtomicU64::new(0),
            meta: Mutex::new(None),
            start: Instant::now(),
        }
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

impl LiveBuffer {
    /// Publishes one row (the loop only: a single writer). Copies plain values into a
    /// preallocated slot; takes no lock and allocates nothing.
    pub fn publish(&self, tick: &Tick, view: &LapView, tyres: Option<StallTyres>, wall_s: f64) {
        let cur = self.index.load(Ordering::Relaxed);
        let w = u8::from(cur == 0);
        let slot = &self.slots[usize::from(w)];
        slot.seq.fetch_add(1, Ordering::Release); // odd: being written
        fence(Ordering::Release);
        let p = slot.snap.get();
        // SAFETY: only the writer writes to a slot, and it writes the one the readers are not
        // directed to; a reader that still reads it discards its copy (odd or changed
        // sequence). The fields are written through raw pointers, never through a reference.
        unsafe {
            (&raw mut (*p).tick).write(*tick);
            (&raw mut (*p).view).write(*view);
            (&raw mut (*p).meta_generation).write(self.generation.load(Ordering::Relaxed));
            (&raw mut (*p).tyres).write(tyres);
            (&raw mut (*p).wall_s).write(wall_s);
            (&raw mut (*p).mono_s).write(self.start.elapsed().as_secs_f64());
        }
        slot.seq.fetch_add(1, Ordering::Release); // even: complete
        self.index.store(w, Ordering::Release);
    }

    /// Replaces the session description the frames are built with (the loop, on a change).
    pub fn set_meta(&self, meta: Option<Arc<SessionMeta>>) {
        *lock(&self.meta) = meta;
        self.generation.fetch_add(1, Ordering::Relaxed);
    }

    /// Nothing to show any more: the simulator is gone.
    pub fn clear(&self) {
        self.index.store(NONE, Ordering::Release);
    }

    /// A consistent copy of the last published snapshot into `out`; `false` before the first
    /// publish and after [`clear`](Self::clear).
    pub fn read(&self, out: &mut LiveSnapshot) -> bool {
        for _ in 0..TRIES {
            let i = self.index.load(Ordering::Acquire);
            if i == NONE {
                return false;
            }
            let slot = &self.slots[usize::from(i)];
            let before = slot.seq.load(Ordering::Acquire);
            if before & 1 == 1 {
                std::hint::spin_loop();
                continue;
            }
            // The copy may race with the writer lapping this slot, so it lands in uninitialised
            // storage and becomes a `LiveSnapshot` only once the sequence proves it complete: a
            // torn copy is never read as a value.
            let mut copy = core::mem::MaybeUninit::<LiveSnapshot>::uninit();
            // SAFETY: both pointers are valid for one `LiveSnapshot` and do not overlap.
            unsafe {
                core::ptr::copy_nonoverlapping(slot.snap.get(), copy.as_mut_ptr(), 1);
            }
            fence(Ordering::Acquire);
            if slot.seq.load(Ordering::Relaxed) == before {
                // SAFETY: the sequence did not change, so the bytes are one complete publish.
                *out = unsafe { copy.assume_init() };
                return true;
            }
        }
        false
    }

    /// Seconds on the buffer's monotonic clock (the clock of `LiveSnapshot::mono_s`).
    pub fn now_s(&self) -> f64 {
        self.start.elapsed().as_secs_f64()
    }

    /// The live frame for `connector.live_frame`, built here from the last snapshot: on the
    /// handler's thread, never on the loop's. The error is the protocol's `game_not_running`
    /// while nothing is published.
    pub fn answer(&self) -> Result<Value, RemoteError> {
        let mut snap = Box::new(LiveSnapshot::empty());
        if !self.read(&mut snap) {
            return Err(RemoteError::new(
                "game_not_running",
                "iRacing is not running.",
            ));
        }
        let meta = lock(&self.meta).clone();
        let frame = frame_json(&snap, meta.as_deref(), self.now_s(), &local_iso(true));
        Ok(to_value(&frame))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn publish(b: &LiveBuffer, n: u32) {
        let mut t = Tick::new();
        t.lap = n as i32;
        t.speed = n as f32;
        t.lap_dist = n as f32;
        let v = iracing_core::tracker::Tracker::new(0).view();
        b.publish(&t, &v, None, f64::from(n));
    }

    #[test]
    fn nothing_before_the_first_publish_and_after_clear() {
        let b = LiveBuffer::default();
        let mut out = LiveSnapshot::empty();
        assert!(!b.read(&mut out));
        assert_eq!(b.answer().unwrap_err().code, "game_not_running");
        publish(&b, 7);
        assert!(b.read(&mut out));
        assert_eq!(out.tick.lap, 7);
        b.clear();
        assert!(!b.read(&mut out));
    }

    /// A reader never sees a half-written snapshot, whatever the writer does meanwhile.
    #[test]
    fn readers_never_see_a_torn_snapshot() {
        use std::sync::atomic::AtomicBool;
        let b = Arc::new(LiveBuffer::default());
        let stop = Arc::new(AtomicBool::new(false));
        let writer = {
            let (b, stop) = (b.clone(), stop.clone());
            std::thread::spawn(move || {
                let mut n = 0u32;
                while !stop.load(Ordering::Relaxed) {
                    n += 1;
                    publish(&b, n);
                }
                n
            })
        };
        let readers: Vec<_> = (0..3)
            .map(|_| {
                let (b, stop) = (b.clone(), stop.clone());
                std::thread::spawn(move || {
                    let mut out = LiveSnapshot::empty();
                    let (mut reads, mut last) = (0u32, 0i32);
                    while !stop.load(Ordering::Relaxed) {
                        if !b.read(&mut out) {
                            continue;
                        }
                        let n = out.tick.lap;
                        assert_eq!(out.tick.speed, n as f32);
                        assert_eq!(out.tick.lap_dist, n as f32);
                        assert_eq!(out.wall_s, f64::from(n));
                        assert!(n >= last, "time does not go back");
                        (reads, last) = (reads + 1, n);
                    }
                    reads
                })
            })
            .collect();
        std::thread::sleep(std::time::Duration::from_millis(500));
        stop.store(true, Ordering::Relaxed);
        let published = writer.join().unwrap();
        let reads: u32 = readers.into_iter().map(|r| r.join().unwrap()).sum();
        assert!(published > 100 && reads > 100, "{published}/{reads}");
    }
}
