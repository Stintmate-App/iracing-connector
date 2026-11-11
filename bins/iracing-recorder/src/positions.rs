//! Exact lap positions from the simulator's disk telemetry.
//!
//! While driving, the sink keeps the address of every sample of a lap in a positions file per
//! session (`<data folder>\positions\<session key>.bin`, see
//! [`iracing_core::positions::file`]). This module appends to those files ([`open_file`],
//! [`append`]) and runs the background thread that, after a session ended and while no session
//! is being driven, reads the `.ibt` files iRacing wrote for it, projects the matched laps and
//! hands them to the sampling loop ([`Ready`]), which queues them as `lap_positions` messages.
//!
//! The thread only reads under the user's `Documents\iRacing\telemetry` and never writes there.
//! It is a tolerance of this connector that a disk file may start up to
//! [`START_SLACK_SECS`](iracing_core::positions::select::START_SLACK_SECS) before the session
//! opened (the file's start date has one-second resolution), and that a lap whose `SessionTick`
//! was missing on live rows is left unmatched.

use std::collections::{BTreeSet, HashMap};
use std::fs::{self, File, OpenOptions};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use iracing_core::positions::file::{
    Decision, PositionsFile, PositionsHeader, encode_header, encode_resolved,
};
use iracing_core::positions::job::{self, LapPositions, Outcome};
use iracing_core::positions::select::{FileFacts, Readiness, Stability, belongs, readiness};
use irsdk::scan::Scan;

/// How often the thread wakes.
pub const WAKE_EVERY: Duration = Duration::from_secs(30);

/// A positions file written to within this many seconds is left alone: its session just ended
/// and the writer thread may still be appending the last laps.
const QUIET_SECS: i64 = 60;

/// The folder of the positions files for an outbox folder: next to it, in the data folder.
pub fn dir_of(outbox_dir: &Path) -> PathBuf {
    outbox_dir.parent().unwrap_or(outbox_dir).join("positions")
}

/// Wall-clock time as Unix seconds.
pub fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

fn file_name(session_key: &str) -> String {
    let safe: String = session_key
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    format!("{safe}.bin")
}

/// Starts the positions file of a session: writes the header into `dir`, unless the file is
/// already there (a resumed session continues its file). Returns its path, or none when the
/// folder or file cannot be made (logged).
pub fn open_file(dir: &Path, header: &PositionsHeader) -> Option<PathBuf> {
    if let Err(e) = fs::create_dir_all(dir) {
        tracing::warn!("Cannot make the positions folder {}: {e}", dir.display());
        return None;
    }
    let path = dir.join(file_name(&header.session_key));
    match OpenOptions::new().write(true).create_new(true).open(&path) {
        Ok(mut f) => {
            if let Err(e) = f.write_all(&encode_header(header)) {
                tracing::warn!("Cannot write {}: {e}", path.display());
                return None;
            }
            Some(path)
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Some(path),
        Err(e) => {
            tracing::warn!("Cannot make {}: {e}", path.display());
            None
        }
    }
}

/// Appends one record to a positions file. A file that is gone is not made again (it was
/// finished and deleted); a failure is logged and the record is lost.
pub fn append(path: &Path, record: &[u8]) {
    let result = OpenOptions::new()
        .append(true)
        .open(path)
        .and_then(|mut f| f.write_all(record));
    if let Err(e) = result {
        tracing::debug!("Cannot append to {}: {e}", path.display());
    }
}

/// The user's Documents folder (`FOLDERID_Documents`), wherever the user moved it.
#[cfg(windows)]
pub fn documents_dir() -> Option<PathBuf> {
    use windows_sys::Win32::System::Com::CoTaskMemFree;
    use windows_sys::Win32::UI::Shell::{FOLDERID_Documents, SHGetKnownFolderPath};
    let mut out: *mut u16 = std::ptr::null_mut();
    // SAFETY: `out` receives a CoTaskMem-allocated string that is freed below.
    let hr =
        unsafe { SHGetKnownFolderPath(&FOLDERID_Documents, 0, std::ptr::null_mut(), &mut out) };
    let result = if hr == 0 && !out.is_null() {
        // SAFETY: on success `out` is a NUL-terminated UTF-16 string.
        let len = (0..).take_while(|&i| unsafe { *out.add(i) } != 0).count();
        let s = String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(out, len) });
        (!s.is_empty()).then(|| PathBuf::from(s))
    } else {
        None
    };
    // SAFETY: frees the string the call allocated (valid for null too).
    unsafe { CoTaskMemFree(out.cast()) };
    result
}

/// The user's Documents folder; none off Windows.
#[cfg(not(windows))]
pub fn documents_dir() -> Option<PathBuf> {
    None
}

/// iRacing's disk telemetry folder: `Documents\iRacing\telemetry`.
pub fn default_telemetry_dir() -> Option<PathBuf> {
    documents_dir().map(|d| d.join("iRacing").join("telemetry"))
}

