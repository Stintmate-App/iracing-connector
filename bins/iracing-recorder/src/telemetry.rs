//! `connector.telemetry` v1: the ring the loop writes samples into and the thread that sends them.
//!
//! The loop already gets every row, so it makes the sample ([`iracing_core::telemetry::sample`])
//! and pushes it into an 8-slot single-producer ring of atomics ([`Ring::push`]): no lock, no
//! allocation, and nothing at all while nobody is subscribed. The sender thread sends what the
//! ring holds in order, at most `rate_hz` per second; with no new row for [`KEEPALIVE`] it sends
//! the last sample again with `FRESH` clear, and `game_state` says paused, or not running when
//! the loop reported that the simulator is gone.
//!
//! The subscription option `o: {rate_hz}` (10 to 200) lowers the cap below the declared 60 Hz;
//! a higher value gives 60 and a value out of range is `bad_request`.

use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::{JoinHandle, Thread};
use std::time::{Duration, Instant};

use iracing_core::SPEC;
use iracing_core::live::FROZEN_S;
use iracing_core::telemetry::{GAME_FROZEN, GAME_PLAYING};
use stintmate_connector_platform::proc::raise_thread_priority;
use stintmate_connector_wire::telemetry::{
    CHANNEL, FRESH, RATE_MAX, RATE_MIN, SAMPLE_SIZE, Sample, idle_values,
};
use stintmate_ipc::blocking::Client;
use stintmate_ipc::clock::now_ns;
use stintmate_ipc::codec::{self, Value};
use stintmate_ipc::error::RemoteError;
use stintmate_ipc::layout::{FieldType, Layout};
use stintmate_ipc::{Error, StreamChannel};

/// With no new sample this long, the last one is sent again.
pub const KEEPALIVE: Duration = Duration::from_millis(250);
/// Slots of the ring.
pub const SLOTS: usize = 8;
/// How long the sender sleeps while nobody is subscribed.
const IDLE_PARK: Duration = Duration::from_secs(1);
/// The words of a slot: the 32 bytes of the sample and its time.
const WORDS: usize = SAMPLE_SIZE / 8 + 1;

/// The layout of a sample (32 bytes).
pub fn layout() -> Layout {
    use FieldType::{F32, U8, U16, U32};
    Layout::new(
        [
            ("lap_distance_m", F32, 1),
            ("speed_ms", F32, 1),
            ("accel_long_ms2", F32, 1),
            ("track_length_m", F32, 1),
            ("lap_time_s", F32, 1),
            ("frame", U32, 1),
            ("sim_lap", U16, 1),
            ("session_type", U8, 1),
            ("game_state", U8, 1),
            ("flags", U16, 1),
            ("reserved", U16, 1),
        ],
        Some("telemetry"),
    )
    .expect("the telemetry layout is valid")
}

#[derive(Default)]
struct RingSlot([AtomicU64; WORDS]);

/// A single-producer, single-consumer ring of samples, all atomics. The producer is the loop,
/// the consumer the sender thread.
pub struct Ring {
    slots: [RingSlot; SLOTS],
    head: AtomicU64,
    tail: AtomicU64,
    subscribed: AtomicBool,
    idle: AtomicBool,
    last_row_ns: AtomicI64,
    dropped: AtomicU64,
    waker: OnceLock<Thread>,
}

impl Default for Ring {
    fn default() -> Self {
        Ring {
            slots: Default::default(),
            head: AtomicU64::new(0),
            tail: AtomicU64::new(0),
            subscribed: AtomicBool::new(false),
            idle: AtomicBool::new(true),
            last_row_ns: AtomicI64::new(0),
            dropped: AtomicU64::new(0),
            waker: OnceLock::new(),
        }
    }
}

impl Ring {
    /// Whether the loop should produce samples (someone is subscribed).
    pub fn wanted(&self) -> bool {
        self.subscribed.load(Ordering::Relaxed)
    }

    /// Sets whether someone is subscribed (the channel's activity hook).
    pub fn set_subscribed(&self, on: bool) {
        self.subscribed.store(on, Ordering::Relaxed);
        if on {
            self.wake();
        }
    }

    /// The simulator is gone (`true`) or its rows arrive (`false`): decides the `game_state`
    /// of a repeated sample.
    pub fn set_idle(&self, idle: bool) {
        self.idle.store(idle, Ordering::Relaxed);
    }

    /// Samples dropped because the sender was behind by a full ring.
    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    fn wake(&self) {
        if let Some(t) = self.waker.get() {
            t.unpark();
        }
    }

