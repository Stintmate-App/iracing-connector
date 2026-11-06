//! The outbox summary for the status, read away from the sampling loop.
//!
//! The runtime's outbox holds its lock while it stores a lap (file create, write and
//! `sync_all`), so a thread that asks it for the summary can wait on the disk. The sampling loop
//! must never wait on the disk: a thread of its own reads the summary and keeps the last one in
//! a small value, and the loop only copies that value. The value's lock is held for a copy, never
//! while the outbox is read.

use std::sync::mpsc::{Sender, channel};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::Duration;

/// What the status reports of the outbox.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Snapshot {
    /// Writes waiting to be delivered.
    pub pending: usize,
    /// When the oldest was stored; empty without any.
    pub oldest_at: String,
    /// Writes Pitwall refused.
    pub rejected: u64,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// The last [`Snapshot`], refreshed by its own thread.
pub struct OutboxSummary {
    cache: Arc<Mutex<Snapshot>>,
    stop: Sender<()>,
    thread: Option<JoinHandle<()>>,
}

impl OutboxSummary {
    /// Reads once with `read` on this thread, then again every `every` on a thread of its own.
    /// `read` may block (the outbox lock); nothing else waits for it.
    pub fn spawn(every: Duration, mut read: impl FnMut(&mut Snapshot) + Send + 'static) -> Self {
        let mut first = Snapshot::default();
        read(&mut first);
        let cache = Arc::new(Mutex::new(first));
        let (stop, rx) = channel::<()>();
        let shared = Arc::clone(&cache);
        let thread = std::thread::Builder::new()
            .name("outbox-summary".into())
            .spawn(move || {
                let mut next = Snapshot::default();
                // A message or a closed channel ends it; only a timeout means another round.
                while rx
                    .recv_timeout(every)
                    .is_err_and(|e| e == std::sync::mpsc::RecvTimeoutError::Timeout)
                {
                    read(&mut next);
                    lock(&shared).clone_from(&next);
                }
            })
            .ok();
        OutboxSummary {
            cache,
            stop,
            thread,
        }
    }

    /// Lends the last snapshot to `f`. Never waits for the outbox.
    pub fn with<R>(&self, f: impl FnOnce(&Snapshot) -> R) -> R {
        f(&lock(&self.cache))
    }

    /// Stops the thread.
    pub fn close(mut self) {
        let _ = self.stop.send(());
        if let Some(h) = self.thread.take() {
            let _ = h.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::{Condvar, Mutex};
    use std::time::Instant;

    #[test]
    fn the_loop_side_never_waits_for_a_blocked_reader() {
        let calls = Arc::new(AtomicU32::new(0));
        let gate = Arc::new((Mutex::new(false), Condvar::new()));
        let (c, g) = (Arc::clone(&calls), Arc::clone(&gate));
        let summary = OutboxSummary::spawn(Duration::from_millis(10), move |s| {
            // The first read answers at once; every later one is stuck, as when a lap is being
            // stored.
            if c.fetch_add(1, Ordering::SeqCst) == 0 {
                s.pending = 3;
                s.oldest_at = "2026-11-06T10:00:00".into();
                return;
            }
            let (m, cv) = &*g;
            let mut open = lock(m);
            while !*open {
                open = cv.wait(open).unwrap_or_else(|e| e.into_inner());
            }
            s.pending = 4;
        });
        let end = Instant::now() + Duration::from_secs(5);
        while calls.load(Ordering::SeqCst) < 2 && Instant::now() < end {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(calls.load(Ordering::SeqCst), 2, "the reader is blocked");
        let started = Instant::now();
        for _ in 0..1000 {
            summary.with(|s| {
                assert_eq!(
                    (s.pending, s.oldest_at.as_str()),
                    (3, "2026-11-06T10:00:00")
                )
            });
        }
        assert!(started.elapsed() < Duration::from_millis(500));
        *lock(&gate.0) = true;
        gate.1.notify_all();
        summary.close();
    }
}
