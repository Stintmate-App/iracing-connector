//! The recorder loop: waits for the simulator's rows, feeds the tracker and the rival tracker,
//! and reports the status. The session text is parsed on other threads.

use std::sync::Arc;
use std::sync::atomic::{AtomicI32, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use iracing_core::SPEC;
use iracing_core::io::{Clock, Sink};
use iracing_core::lap::RESERVE_ROWS;
use iracing_core::meta::{MetaWorker, SessionMeta};
use iracing_core::resume::IrClock;
use iracing_core::rivals::{CatalogWatch, RivalTracker};
use iracing_core::tick::Tick;
use iracing_core::tracker::{State, Tracker};
use iracing_core::vars::Vars;
use irsdk::live::{Live, OpenError};
use irsdk::sampler::Poll;
use irsdk::worker::{YamlSource, YamlWorker};
use stintmate_connector_link::boot::Booted;
use stintmate_connector_link::link::LinkState;
use stintmate_connector_link::sink::{CLOSE_DRAIN, WriteSink};
use stintmate_connector_link::status::{StatusInput, StatusWorker};
use stintmate_connector_platform::game::{GameWatch, game_pid};
use stintmate_connector_platform::logging::local_iso;

use crate::cli::{Options, SdkNames};
use crate::sink::{NoteClock, TrackerSink};
use crate::status::{self, Facts};
use crate::summary::OutboxSummary;

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
    clock: Box<dyn Clock>,
    rivals: RivalTracker,
    catalog: CatalogWatch,
    laps_seen: u32,
}

impl<S: Sink + NoteClock> Pipeline<S> {
    /// A pipeline writing to `sink` and reading time from `clock`.
    pub fn new(sink: S, clock: Box<dyn Clock>) -> Pipeline<S> {
        Pipeline {
            tick: Tick::new(),
            tracker: Tracker::new(RESERVE_ROWS),
            sink,
            meta: None,
            clock,
            rivals: RivalTracker::new(),
            catalog: CatalogWatch::new(),
            laps_seen: 0,
        }
    }

    /// A new session description arrived: the mid-session catalog goes out before any row that
    /// names a new car.
    pub fn set_meta(&mut self, meta: Arc<SessionMeta>) {
        self.catalog.offer(&meta, &mut self.sink);
        self.meta = Some(meta);
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
        if let Some(rows) = self.rivals.take_due(self.tick.session_time, ended) {
            self.sink.participants(local, rows);
        }
    }

    /// The source of the rows changed (the simulator went away, a new mapping opened or its
    /// layout restarted): the description of the old one must not be matched to the rows of the
    /// new one. The tracker matches a description to rows by the session number alone, and the
    /// offline simulator numbers the sessions of every event 0.
    pub fn forget_meta(&mut self) {
        self.meta = None;
    }

    /// The simulator went away: the session ends.
    pub fn off(&mut self) {
        self.tracker.on_off(&mut self.sink, &*self.clock);
        self.forget_meta();
        self.rivals.reset();
        self.catalog.closed();
        self.laps_seen = 0;
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
            Ok(v) => Some(v),
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
        let poll = self.live.wait(wait);
        match poll {
            Poll::Row => {
                let Some(vars) = self.vars.as_ref() else {
                    return poll;
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
            }
            Poll::Reset => {
                pipe.forget_meta();
                self.resolve();
                self.last_update = i32::MIN;
            }
            Poll::Disconnected => {
                pipe.tick.connected = false;
                pipe.row();
            }
            Poll::NoNewRow | Poll::Torn => {}
        }
        poll
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
    booted.pitwall.start();
    if let Some(control) = &booted.control {
        control.start();
    }
    let outbox = booted.location.data_dir.join("outbox");
    let write = match WriteSink::<IrClock>::new(&outbox, booted.pitwall.clone(), None, SPEC.id) {
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
            let sink = &pipe.sink.write;
            let stats = sink.stats();
            let local = pipe.tracker.session_local();
            let open = local.is_some();
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
    if keep && let Some(clock) = pipe.ir_clock() {
        pipe.sink.write.keep_session_open(clock);
        pipe.stop(true);
    } else {
        pipe.stop(false);
    }
    pipe.sink.write.close(CLOSE_DRAIN);
    outbox_summary.close();
    drop(reader);
    worker.close();
    booted.pitwall.close();
    if let Some(control) = &booted.control {
        control.close();
    }
    0
}