/// Puts the calling thread in the background mode (low CPU, I/O and memory priority).
#[cfg(windows)]
fn background_mode() {
    use windows_sys::Win32::System::Threading::{
        GetCurrentThread, SetThreadPriority, THREAD_MODE_BACKGROUND_BEGIN,
    };
    // SAFETY: the pseudo handle of the calling thread.
    unsafe { SetThreadPriority(GetCurrentThread(), THREAD_MODE_BACKGROUND_BEGIN) };
}

#[cfg(not(windows))]
fn background_mode() {}

/// The exact positions of one lap, found on disk, for the sampling loop to queue.
pub struct Ready {
    /// The session key the lap was sent under.
    pub session_key: String,
    /// The lap's number in the session.
    pub lap_seq: u64,
    /// The positions file to mark the lap resolved in once it is queued.
    pub file: PathBuf,
    /// The positions.
    pub positions: LapPositions,
}

/// Marks a lap resolved in its positions file.
pub fn mark_resolved(file: &Path, lap_seq: u64) {
    append(file, &encode_resolved(lap_seq));
}

/// The work of the thread, apart from the thread: one [`Worker::pass`] per wake.
pub struct Worker {
    positions_dir: PathBuf,
    telemetry_dir: Box<dyn Fn() -> Option<PathBuf> + Send>,
    driving: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    out: Sender<Ready>,
    stability: HashMap<PathBuf, Stability>,
    /// The laps already handed over, by positions file, until the file shows them resolved.
    handed: HashMap<PathBuf, BTreeSet<u64>>,
    started: Instant,
}

impl Worker {
    /// A worker over the positions in `positions_dir` and the disk files `telemetry_dir` names.
    pub fn new(
        positions_dir: PathBuf,
        telemetry_dir: Box<dyn Fn() -> Option<PathBuf> + Send>,
        driving: Arc<AtomicBool>,
        stop: Arc<AtomicBool>,
        out: Sender<Ready>,
    ) -> Worker {
        Worker {
            positions_dir,
            telemetry_dir,
            driving,
            stop,
            out,
            stability: HashMap::new(),
            handed: HashMap::new(),
            started: Instant::now(),
        }
    }

    fn should_stop(&self) -> bool {
        self.stop.load(Ordering::Relaxed) || self.driving.load(Ordering::Relaxed)
    }

