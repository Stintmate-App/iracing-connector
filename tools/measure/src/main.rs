//! `iracing-measure`: runs the recorder's loop over a synthetic session replayed into a test
//! memory mapping and reports what it cost.
//!
//! The loop is the recorder's own (`Reader::poll_timed` and `Pipeline`, the live frame, the
//! detectors and the spotter included); only the ends differ: the records go to a sink that
//! counts them instead of Pitwall, and with `--subscribe` the telemetry stream is switched on
//! and drained by a thread. The pipe to Pitwall is not part of the measurement.
//!
//! The simulator's own names are never used: the mapping is a test mapping with a name made
//! from this process's id.

#![deny(unsafe_op_in_unsafe_fn)]

#[cfg(windows)]
fn main() -> std::process::ExitCode {
    app::run()
}

#[cfg(not(windows))]
fn main() -> std::process::ExitCode {
    eprintln!("iracing-measure uses a Windows memory mapping and runs on Windows only");
    std::process::ExitCode::from(2)
}

#[cfg(windows)]
mod app {
    use std::alloc::{GlobalAlloc, Layout, System};
    use std::cell::Cell;
    use std::process::ExitCode;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    use iracing_core::testing::{LapProfile, Scenario, YamlBuilder};
    use iracing_core::{FinishedLap, ParticipantLap, SessionOpenInfo, SetupReason, Sink};
    use iracing_recorder::announce;
    use iracing_recorder::cli::SdkNames;
    use iracing_recorder::live::LiveBuffer;
    use iracing_recorder::recorder::{Pipeline, Reader, WallClock};
    use iracing_recorder::sink::NoteClock;
    use iracing_recorder::telemetry::Ring;
    use irsdk::sampler::Poll;
    use irsdk::testing::map::TestMap;

    const USAGE: &str = "usage: iracing-measure --synthetic <seconds> [--cars <n>] [--subscribe]\n\
  --synthetic <seconds>  length of the replayed session, in seconds (rows at 60 Hz, 5 or more)\n\
  --cars <n>             cars in the session, the player included (1 to 64, default 64)\n\
  --subscribe            switch the telemetry stream on, as when Pitwall subscribes";

    // ---- allocations, counted on the sampling thread only ----------------------------------

