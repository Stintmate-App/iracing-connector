//! The recorder loop: waits for the simulator's rows, feeds the tracker and the rival tracker,
//! and reports the status. The session text is parsed on other threads.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use iracing_core::SPEC;
use iracing_core::announce::detector::Detector;
use iracing_core::announce::spotter::Spotter;
use iracing_core::announce::{Call, Calls};
use iracing_core::io::{Clock, Sink};
use iracing_core::lap::RESERVE_ROWS;
use iracing_core::meta::{MetaWorker, SessionMeta};
use iracing_core::resume::IrClock;
use iracing_core::rivals::{CatalogWatch, RivalTracker};
use iracing_core::telemetry::sample;
use iracing_core::tick::Tick;
use iracing_core::tracker::{State, Tracker};
use iracing_core::vars::Vars;
use irsdk::live::{Live, OpenError};
use irsdk::sampler::Poll;
use irsdk::worker::{YamlSource, YamlWorker};
use stintmate_connector_link::announcer::Announcer;
use stintmate_connector_link::boot::Booted;
use stintmate_connector_link::link::LinkState;
use stintmate_connector_link::sink::{CLOSE_DRAIN, WriteSink};
use stintmate_connector_link::status::{StatusInput, StatusWorker};
use stintmate_connector_platform::game::{GameWatch, game_pid};
use stintmate_connector_platform::logging::local_iso;

use stintmate_ipc::MethodOpts;
use stintmate_ipc::clock::now_ns;

use crate::announce::{self, Fired, Outlet};
use crate::cli::{Options, SdkNames};
use crate::live::LiveBuffer;
use crate::positions::mark_resolved;
use crate::serve::{self, SessionKeyCell};
use crate::sink::{NoteClock, TrackerSink, positions_message};
use crate::status::{self, Facts};
use crate::summary::OutboxSummary;
use crate::telemetry::{Ring, Telemetry};

/// How long one wait for the simulator's event lasts. A missed event costs at most this.
const WAIT: Duration = Duration::from_millis(25);
/// How often the status is filled.
const STATUS_EVERY: Duration = Duration::from_millis(500);
/// How often the outbox summary is refreshed, on its own thread.
const OUTBOX_EVERY: Duration = Duration::from_millis(250);
/// How often the simulator's process (or its memory map) is looked for while nothing is open.
const ATTACH_EVERY: Duration = Duration::from_secs(1);
/// The game token of a test mapping, which has no process.
const SDK_TOKEN: u64 = 1;

/// The wall clock the tracker stamps with.
pub struct WallClock {
    start: Instant,
}

impl WallClock {
    /// A clock whose monotonic zero is now.
    pub fn new() -> WallClock {
        WallClock {
            start: Instant::now(),
        }
    }
}

impl Default for WallClock {
    fn default() -> Self {
        Self::new()
    }
}

impl Clock for WallClock {
    fn iso(&self) -> String {
        local_iso(false)
    }

    fn mono_s(&self) -> f64 {
        self.start.elapsed().as_secs_f64()
    }
}

/// Seconds since 1970 on the system clock.
pub fn wall_s() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |d| d.as_secs_f64())
}

/// Everything a row passes through after it became a [`Tick`]: the tracker, the rivals and the
/// catalog watch, writing to `S`.
pub struct Pipeline<S: Sink + NoteClock> {
    /// The row being processed.
    pub tick: Box<Tick>,
    /// The lap tracker.
    pub tracker: Tracker,
    /// The sink the records go to.
    pub sink: S,
    /// The latest session description.
    pub meta: Option<Arc<SessionMeta>>,
    /// Where each row is published for `connector.live_frame`, when something serves it.
    pub live: Option<Arc<LiveBuffer>>,
    /// Where each row's sample goes for `connector.telemetry`, when something serves it.
    pub ring: Option<Arc<Ring>>,
    /// Where the fired announcements go; without it the detectors still run and say nothing.
    pub announce: Option<Outlet>,
    /// The monotonic time of the row in nanoseconds, taken right after the wait returned.
    pub t_ns: i64,
    clock: Box<dyn Clock>,
    rivals: RivalTracker,
    catalog: CatalogWatch,
    laps_seen: u32,
    frames: u32,
    detector: Detector,
    spotter: Spotter,
    calls: Calls,
    green_s: Option<f64>,
}