    /// One look at the positions files: deletes the finished and the expired ones and works on
    /// those of ended sessions. Does nothing while a session is being driven.
    pub fn pass(&mut self) {
        if self.should_stop() {
            return;
        }
        let Ok(entries) = fs::read_dir(&self.positions_dir) else {
            return;
        };
        let now = unix_now();
        let mut files: Vec<PathBuf> = entries
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e == "bin"))
            .collect();
        files.sort();
        for path in files {
            if self.should_stop() {
                return;
            }
            self.one(&path, now);
        }
    }

    fn one(&mut self, path: &Path, now: i64) {
        let Ok(bytes) = fs::read(path) else {
            return;
        };
        let modified = fs::metadata(path)
            .ok()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map_or(now, |d| i64::try_from(d.as_secs()).unwrap_or(now));
        let mut file = match PositionsFile::decode(&bytes) {
            Ok(f) => f,
            Err(e) => {
                // Not ours, or from a newer version: left alone until it is old.
                tracing::debug!("Positions file {} not read: {e}", path.display());
                if now.saturating_sub(modified) > iracing_core::positions::file::EXPIRY_SECS {
                    let _ = fs::remove_file(path);
                }
                return;
            }
        };
        if file.decide(now) == Decision::Delete {
            tracing::debug!(
                "Positions file of session {} deleted",
                file.header.session_key
            );
            let _ = fs::remove_file(path);
            self.handed.remove(path);
            return;
        }
        if !file.is_closed() || now.saturating_sub(modified) < QUIET_SECS {
            return;
        }
        if let Some(handed) = self.handed.get_mut(path) {
            // What the file shows resolved needs no more remembering.
            handed.retain(|s| !file.resolved.contains(s));
            file.resolved.extend(handed.iter().copied());
        }
        if file.pending().next().is_none() {
            return;
        }
        self.work(path, &file, now);
    }

    /// The disk files that belong to the session of `file` and are ready to read; none when one
    /// of them is not ready yet (it is looked at again at the next wake).
    fn scans_for(&mut self, file: &PositionsFile, now: i64) -> Option<Vec<Scan<File>>> {
        let dir = (self.telemetry_dir)()?;
        let entries = fs::read_dir(&dir).ok()?;
        let header = &file.header;
        let now_s = i64::try_from(self.started.elapsed().as_secs()).unwrap_or(i64::MAX);
        let mut candidates: Vec<(i64, PathBuf)> = Vec::new();
        for entry in entries.filter_map(Result::ok) {
            let p = entry.path();
            if !p.extension().is_some_and(|e| e.eq_ignore_ascii_case("ibt")) {
                continue;
            }
            let modified = entry
                .metadata()
                .ok()
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map_or(now, |d| i64::try_from(d.as_secs()).unwrap_or(now));
            if modified >= header.opened_unix {
                candidates.push((modified, p));
            }
        }
        candidates.sort_by_key(|(_, p)| p.clone());
        let mut scans = Vec::new();
        let mut waiting = false;
        for (modified, p) in candidates {
            let Ok(scan) = Scan::open(&p) else {
                continue;
            };
            let facts = FileFacts::of(&scan, modified);
            if let Err(why) = belongs(header, file.closed_unix, &facts) {
                tracing::debug!(
                    "{} is not of session {}: {why:?}",
                    p.display(),
                    header.session_key
                );
                continue;
            }
            let st = self.stability.entry(p.clone()).or_default();
            match readiness(&facts, st, now_s) {
                Readiness::Ready => scans.push(scan),
                Readiness::Wait => waiting = true,
            }
        }
        if waiting {
            tracing::debug!(
                "A disk file of session {} is not complete yet",
                header.session_key
            );
            return None;
        }
        Some(scans)
    }

    fn work(&mut self, path: &Path, file: &PositionsFile, now: i64) {
        let Some(mut scans) = self.scans_for(file, now) else {
            return;
        };
        if scans.is_empty() {
            tracing::debug!(
                "No disk telemetry found for session {}",
                file.header.session_key
            );
            return;
        }
        let (stop, driving) = (self.stop.clone(), self.driving.clone());
        let stop_now = move || stop.load(Ordering::Relaxed) || driving.load(Ordering::Relaxed);
        let mut found: Vec<LapPositions> = Vec::new();
        let report = job::run(file, &mut scans, &stop_now, &mut |p| found.push(p));
        if report.stopped {
            tracing::debug!(
                "Positions job of session {} stopped",
                file.header.session_key
            );
            return;
        }
        let key = &file.header.session_key;
        let (mut sent, mut left) = (0usize, 0usize);
        for (lap_seq, outcome) in &report.outcomes {
            match outcome {
                Outcome::Sent => sent += 1,
                Outcome::Left(reason) => {
                    left += 1;
                    tracing::debug!("Lap {lap_seq} of session {key} keeps its positions: {reason}");
                    // Final: every file of the session was complete and unchanged.
                    mark_resolved(path, *lap_seq);
                }
            }
        }
        let handed = self.handed.entry(path.to_owned()).or_default();
        for p in found {
            handed.insert(p.lap_seq);
            let ready = Ready {
                session_key: key.clone(),
                lap_seq: p.lap_seq,
                file: path.to_owned(),
                positions: p,
            };
            if self.out.send(ready).is_err() {
                return;
            }
        }
        tracing::info!(
            "Exact positions from disk telemetry: {sent} of {} laps of session {key} ({left} left with their recorded positions)",
            report.outcomes.len()
        );
    }
}

/// The running thread and the receiving end of what it finds.
pub struct Positions {
    /// Laps with exact positions, to queue.
    pub ready: Receiver<Ready>,
    stop: Arc<AtomicBool>,
    thread: Mutex<Option<JoinHandle<()>>>,
}

impl Positions {
    /// Starts the thread over `positions_dir` and the default disk telemetry folder. `driving` is
    /// set by the sampling loop while a session is open.
    pub fn spawn(positions_dir: PathBuf, driving: Arc<AtomicBool>) -> std::io::Result<Positions> {
        Positions::spawn_with(
            positions_dir,
            Box::new(default_telemetry_dir),
            driving,
            WAKE_EVERY,
        )
    }

    /// Starts the thread with its own folder lookup and wake interval (tests).
    pub fn spawn_with(
        positions_dir: PathBuf,
        telemetry_dir: Box<dyn Fn() -> Option<PathBuf> + Send>,
        driving: Arc<AtomicBool>,
        every: Duration,
    ) -> std::io::Result<Positions> {
        let (tx, rx) = channel();
        let stop = Arc::new(AtomicBool::new(false));
        let mut worker = Worker::new(positions_dir, telemetry_dir, driving, stop.clone(), tx);
        let flag = stop.clone();
        let thread = std::thread::Builder::new()
            .name("positions".into())
            .spawn(move || {
                background_mode();
                while !flag.load(Ordering::Relaxed) {
                    std::thread::park_timeout(every);
                    if flag.load(Ordering::Relaxed) {
                        break;
                    }
                    worker.pass();
                }
            })?;
        Ok(Positions {
            ready: rx,
            stop,
            thread: Mutex::new(Some(thread)),
        })
    }

    /// Stops the thread and waits for it.
    pub fn close(&self) {
        self.stop.store(true, Ordering::Relaxed);
        let handle = self.thread.lock().unwrap_or_else(|p| p.into_inner()).take();
        if let Some(h) = handle {
            h.thread().unpark();
            let _ = h.join();
        }
    }
}