    struct Counting;
    thread_local! {
        static COUNT: Cell<bool> = const { Cell::new(false) };
        static N: Cell<u64> = const { Cell::new(0) };
    }
    // SAFETY: forwards to the system allocator and only bumps a thread-local counter.
    unsafe impl GlobalAlloc for Counting {
        unsafe fn alloc(&self, l: Layout) -> *mut u8 {
            if COUNT.with(Cell::get) {
                N.with(|n| n.set(n.get() + 1));
            }
            // SAFETY: the caller upholds `GlobalAlloc::alloc`'s contract for `l`.
            unsafe { System.alloc(l) }
        }
        unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
            // SAFETY: `p` came from `alloc` with `l`, as the caller promises.
            unsafe { System.dealloc(p, l) }
        }
        unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 {
            if COUNT.with(Cell::get) {
                N.with(|c| c.set(c.get() + 1));
            }
            // SAFETY: `p` came from `alloc` with `l`, as the caller promises.
            unsafe { System.realloc(p, l, n) }
        }
    }
    #[global_allocator]
    static ALLOC: Counting = Counting;

    // ---- CPU time and memory from the system ----------------------------------------------

    #[repr(C)]
    #[derive(Default, Clone, Copy)]
    struct FileTime {
        low: u32,
        high: u32,
    }

    impl FileTime {
        /// 100-nanosecond units to seconds.
        fn secs(self) -> f64 {
            ((u64::from(self.high) << 32) | u64::from(self.low)) as f64 * 1e-7
        }
    }

    #[repr(C)]
    #[derive(Default)]
    struct MemoryCounters {
        cb: u32,
        page_faults: u32,
        peak_working_set: usize,
        working_set: usize,
        quota_peak_paged: usize,
        quota_paged: usize,
        quota_peak_non_paged: usize,
        quota_non_paged: usize,
        pagefile: usize,
        peak_pagefile: usize,
    }

    type TimesFn = unsafe extern "system" fn(
        isize,
        *mut FileTime,
        *mut FileTime,
        *mut FileTime,
        *mut FileTime,
    ) -> i32;

    unsafe extern "system" {
        fn GetCurrentThread() -> isize;
        fn GetCurrentProcess() -> isize;
        fn GetThreadTimes(
            thread: isize,
            create: *mut FileTime,
            exit: *mut FileTime,
            kernel: *mut FileTime,
            user: *mut FileTime,
        ) -> i32;
        fn GetProcessTimes(
            process: isize,
            create: *mut FileTime,
            exit: *mut FileTime,
            kernel: *mut FileTime,
            user: *mut FileTime,
        ) -> i32;
        fn K32GetProcessMemoryInfo(process: isize, counters: *mut MemoryCounters, cb: u32) -> i32;
    }

    /// Kernel plus user CPU seconds of `handle` through `times`.
    fn cpu_of(times: TimesFn, handle: isize) -> f64 {
        let mut t = [FileTime::default(); 4];
        // SAFETY: the four pointers address live `FileTime`s and `handle` is a pseudo-handle of
        // the calling thread or process, which is always valid.
        let ok = unsafe { times(handle, &mut t[0], &mut t[1], &mut t[2], &mut t[3]) };
        if ok == 0 {
            f64::NAN
        } else {
            t[2].secs() + t[3].secs()
        }
    }

    /// CPU seconds of the calling thread.
    fn thread_cpu() -> f64 {
        // SAFETY: `GetCurrentThread` has no preconditions.
        cpu_of(GetThreadTimes, unsafe { GetCurrentThread() })
    }

    /// CPU seconds of this process, every thread.
    fn process_cpu() -> f64 {
        // SAFETY: `GetCurrentProcess` has no preconditions.
        cpu_of(GetProcessTimes, unsafe { GetCurrentProcess() })
    }

    /// `(working set, peak working set)` of this process, in bytes.
    fn memory() -> (usize, usize) {
        let mut c = MemoryCounters {
            cb: size_of::<MemoryCounters>() as u32,
            ..Default::default()
        };
        // SAFETY: `c` is a live `PROCESS_MEMORY_COUNTERS` with `cb` set to its size.
        let ok = unsafe { K32GetProcessMemoryInfo(GetCurrentProcess(), &mut c, c.cb) };
        if ok == 0 {
            (0, 0)
        } else {
            (c.working_set, c.peak_working_set)
        }
    }

    // ---- the sink: counts what the tracker produces ----------------------------------------

    #[derive(Default)]
    struct Counts {
        sessions: u32,
        laps: u32,
        setups: u32,
        pit_exits: u32,
        participant_rows: usize,
    }

    impl NoteClock for Counts {}
    impl Sink for Counts {
        fn start_session(&mut self, _: &SessionOpenInfo) -> u64 {
            self.sessions += 1;
            u64::from(self.sessions)
        }
        fn end_session(&mut self, _: u64, _: &str) {}
        fn lap(&mut self, _: u64, _: FinishedLap) {
            self.laps += 1;
        }
        fn setup(
            &mut self,
            _: u64,
            _: SetupReason,
            _: &str,
            _: Arc<iracing_core::meta::SessionMeta>,
            _: f32,
        ) {
            self.setups += 1;
        }
        fn pit_exit(&mut self, _: u64, _: &str, _: Option<f64>) {
            self.pit_exits += 1;
        }
        fn participants(&mut self, _: u64, rows: Vec<ParticipantLap>) {
            self.participant_rows += rows.len();
        }
        fn catalog(&mut self, _: Arc<iracing_core::meta::SessionMeta>) {}
    }

    // ---- the command line ------------------------------------------------------------------

    struct Config {
        seconds: f64,
        cars: usize,
        subscribe: bool,
    }

    fn parse(args: &[String]) -> Result<Config, String> {
        let (mut seconds, mut cars, mut subscribe) = (None, 64usize, false);
        let mut it = args.iter();
        while let Some(a) = it.next() {
            match a.as_str() {
                "--synthetic" => {
                    let v = it.next().ok_or("--synthetic needs a value")?;
                    seconds = Some(
                        v.parse::<f64>()
                            .ok()
                            .filter(|s| s.is_finite() && *s >= 5.0)
                            .ok_or("--synthetic must be a number of seconds, 5 or more")?,
                    );
                }
                "--cars" => {
                    let v = it.next().ok_or("--cars needs a value")?;
                    cars = v
                        .parse::<usize>()
                        .ok()
                        .filter(|n| (1..=64).contains(n))
                        .ok_or("--cars must be a number from 1 to 64")?;
                }
                "--subscribe" => subscribe = true,
                other => return Err(format!("unknown argument {other}")),
            }
        }
        Ok(Config {
            seconds: seconds.ok_or("--synthetic <seconds> is required")?,
            cars,
            subscribe,
        })
    }

    /// The session: garage, pit exit, then laps of 60 s until `seconds` are filled; the other
    /// cars cross the line once for every lap of the player.
    fn scenario(cfg: &Config) -> Scenario {
        let lap_s = 60.0;
        let yaml = YamlBuilder::new()
            .pace_car(None)
            .roster(cfg.cars, if cfg.cars > 1 { 2 } else { 1 });
        let mut s = Scenario::new(yaml);
        s.field(cfg.cars - 1);
        s.garage(0.5).to_stall().drive_out();
        let player = s.yaml().player();
        let rivals: Vec<usize> = s
            .yaml()
            .slots()
            .into_iter()
            .filter(|i| *i != player)
            .map(|i| i as usize)
            .collect();
        let target = (cfg.seconds * 60.0) as usize;
        let mut k = 0u32;
        while s.rows().len() < target {
            s.lap(LapProfile::new(lap_s));
            for (n, idx) in rivals.iter().enumerate() {
                let t = lap_s as f32 + 0.37 * (n as f32 + 1.0) + k as f32 * 0.01;
                s.rival(*idx).lap(t);
            }
            k += 1;
        }
        s
    }

    fn percentile(sorted: &[u32], p: f64) -> u32 {
        let i = ((sorted.len() as f64 - 1.0) * p).round() as usize;
        sorted[i]
    }

    fn mib(bytes: usize) -> f64 {
        bytes as f64 / (1024.0 * 1024.0)
    }

    pub fn run() -> ExitCode {
        let args: Vec<String> = std::env::args().skip(1).collect();
        let cfg = match parse(&args) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("iracing-measure: {e}\n{USAGE}");
                return ExitCode::from(2);
            }
        };
        let s = scenario(&cfg);
        let rows = s.rows().len();
        let row_bytes = s.rows().first().map_or(0, Vec::len);
        let held = rows * row_bytes;
        let tag = std::process::id();
        let names = SdkNames {
            map: format!("stintmate_measure_{tag}_map"),
            event: format!("stintmate_measure_{tag}_event"),
        };
        let size = TestMap::required_size(s.layout(), s.initial_yaml().len() + 64 * 1024);
        let mut map =
            match TestMap::create(&names.map, &names.event, s.layout(), s.initial_yaml(), size) {
                Ok(m) => m,
                Err(e) => {
                    eprintln!("iracing-measure: cannot create the test mapping: {e}");
                    return ExitCode::from(1);
                }
            };
        let stop = Arc::new(AtomicBool::new(false));
        let done = Arc::new(AtomicBool::new(false));
        let feeder_cpu = Arc::new(Mutex::new(0.0f64));
        // The feeder plays the simulator at 60 rows per second.
        let feeder = {
            let (stop, done, cpu) = (stop.clone(), done.clone(), feeder_cpu.clone());
            std::thread::spawn(move || {
                let n = s.replay_into(&mut map, 60.0, &stop);
                *cpu.lock().unwrap() = thread_cpu();
                done.store(true, Ordering::SeqCst);
                (n, s)
            })
        };

        let mut reader = match Reader::open(Some(&names)) {
            Ok(r) => r,
            Err(e) => {
                stop.store(true, Ordering::SeqCst);
                eprintln!("iracing-measure: cannot read the test mapping: {e}");
                return ExitCode::from(1);
            }
        };
        let mut pipe = Pipeline::new(Counts::default(), Box::new(WallClock::new()));
        let ring = Arc::new(Ring::default());
        ring.set_subscribed(cfg.subscribe);
        ring.set_idle(false);
        pipe.live = Some(Arc::new(LiveBuffer::default()));
        pipe.ring = Some(ring.clone());
        let (outlet, fired) = announce::channel();
        pipe.announce = Some(outlet);

        // The consumer of the telemetry ring and of the announcements, as the sender threads are.
        let drain_stop = Arc::new(AtomicBool::new(false));
        let drain = {
            let (ring, stop) = (ring.clone(), drain_stop.clone());
            std::thread::spawn(move || {
                let (mut samples, mut calls) = (0u64, 0u64);
                while !stop.load(Ordering::Relaxed) {
                    while ring.pop().is_some() {
                        samples += 1;
                    }
                    while fired.try_recv().is_ok() {
                        calls += 1;
                    }
                    std::thread::sleep(Duration::from_millis(2));
                }
                (samples, calls)
            })
        };

        let mut steps: Vec<u32> = Vec::with_capacity(rows + 1024);
        let (mut alloc_total, mut alloc_rows, mut max_allocs) = (0u64, 0u64, 0u64);
        let (mut torn, mut resets) = (0u32, 0u32);
        let (ws_before, _) = memory();
        let (proc0, thread0, wall0) = (process_cpu(), thread_cpu(), Instant::now());
        let deadline = Instant::now() + Duration::from_secs_f64(cfg.seconds + 60.0);
        loop {
            N.with(|n| n.set(0));
            COUNT.with(|c| c.set(true));
            let (poll, step) = reader.poll_timed(Duration::from_millis(25), &mut pipe);
            COUNT.with(|c| c.set(false));
            match poll {
                Poll::Row => {
                    steps.push(step.as_nanos().min(u128::from(u32::MAX)) as u32);
                    let n = N.with(Cell::get);
                    alloc_total += n;
                    alloc_rows += u64::from(n > 0);
                    max_allocs = max_allocs.max(n);
                }
                Poll::Torn => torn += 1,
                Poll::Reset => resets += 1,
                Poll::NoNewRow | Poll::Disconnected => {
                    if done.load(Ordering::SeqCst) {
                        break;
                    }
                }
            }
            if Instant::now() > deadline {
                eprintln!("iracing-measure: the replay did not finish in time");
                break;
            }
        }
        let (proc1, thread1, wall) = (process_cpu(), thread_cpu(), wall0.elapsed().as_secs_f64());
        let (ws_after, ws_peak) = memory();
        stop.store(true, Ordering::SeqCst);
        // The scenario stays alive until here so its rows count in the memory figures.
        let pushed = feeder.join().map_or(0, |(n, kept)| {
            drop(kept);
            n
        });
        drain_stop.store(true, Ordering::SeqCst);
        let (samples, calls) = drain.join().unwrap_or((0, 0));
        let feeder_cpu = *feeder_cpu.lock().unwrap();

        if steps.is_empty() {
            eprintln!("iracing-measure: no row was processed");
            return ExitCode::from(1);
        }
        let n = steps.len();
        steps.sort_unstable();
        let us = |ns: u32| f64::from(ns) / 1000.0;
        let loop_cpu = thread1 - thread0;
        let recorder_cpu = (proc1 - proc0) - feeder_cpu;
        let busy_s: f64 = steps.iter().map(|&ns| f64::from(ns)).sum::<f64>() * 1e-9;
        let view = pipe.tracker.view();
        println!(
            "iracing-measure: synthetic replay of {} s, {} cars, telemetry stream {}",
            cfg.seconds,
            cfg.cars,
            if cfg.subscribe { "on" } else { "off" }
        );
        println!(
            "rows pushed {pushed}, processed {n}, torn {torn}, resets {resets}, row {row_bytes} bytes"
        );
        println!(
            "recorded: {} session(s), {} lap(s) written, {} setup(s), {} pit exit(s), {} participant row(s); laps in the tracker {}",
            pipe.sink.sessions,
            pipe.sink.laps,
            pipe.sink.setups,
            pipe.sink.pit_exits,
            pipe.sink.participant_rows,
            view.laps_recorded
        );
        println!("telemetry samples drained {samples}, announcements fired {calls}");
        println!(
            "wall {wall:.1} s (the system accounts thread time in clock ticks of about 15.6 ms: the CPU figures below are statistical, the step times are exact)"
        );
        println!(
            "cpu, sampling thread: {loop_cpu:.3} s = {:.2} % of one core",
            100.0 * loop_cpu / wall
        );
        println!(
            "cpu, recorder side (every thread of this process but the feeder): {recorder_cpu:.3} s = {:.2} % of one core",
            100.0 * recorder_cpu / wall
        );
        println!(
            "step per row (wait returned to row published), microseconds: p50 {:.1}, p99 {:.1}, max {:.1}; the steps add up to {busy_s:.3} s = {:.3} % of one core",
            us(percentile(&steps, 0.5)),
            us(percentile(&steps, 0.99)),
            us(steps[n - 1]),
            100.0 * busy_s / wall
        );
        println!(
            "allocations on the sampling thread: {alloc_total} in {n} rows ({:.4} per row); {alloc_rows} row(s) allocated, at most {max_allocs} in one row",
            alloc_total as f64 / n as f64
        );
        println!(
            "working set of this process: {:.1} MiB before, {:.1} MiB after, peak {:.1} MiB; the replayed rows held in memory: {:.1} MiB",
            mib(ws_before),
            mib(ws_after),
            mib(ws_peak),
            mib(held)
        );
        ExitCode::SUCCESS
    }
}