/// How often the other cars are read for the flags' causes and the rivals' flags.
const RIVAL_HZ: f64 = 2.0;

impl<S: Sink + NoteClock> Pipeline<S> {
    /// A pipeline writing to `sink` and reading time from `clock`.
    pub fn new(sink: S, clock: Box<dyn Clock>) -> Pipeline<S> {
        Pipeline {
            tick: Tick::new(),
            tracker: Tracker::new(RESERVE_ROWS),
            sink,
            meta: None,
            live: None,
            ring: None,
            announce: None,
            t_ns: 0,
            clock,
            rivals: RivalTracker::new(),
            catalog: CatalogWatch::new(),
            laps_seen: 0,
            frames: 0,
            detector: Detector::new(RIVAL_HZ),
            spotter: Spotter::new(),
            calls: Calls::new(),
            green_s: None,
        }
    }

    /// Hands the calls the detectors made to the sender and empties the list. Nothing is
    /// allocated unless a call fires.
    fn flush(&mut self) {
        if self.calls.is_empty() {
            return;
        }
        if let Some(out) = &self.announce {
            let length = self.meta.as_deref().and_then(|m| m.track.length_m);
            let lap_dist = length.map(|l| (f64::from(self.tick.lap_dist_pct) * l) as f32);
            for call in self.calls.as_slice() {
                out.push(Fired {
                    call: Call::clone(call),
                    session_time: self.tick.session_time,
                    lap: self.tick.lap,
                    lap_dist,
                });
            }
        }
        self.calls.clear();
    }

    /// The detectors read the row. Hot: allocates nothing while no call fires.
    fn announce_row(&mut self) {
        if !self.tick.connected {
            return;
        }
        let now = self.tick.session_time;
        if self.tick.session_state == 4 {
            self.green_s.get_or_insert(now);
        } else {
            self.green_s = None;
        }
        let meta = self.meta.as_deref();
        self.detector.step(&self.tick, meta, &mut self.calls);
        self.spotter
            .step(&self.tick, meta, self.green_s, &mut self.calls);
        self.flush();
    }

    /// A new session description arrived: the mid-session catalog goes out before any row that
    /// names a new car.
    pub fn set_meta(&mut self, meta: Arc<SessionMeta>) {
        self.catalog.offer(&meta, &mut self.sink);
        self.detector.on_meta(&meta, &mut self.calls);
        self.flush();
        if let Some(live) = &self.live {
            live.set_meta(Some(meta.clone()));
        }
        self.meta = Some(meta);
    }

    /// Hands the row just processed to the live frame and to the telemetry stream. Hot: copies
    /// plain values into preallocated buffers, takes no lock and allocates nothing.
    pub fn publish(&mut self) {
        let view = self.tracker.view();
        if let Some(live) = &self.live {
            live.publish(
                &self.tick,
                &view,
                self.tracker.stall_tyres().copied(),
                wall_s(),
            );
        }
        if let Some(ring) = self.ring.as_ref().filter(|r| r.wanted()) {
            self.frames = self.frames.wrapping_add(1);
            let length = self
                .meta
                .as_deref()
                .and_then(|m| m.track.length_m)
                .map_or(0.0, |l| l as f32);
            ring.push(&sample(&self.tick, &view, length, self.frames), self.t_ns);
        }
    }

    /// The wall clock's reading of the row for the resume file; `None` without a description.
    pub fn ir_clock(&self) -> Option<IrClock> {
        self.meta
            .as_deref()
            .map(|m| IrClock::new(m, &self.tick, wall_s()))
    }