    /// The loop's side: stores `sample`, taken at `t_ns`, and wakes the sender. A full ring
    /// drops the new sample. No lock, no allocation.
    pub fn push(&self, sample: &Sample, t_ns: i64) {
        let head = self.head.load(Ordering::Relaxed);
        if head.wrapping_sub(self.tail.load(Ordering::Acquire)) >= SLOTS as u64 {
            self.dropped.fetch_add(1, Ordering::Relaxed);
            return;
        }
        let bytes = sample.to_bytes();
        let slot = &self.slots[(head % SLOTS as u64) as usize].0;
        for (i, w) in bytes.as_chunks::<8>().0.iter().enumerate() {
            slot[i].store(u64::from_le_bytes(*w), Ordering::Relaxed);
        }
        slot[WORDS - 1].store(t_ns as u64, Ordering::Relaxed);
        self.last_row_ns.store(t_ns, Ordering::Relaxed);
        self.head.store(head.wrapping_add(1), Ordering::Release);
        self.wake();
    }

    /// The sender's side: the oldest sample not yet taken, with its time.
    pub fn pop(&self) -> Option<(Sample, i64)> {
        let tail = self.tail.load(Ordering::Relaxed);
        if tail == self.head.load(Ordering::Acquire) {
            return None;
        }
        let slot = &self.slots[(tail % SLOTS as u64) as usize].0;
        let mut bytes = [0u8; SAMPLE_SIZE];
        for (i, w) in bytes.as_chunks_mut::<8>().0.iter_mut().enumerate() {
            *w = slot[i].load(Ordering::Relaxed).to_le_bytes();
        }
        let t_ns = slot[WORDS - 1].load(Ordering::Relaxed) as i64;
        self.tail.store(tail.wrapping_add(1), Ordering::Release);
        Some((Sample::from_bytes(&bytes), t_ns))
    }
}

/// Counters of the stream.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct TelemetryStats {
    /// Samples sent, repeats included.
    pub sent: u64,
    /// Repeats sent with no new row.
    pub repeats: u64,
}

struct Inner {
    ring: Arc<Ring>,
    channel: StreamChannel,
    rate_bits: AtomicU64,
    stop: AtomicBool,
    sent: AtomicU64,
    repeats: AtomicU64,
}

/// The stream's producer: declares the channel and runs the sender thread.
pub struct Telemetry {
    inner: Arc<Inner>,
    thread: Mutex<Option<JoinHandle<()>>>,
}

fn bad_rate() -> RemoteError {
    RemoteError::new(
        "bad_request",
        format!("{CHANNEL}: o.rate_hz must be {RATE_MIN}..{RATE_MAX}"),
    )
}

/// The sending cap of a subscription: the option, never above the declared rate.
fn cap(options: &Value) -> Result<f64, RemoteError> {
    let declared = SPEC.recorder_hz;
    match codec::get(options, "rate_hz") {
        None | Some(Value::Nil) => Ok(declared),
        Some(Value::Boolean(_)) => Err(bad_rate()),
        Some(v) => match v.as_f64().or_else(|| v.as_i64().map(|n| n as f64)) {
            Some(r) if (f64::from(RATE_MIN)..=f64::from(RATE_MAX)).contains(&r) => {
                Ok(r.min(declared))
            }
            _ => Err(bad_rate()),
        },
    }
}

impl Telemetry {
    /// Declares `connector.telemetry` at the recorder's rate on `client`, hooks its
    /// subscription and starts the sender thread, which sleeps until someone subscribes.
    /// Declare before the client starts.
    pub fn install(client: &Client, ring: Arc<Ring>) -> Result<Telemetry, Error> {
        let channel =
            client.stream_channel_with(CHANNEL, layout(), &[1], Some(SPEC.recorder_hz), None)?;
        let inner = Arc::new(Inner {
            ring: ring.clone(),
            channel: channel.clone(),
            rate_bits: AtomicU64::new(SPEC.recorder_hz.to_bits()),
            stop: AtomicBool::new(false),
            sent: AtomicU64::new(0),
            repeats: AtomicU64::new(0),
        });
        let i = inner.clone();
        client.on_subscribe(&channel, move |req| {
            let rate = cap(&req.options)?;
            i.rate_bits.store(rate.to_bits(), Ordering::Relaxed);
            Ok(())
        });
        let r = ring.clone();
        client.on_active(&channel, move |active| {
            tracing::info!(
                "Pitwall {} {CHANNEL}",
                if active { "subscribed to" } else { "left" }
            );
            r.set_subscribed(active);
        });
        let me = inner.clone();
        let handle = std::thread::Builder::new()
            .name("telemetry".into())
            .spawn(move || me.run())
            .map_err(|e| Error::from(std::io::Error::other(e)))?;
        let _ = ring.waker.set(handle.thread().clone());
        Ok(Telemetry {
            inner,
            thread: Mutex::new(Some(handle)),
        })
    }