    /// Processes the tick as filled by the caller. Hot: allocates nothing while no lap ends
    /// and no session opens or closes.
    pub fn row(&mut self) {
        if self.tracker.session_local().is_none()
            && let Some(clock) = self.ir_clock()
        {
            self.sink.note_clock(clock);
        }
        let before = self.tracker.session_local();
        self.tracker
            .step(&self.tick, self.meta.as_ref(), &mut self.sink, &*self.clock);
        self.announce_row();
        let after = self.tracker.session_local();
        if before != after {
            self.rivals.reset();
            self.laps_seen = 0;
            match (after, self.meta.as_deref()) {
                (Some(_), Some(m)) => self.catalog.opened(m),
                _ => self.catalog.closed(),
            }
        }
        let (Some(local), Some(meta)) = (after, self.meta.as_deref()) else {
            return;
        };
        if !self.tick.connected || meta.session_num != self.tick.session_num {
            return;
        }
        self.rivals.step(&self.tick, meta);
        let laps = self.tracker.view().laps_recorded;
        let ended = laps != self.laps_seen;
        self.laps_seen = laps;
        if let Some(mut rows) = self.rivals.take_due(self.tick.session_time, ended) {
            iracing_core::rivals::apply_player_validity(&mut rows, |lap| {
                self.tracker.lap_valid(lap)
            });
            self.sink.participants(local, rows);
        }
    }

    /// The source of the rows changed (the simulator went away, a new mapping opened or its
    /// layout restarted): the description of the old one must not be matched to the rows of the
    /// new one. The tracker matches a description to rows by the session number alone, and the
    /// offline simulator numbers the sessions of every event 0.
    pub fn forget_meta(&mut self) {
        if self.meta.take().is_some()
            && let Some(live) = &self.live
        {
            live.set_meta(None);
        }
    }

    /// The simulator went away: the session ends.
    pub fn off(&mut self) {
        self.tracker.on_off(&mut self.sink, &*self.clock);
        if let Some(live) = &self.live {
            live.clear();
        }
        self.forget_meta();
        self.rivals.reset();
        self.catalog.closed();
        self.laps_seen = 0;
        self.detector.reset();
        self.spotter.reset();
        self.green_s = None;
        self.calls.clear();
    }

    /// The recorder stops. With `keep_open` the session is left open for a restarted recorder:
    /// the open lap is closed, and the sink is told the session ended after it was given the
    /// clock to keep (`keep_session_open`), which makes it store the resume file instead of
    /// sending `session_end`.
    pub fn stop(&mut self, keep_open: bool) {
        let local = self.tracker.session_local();
        self.tracker.stop(keep_open, &mut self.sink, &*self.clock);
        if keep_open && let Some(local) = local {
            self.sink.end_session(local, &self.clock.iso());
        }
    }
}

/// The open simulator mapping with the threads that parse its session text.
pub struct Reader {
    live: Live,
    yaml: Arc<YamlWorker>,
    metaw: MetaWorker,
    session_num: Arc<AtomicI32>,
    vars: Option<Vars>,
    last_update: i32,
    last_gen: u64,
}

impl Reader {
    /// Opens the simulator's mapping, or the named test mapping.
    pub fn open(names: Option<&SdkNames>) -> Result<Reader, OpenError> {
        let live = match names {
            Some(n) => Live::open_named(&n.map, &n.event)?,
            None => Live::open()?,
        };
        let source: Arc<dyn YamlSource> = Arc::new(live.view());
        let yaml = Arc::new(YamlWorker::spawn(source).map_err(|_| OpenError::Os(0))?);
        let session_num = Arc::new(AtomicI32::new(-1));
        let metaw = MetaWorker::spawn(Arc::clone(&yaml), Arc::clone(&session_num))
            .map_err(|_| OpenError::Os(0))?;
        let mut reader = Reader {
            live,
            yaml,
            metaw,
            session_num,
            vars: None,
            last_update: i32::MIN,
            last_gen: 0,
        };
        reader.resolve();
        Ok(reader)
    }

    fn resolve(&mut self) {
        self.vars = match Vars::resolve(self.live.sampler().vars()) {
            Ok(v) => {
                let missing: Vec<_> = v.missing_optional().collect();
                let has = |name: &str| !missing.contains(&name);
                let velocity = has("VelocityX") && has("VelocityY");
                let heading = if has("YawNorth") {
                    Some("YawNorth")
                } else if has("Yaw") {
                    Some("Yaw")
                } else {
                    None
                };
                match heading {
                    Some(h) if velocity => tracing::info!("Position: velocity and {h}"),
                    Some(h) => tracing::info!("Position: speed and {h} (no VelocityX/VelocityY)"),
                    None => tracing::info!("Position: none (no YawNorth or Yaw)"),
                }
                Some(v)
            }
            Err(e) => {
                tracing::error!("The simulator's variables are not usable: {e}");
                None
            }
        };
    }

    /// Whether the variables the recorder needs were found.
    pub fn supported(&self) -> bool {
        self.vars.is_some()
    }

    /// Whether the simulator reports itself connected.
    pub fn connected(&self) -> bool {
        self.live.sampler().connected()
    }

    /// Waits for a row for at most `wait` and passes what comes through `pipe`.
    pub fn poll<S: Sink + NoteClock>(&mut self, wait: Duration, pipe: &mut Pipeline<S>) -> Poll {
        self.poll_impl::<false, S>(wait, pipe).0
    }

    /// Like [`Reader::poll`], and also returns how long the row took to process: from the
    /// moment the wait returned to the end of the row's publication (zero when no row came).
    /// For measuring; the recorder's loop calls [`Reader::poll`], which reads no clock.
    pub fn poll_timed<S: Sink + NoteClock>(
        &mut self,
        wait: Duration,
        pipe: &mut Pipeline<S>,
    ) -> (Poll, Duration) {
        self.poll_impl::<true, S>(wait, pipe)
    }

    fn poll_impl<const TIMED: bool, S: Sink + NoteClock>(
        &mut self,
        wait: Duration,
        pipe: &mut Pipeline<S>,
    ) -> (Poll, Duration) {
        let poll = self.live.wait(wait);
        let began = TIMED.then(Instant::now);
        let mut step = Duration::ZERO;
        match poll {
            Poll::Row => {
                pipe.t_ns = now_ns();
                let Some(vars) = self.vars.as_ref() else {
                    return (poll, step);
                };
                let sampler = self.live.sampler();
                pipe.tick.fill(vars, sampler.row());
                pipe.tick.connected = sampler.connected();
                pipe.tick.fresh = true;
                self.session_num
                    .store(pipe.tick.session_num, Ordering::Relaxed);
                let update = sampler.session_info_update();
                if update != self.last_update {
                    self.last_update = update;
                    self.yaml.notify(update);
                }
                let generation = self.metaw.generation();
                if generation != self.last_gen {
                    // A description is taken only when it was built from the text the simulator
                    // holds now; a newer text is on its way and the description is retried.
                    match self.metaw.latest() {
                        Some(meta) if meta.update == update => {
                            self.last_gen = generation;
                            pipe.set_meta(meta);
                        }
                        Some(_) => {}
                        None => self.last_gen = generation,
                    }
                }
                pipe.row();
                pipe.publish();
                if let Some(t) = began {
                    step = t.elapsed();
                }
            }
            Poll::Reset => {
                pipe.forget_meta();
                self.resolve();
                self.last_update = i32::MIN;
            }
            Poll::Disconnected => {
                pipe.t_ns = now_ns();
                pipe.tick.connected = false;
                pipe.row();
                pipe.publish();
            }
            Poll::NoNewRow | Poll::Torn => {}
        }
        (poll, step)
    }
}

fn pitwall_text(state: LinkState) -> &'static str {
    match state {
        LinkState::Connected => "connected",
        LinkState::Waiting => "waiting",
        LinkState::Refused => "refused",
    }
}