    /// Someone is subscribed.
    pub fn subscribed(&self) -> bool {
        self.inner.ring.wanted()
    }

    /// The counters.
    pub fn stats(&self) -> TelemetryStats {
        TelemetryStats {
            sent: self.inner.sent.load(Ordering::Relaxed),
            repeats: self.inner.repeats.load(Ordering::Relaxed),
        }
    }

    /// Stops the sender thread (shutdown).
    pub fn close(&self) {
        self.inner.stop.store(true, Ordering::Relaxed);
        self.inner.ring.wake();
        let h = self.thread.lock().unwrap_or_else(|p| p.into_inner()).take();
        if let Some(h) = h {
            let _ = h.join();
        }
    }
}

impl Inner {
    fn send(&self, s: &Sample, t_ns: i64) {
        if self.channel.send(&s.to_bytes(), Some(t_ns)).is_ok() {
            self.sent.fetch_add(1, Ordering::Relaxed);
        }
    }

    fn rate(&self) -> f64 {
        f64::from_bits(self.rate_bits.load(Ordering::Relaxed))
    }

    /// The sender: runs until [`Telemetry::close`].
    fn run(&self) {
        raise_thread_priority();
        let ring = &self.ring;
        let mut last: Option<Sample> = None;
        let mut last_sent = Instant::now();
        let mut last_any = Instant::now();
        while !self.stop.load(Ordering::Relaxed) {
            if !ring.wanted() {
                last = None;
                while ring.pop().is_some() {}
                std::thread::park_timeout(IDLE_PARK);
                continue;
            }
            let min_gap = Duration::from_secs_f64(0.95 / self.rate());
            while let Some((s, t_ns)) = ring.pop() {
                last = Some(s);
                if last_sent.elapsed() >= min_gap {
                    self.send(&s, t_ns);
                    last_sent = Instant::now();
                    last_any = last_sent;
                }
            }
            let quiet = last_any.elapsed();
            if quiet >= KEEPALIVE {
                let frame = last.map_or(0, |s| s.frame);
                let repeat = if ring.idle.load(Ordering::Relaxed) {
                    idle_values(frame)
                } else {
                    let mut s = last.unwrap_or_else(|| idle_values(frame));
                    s.flags &= !FRESH;
                    let silent = (now_ns() - ring.last_row_ns.load(Ordering::Relaxed)) as f64 / 1e9;
                    if silent >= FROZEN_S && s.game_state == GAME_PLAYING {
                        s.game_state = GAME_FROZEN;
                    }
                    s
                };
                self.send(&repeat, now_ns());
                self.repeats.fetch_add(1, Ordering::Relaxed);
                last_any = Instant::now();
                continue;
            }
            std::thread::park_timeout(KEEPALIVE - quiet);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(n: u32) -> Sample {
        let mut s = idle_values(n);
        s.speed_ms = n as f32;
        s
    }

    #[test]
    fn the_ring_keeps_order_and_drops_the_new_one_when_full() {
        let r = Ring::default();
        assert!(r.pop().is_none());
        for n in 0..SLOTS as u32 + 2 {
            r.push(&sample(n), i64::from(n));
        }
        assert_eq!(r.dropped(), 2);
        for n in 0..SLOTS as u32 {
            let (s, t) = r.pop().expect("a sample");
            assert_eq!((s.frame, t), (n, i64::from(n)));
            assert_eq!(s.speed_ms, n as f32);
        }
        assert!(r.pop().is_none());
        r.push(&sample(99), 5);
        assert_eq!(r.pop().map(|(s, _)| s.frame), Some(99));
    }

    #[test]
    fn the_rate_option_is_checked_and_capped() {
        let opt = |v: Value| codec::map([("rate_hz", v)]);
        assert_eq!(
            cap(&codec::map::<[(&str, Value); 0], &str>([])).unwrap(),
            60.0
        );
        assert_eq!(cap(&opt(Value::from(30))).unwrap(), 30.0);
        assert_eq!(cap(&opt(Value::from(120))).unwrap(), 60.0);
        for bad in [Value::from(5), Value::from(500), Value::from(true)] {
            assert_eq!(cap(&opt(bad)).unwrap_err().code, "bad_request");
        }
    }
}