/// Runs until the stop event; the exit code of the process.
pub fn run(booted: Booted, opts: &Options) -> i32 {
    tracing::info!(
        "Recorder {} started{}",
        SPEC.version,
        if opts.sdk.is_some() {
            " (reading a test mapping)"
        } else {
            ""
        }
    );
    let outbox = booted.location.data_dir.join("outbox");
    let mut write = match WriteSink::<IrClock>::new(&outbox, booted.pitwall.clone(), None, SPEC.id)
    {
        Ok(w) => w,
        Err(e) => {
            tracing::error!("Cannot open the outbox {}: {e}", outbox.display());
            booted.pitwall.close();
            if let Some(control) = &booted.control {
                control.close();
            }
            return 1;
        }
    };
    // The descriptions of the simulator data kinds; the runtime resends them for every session.
    write.sim_data_kinds(iracing_core::sim_data::kinds());
    let worker = StatusWorker::new(booted.status);
    // The sampling thread never takes the outbox lock (it is held while a lap is written to the
    // disk): the summary is read on a thread of its own, and the loop copies the last one.
    let outbox_summary = {
        let outbox = write.outbox();
        OutboxSummary::spawn(OUTBOX_EVERY, move |s| {
            outbox.with_summary(|pending, oldest, rejected| {
                s.pending = pending;
                s.rejected = rejected;
                s.oldest_at.clear();
                s.oldest_at.push_str(oldest.unwrap_or_default());
            });
        })
    };
    let mut pipe = Pipeline::new(TrackerSink::new(write), Box::new(WallClock::new()));
    // After a session ended, the thread looks for the simulator's disk telemetry of it and finds
    // exact positions for its laps. It works only while no session is open (`driving`).
    let driving = Arc::new(AtomicBool::new(false));
    let positions = match crate::positions::Positions::spawn(
        crate::positions::dir_of(&outbox),
        driving.clone(),
    ) {
        Ok(p) => Some(p),
        Err(e) => {
            tracing::warn!("Cannot start the positions thread: {e}");
            None
        }
    };

    // What Pitwall can ask for is registered before the link starts: the live frame, the
    // capture target, the actions and the telemetry stream.
    let live = Arc::new(LiveBuffer::default());
    let ring = Arc::new(Ring::default());
    let session_key: SessionKeyCell = Arc::default();
    let io_err = |e: stintmate_ipc::Error| e.to_string();
    let served = (|| -> Result<(Telemetry, Arc<Announcer>), String> {
        let l = live.clone();
        booted
            .pitwall
            .client
            .method(
                "connector.live_frame",
                MethodOpts::default().inline(true),
                move |_ctx, _params| l.answer(),
            )
            .map_err(io_err)?;
        let announcer = serve::install(&booted.pitwall, session_key.clone(), booted.started_at)?;
        let telemetry = Telemetry::install(&booted.pitwall.client, ring.clone()).map_err(io_err)?;
        Ok((telemetry, announcer))
    })();
    let (telemetry, announcer) = match served {
        Ok(t) => t,
        Err(e) => {
            tracing::error!("Cannot register the methods Pitwall calls: {e}");
            outbox_summary.close();
            pipe.sink.write.close(CLOSE_DRAIN);
            worker.close();
            booted.pitwall.close();
            if let Some(control) = &booted.control {
                control.close();
            }
            return 1;
        }
    };
    let (outlet, fired) = announce::channel();
    let sender = match announce::spawn_sender(fired, announcer) {
        Ok(h) => Some(h),
        Err(e) => {
            tracing::error!("Cannot start the announcement sender: {e}");
            None
        }
    };
    pipe.announce = sender.is_some().then_some(outlet);
    pipe.live = Some(live);
    pipe.ring = Some(ring.clone());
    booted.pitwall.start();
    if let Some(control) = &booted.control {
        control.start();
    }
    let mut key_seen: Option<String> = None;
    let mut watch = GameWatch::new(&SPEC.game);
    let mut input = StatusInput::default();
    let mut reader: Option<Reader> = None;
    let mut game_running = opts.sdk.is_some();
    let (mut next_attach, mut next_status) = (Instant::now(), Instant::now());
    let mut first = true;

    while !booted.stop.is_set() {
        let now = Instant::now();
        if reader.is_none() {
            if now >= next_attach {
                next_attach = now + ATTACH_EVERY;
                game_running = opts.sdk.is_some() || watch.running(false);
                if game_running {
                    match Reader::open(opts.sdk.as_ref()) {
                        Ok(r) => {
                            let token = opts.sdk.as_ref().map_or_else(
                                || game_pid(&SPEC.game).map(u64::from),
                                |_| Some(SDK_TOKEN),
                            );
                            pipe.sink.write.set_game_token(token);
                            pipe.forget_meta();
                            tracing::info!("Attached to the simulator");
                            reader = Some(r);
                        }
                        Err(OpenError::NotRunning) => {}
                        Err(e) => tracing::warn!("Cannot read the simulator: {e}"),
                    }
                }
            }
        } else if let Some(r) = reader.as_mut() {
            r.poll(WAIT, &mut pipe);
            if now >= next_attach {
                next_attach = now + ATTACH_EVERY;
                if opts.sdk.is_none() && !watch.running(false) {
                    tracing::info!("The simulator is gone");
                    game_running = false;
                    pipe.off();
                    reader = None;
                }
            }
        }
        if now >= next_status || first {
            next_status = now + STATUS_EVERY;
            let local = pipe.tracker.session_local();
            let open = local.is_some();
            driving.store(open, Ordering::Relaxed);
            if let Some(p) = &positions {
                // Queued here, on the thread that owns the sink, a moment after they are found.
                while let Ok(r) = p.ready.try_recv() {
                    let (file, lap_seq) = (r.file, r.lap_seq);
                    pipe.sink.write.lap_positions(
                        &r.session_key,
                        lap_seq,
                        Box::new(move || {
                            mark_resolved(&file, lap_seq);
                            positions_message(r.positions)
                        }),
                    );
                }
            }
            let sink = &pipe.sink.write;
            let stats = sink.stats();
            let facts = Facts {
                game_running,
                pitwall: pitwall_text(booted.pitwall.state().0),
                uptime_s: booted.started_at.elapsed().as_secs_f64().floor(),
                mapped: reader.as_ref().is_some_and(Reader::connected),
                unsupported: reader.as_ref().is_some_and(|r| !r.supported()),
                replay: pipe.tick.is_replay,
                view: pipe.tracker.view(),
                session_open: open,
                meta: pipe.meta.as_deref(),
            };
            status::fill(&mut input, &facts);
            outbox_summary.with(|o| {
                input.plain.outbox_pending = o.pending;
                input.plain.outbox_rejected = o.rejected;
                input
                    .oldest_at
                    .assign((!o.oldest_at.is_empty()).then_some(o.oldest_at.as_str()));
            });
            let p = &mut input.plain;
            p.laps_queued = stats.laps_queued;
            p.laps_sent = stats.laps_sent;
            p.laps_rejected = stats.laps_rejected;
            p.stint = sink.stint(local);
            p.session_id = sink.remote_id(local);
            sink.with_session_key(local, |k| input.session_key.assign(k));
            if input.session_key.get() != key_seen.as_deref() {
                // Changes with a session: the announcer's copy, off the per-row path.
                key_seen = input.session_key.get().map(str::to_owned);
                *session_key.lock().unwrap_or_else(|p| p.into_inner()) = key_seen.clone();
            }
            input.plain.telemetry = telemetry.subscribed();
            ring.set_idle(reader.is_none());
            worker.submit(&input, first);
            first = false;
        }
        if reader.is_none() && booted.stop.wait(Duration::from_millis(100)) {
            break;
        }
    }

    tracing::info!("Stop requested; closing");
    let keep = game_running
        && reader.as_ref().is_some_and(Reader::connected)
        && pipe.tracker.state() != State::Off
        && pipe.tracker.session_local().is_some();
    pipe.sink.keep_positions_open = keep;
    if keep && let Some(clock) = pipe.ir_clock() {
        pipe.sink.write.keep_session_open(clock);
        pipe.stop(true);
    } else {
        pipe.stop(false);
    }
    if let Some(p) = &positions {
        p.close();
    }
    pipe.sink.write.close(CLOSE_DRAIN);
    outbox_summary.close();
    pipe.announce = None;
    if let Some(h) = sender {
        let _ = h.join();
    }
    drop(reader);
    telemetry.close();
    worker.close();
    booted.pitwall.close();
    if let Some(control) = &booted.control {
        control.close();
    }
    0
}
