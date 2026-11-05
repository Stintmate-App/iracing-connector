//! The session text off the sampling thread.
//!
//! Parsing the session text takes milliseconds, so it never runs where rows are sampled. The
//! sampling thread calls [`YamlWorker::notify`] with the update counter it sees; a worker thread
//! copies and parses the text only when that counter moved, and publishes the result as a
//! [`SessionDoc`] that the sampling thread reads without waiting for a parse.

use std::io;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle, Thread};

use crate::ibt::Ibt;
use crate::session::SessionInfo;
use crate::view::View;
use crate::yaml::fallback::{self, Fallback};
use crate::yaml::{self, SectionHashes};

/// Offset of the update counter in the header.
const UPDATE_AT: usize = 12;
/// Offset of the session text length in the header.
const LEN_AT: usize = 16;
/// Offset of the session text in the header.
const OFFSET_AT: usize = 20;
/// Copy attempts before the worker waits for the next notice.
const COPY_TRIES: usize = 64;
/// Marks "no update wanted yet" in the atomic (an `i32` never equals it).
const NONE: i64 = i64::MIN;

/// Where the session text comes from.
pub trait YamlSource: Send + Sync {
    /// The session update counter now.
    fn update(&self) -> i32;

    /// Replaces `dst` with the raw session text (padding included). Returns `true` when
    /// [`update`](YamlSource::update) was the same before and after the copy, so the text is
    /// whole; `false` for a torn copy or a text that cannot be read.
    fn copy_yaml(&self, dst: &mut Vec<u8>) -> bool;
}

/// Copies the session text out of a header-led region, checking the counter around the copy.
fn copy_from_view<V: View + ?Sized>(view: &V, dst: &mut Vec<u8>) -> bool {
    dst.clear();
    let Some(before) = view.load_i32(UPDATE_AT) else {
        return false;
    };
    let (Some(len), Some(at)) = (view.load_i32(LEN_AT), view.load_i32(OFFSET_AT)) else {
        return false;
    };
    let (Ok(len), Ok(at)) = (usize::try_from(len), usize::try_from(at)) else {
        return false;
    };
    if at.checked_add(len).is_none_or(|end| end > view.len()) {
        return false;
    }
    dst.resize(len, 0);
    let copied = view.copy_out(at, dst);
    let after = view.load_i32(UPDATE_AT);
    if !copied || after != Some(before) {
        dst.clear();
        return false;
    }
    true
}

#[cfg(windows)]
impl YamlSource for Arc<crate::live::MapView> {
    fn update(&self) -> i32 {
        self.load_i32(UPDATE_AT).unwrap_or(0)
    }

    fn copy_yaml(&self, dst: &mut Vec<u8>) -> bool {
        copy_from_view(self, dst)
    }
}

impl YamlSource for Ibt {
    fn update(&self) -> i32 {
        self.header().session_info_update
    }

    fn copy_yaml(&self, dst: &mut Vec<u8>) -> bool {
        dst.clear();
        dst.extend_from_slice(self.yaml_raw());
        true
    }
}

/// A parsed session text.
#[derive(Debug, Clone)]
pub struct SessionDoc {
    /// The update counter the text was read at.
    pub update: i32,
    /// The decoded text.
    pub text: String,
    /// Whether the text was UTF-8 (otherwise it was decoded as cp1252).
    pub utf8: bool,
    /// The sections that parsed.
    pub info: SessionInfo,
    /// `(section, message)` for each section that did not parse.
    pub errors: Vec<(String, String)>,
    /// What a line scan found, including what the typed parse lost.
    pub fallback: Fallback,
    /// A hash of each section, to tell which part changed between two documents.
    pub sections: SectionHashes,
}

impl SessionDoc {
    /// Decodes and parses a raw session text read at `update`.
    pub fn from_raw(update: i32, raw: &[u8]) -> SessionDoc {
        let decoded = yaml::decode(raw);
        let parsed = yaml::parse(&decoded.text);
        SessionDoc {
            update,
            fallback: fallback::extract(&decoded.text),
            sections: SectionHashes::of(&decoded.text),
            text: decoded.text,
            utf8: decoded.utf8,
            info: parsed.info,
            errors: parsed.errors,
        }
    }

    /// Whether the section hashes of `self` and `other` are all equal: nothing changed but the
    /// counter.
    pub fn same_sections(&self, other: &SessionDoc) -> bool {
        self.sections == other.sections
    }
}

#[derive(Debug)]
struct Shared {
    wanted: AtomicI64,
    stop: AtomicBool,
    generation: AtomicU64,
    latest: Mutex<Option<Arc<SessionDoc>>>,
}

/// A thread that keeps the newest parsed session text.
#[derive(Debug)]
pub struct YamlWorker {
    shared: Arc<Shared>,
    waker: Thread,
    handle: Option<JoinHandle<()>>,
}

impl YamlWorker {
    /// Starts the thread `irsdk-yaml`. It reads nothing until the first [`notify`](Self::notify).
    pub fn spawn(source: Arc<dyn YamlSource>) -> io::Result<YamlWorker> {
        let shared = Arc::new(Shared {
            wanted: AtomicI64::new(NONE),
            stop: AtomicBool::new(false),
            generation: AtomicU64::new(0),
            latest: Mutex::new(None),
        });
        let theirs = Arc::clone(&shared);
        let handle = thread::Builder::new()
            .name("irsdk-yaml".into())
            .spawn(move || run(source.as_ref(), &theirs))?;
        let waker = handle.thread().clone();
        Ok(YamlWorker {
            shared,
            waker,
            handle: Some(handle),
        })
    }

    /// Tells the worker the update counter is now `update`. Hot: one atomic store and one
    /// unpark, no allocation, no waiting.
    pub fn notify(&self, update: i32) {
        self.shared
            .wanted
            .store(i64::from(update), Ordering::Release);
        self.waker.unpark();
    }

    /// The newest document, if any was parsed yet.
    pub fn latest(&self) -> Option<Arc<SessionDoc>> {
        self.shared
            .latest
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// How many documents were published.
    pub fn generation(&self) -> u64 {
        self.shared.generation.load(Ordering::Acquire)
    }

    /// Stops the thread and waits for it.
    pub fn close(self) {}
}

impl Drop for YamlWorker {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Release);
        self.waker.unpark();
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

fn run(source: &dyn YamlSource, shared: &Shared) {
    // The last counter published, and the last notice dealt with (published, found already
    // published, or given up on): the thread waits until a notice differs from the latter.
    let mut last: Option<i32> = None;
    let mut handled: Option<i64> = None;
    let mut raw = Vec::new();
    loop {
        if shared.stop.load(Ordering::Acquire) {
            return;
        }
        let wanted = shared.wanted.load(Ordering::Acquire);
        if wanted == NONE || handled == Some(wanted) {
            thread::park();
            continue;
        }
        let update = source.update();
        if last == Some(update) {
            handled = Some(wanted);
            continue;
        }
        let mut whole = false;
        for _ in 0..COPY_TRIES {
            if source.copy_yaml(&mut raw) {
                whole = true;
                break;
            }
            thread::yield_now();
        }
        if !whole {
            // A torn copy that will not settle: wait for a different notice.
            handled = Some(wanted);
            continue;
        }
        let doc = SessionDoc::from_raw(update, &raw);
        if source.update() != update {
            // The text moved while it was parsed: discard and read again.
            continue;
        }
        last = Some(update);
        handled = Some(wanted);
        *shared.latest.lock().unwrap_or_else(|e| e.into_inner()) = Some(Arc::new(doc));
        shared.generation.fetch_add(1, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicI32;
    use std::time::{Duration, Instant};

    const A: &str = "---\nWeekendInfo:\n Encoding: UTF8\n TrackName: monza\n TrackID: 7\nDriverInfo:\n DriverCarIdx: 2\n";
    const B: &str = "---\nWeekendInfo:\n Encoding: UTF8\n TrackName: monza\n TrackID: 7\nDriverInfo:\n DriverCarIdx: 3\n";

    /// A source with a text and a counter, and knobs to tear copies or move the counter.
    struct Fake {
        update: AtomicI32,
        text: Mutex<Vec<u8>>,
        /// Copies that report a torn read before one succeeds.
        tears: AtomicI32,
        /// Reads of `update` left before the counter moves by one on its own (0 = never).
        bump_after: AtomicI32,
        copies: AtomicI32,
    }

    impl Fake {
        fn new(update: i32, text: &str) -> Arc<Fake> {
            Arc::new(Fake {
                update: AtomicI32::new(update),
                text: Mutex::new(text.as_bytes().to_vec()),
                tears: AtomicI32::new(0),
                bump_after: AtomicI32::new(0),
                copies: AtomicI32::new(0),
            })
        }
        fn set(&self, update: i32, text: &str) {
            *self.text.lock().unwrap() = text.as_bytes().to_vec();
            self.update.store(update, Ordering::SeqCst);
        }
    }

    impl YamlSource for Fake {
        fn update(&self) -> i32 {
            let u = self.update.load(Ordering::SeqCst);
            let left = self.bump_after.load(Ordering::SeqCst);
            if left > 0 && self.bump_after.fetch_sub(1, Ordering::SeqCst) == 1 {
                *self.text.lock().unwrap() = B.as_bytes().to_vec();
                self.update.store(u + 1, Ordering::SeqCst);
            }
            self.update.load(Ordering::SeqCst)
        }
        fn copy_yaml(&self, dst: &mut Vec<u8>) -> bool {
            self.copies.fetch_add(1, Ordering::SeqCst);
            if self.tears.load(Ordering::SeqCst) > 0 {
                self.tears.fetch_sub(1, Ordering::SeqCst);
                dst.clear();
                return false;
            }
            dst.clear();
            dst.extend_from_slice(&self.text.lock().unwrap());
            true
        }
    }

    fn wait_for(what: &str, mut ok: impl FnMut() -> bool) {
        let end = Instant::now() + Duration::from_secs(10);
        while !ok() {
            assert!(Instant::now() < end, "timed out waiting for {what}");
            thread::sleep(Duration::from_millis(2));
        }
    }

    fn driver_idx(d: &SessionDoc) -> Option<i64> {
        d.info.driver.as_ref().and_then(|d| d.driver_car_idx)
    }

    #[test]
    fn nothing_is_read_before_a_notice() {
        let src = Fake::new(1, A);
        let w = YamlWorker::spawn(src.clone()).unwrap();
        thread::sleep(Duration::from_millis(50));
        assert!(w.latest().is_none());
        assert_eq!(src.copies.load(Ordering::SeqCst), 0);
        w.close();
    }

    #[test]
    fn a_notice_publishes_the_parsed_text() {
        let src = Fake::new(4, A);
        let w = YamlWorker::spawn(src).unwrap();
        w.notify(4);
        wait_for("the first document", || w.latest().is_some());
        let d = w.latest().unwrap();
        assert_eq!(d.update, 4);
        assert!(d.utf8);
        assert_eq!(driver_idx(&d), Some(2));
        assert_eq!(w.generation(), 1);
    }

    #[test]
    fn the_same_counter_is_not_read_again() {
        let src = Fake::new(4, A);
        let w = YamlWorker::spawn(src.clone()).unwrap();
        w.notify(4);
        wait_for("the document", || w.generation() == 1);
        let copies = src.copies.load(Ordering::SeqCst);
        for _ in 0..100 {
            w.notify(4);
        }
        thread::sleep(Duration::from_millis(50));
        assert_eq!(src.copies.load(Ordering::SeqCst), copies);
        assert_eq!(w.generation(), 1);
    }

    #[test]
    fn a_moved_counter_publishes_a_new_document_and_hashes_tell_what_changed() {
        let src = Fake::new(1, A);
        let w = YamlWorker::spawn(src.clone()).unwrap();
        w.notify(1);
        wait_for("the first", || w.generation() == 1);
        let first = w.latest().unwrap();
        src.set(2, B);
        w.notify(2);
        wait_for("the second", || w.generation() == 2);
        let second = w.latest().unwrap();
        assert_eq!(second.update, 2);
        assert_eq!(driver_idx(&second), Some(3));
        assert_eq!(first.sections.weekend, second.sections.weekend);
        assert_ne!(first.sections.driver, second.sections.driver);
        assert!(!first.same_sections(&second));
        // The earlier document stays valid for whoever holds it.
        assert_eq!(driver_idx(&first), Some(2));
    }

    #[test]
    fn a_torn_copy_is_retried() {
        let src = Fake::new(1, A);
        src.tears.store(5, Ordering::SeqCst);
        let w = YamlWorker::spawn(src.clone()).unwrap();
        w.notify(1);
        wait_for("the document", || w.latest().is_some());
        assert!(src.copies.load(Ordering::SeqCst) >= 6);
        assert_eq!(driver_idx(&w.latest().unwrap()), Some(2));
    }

    #[test]
    fn a_parse_that_finishes_after_the_counter_moved_is_discarded() {
        let src = Fake::new(1, A);
        // The counter is read once before the copy and once after the parse; it moves on the
        // second read, so the first parse is stale.
        src.bump_after.store(2, Ordering::SeqCst);
        let w = YamlWorker::spawn(src.clone()).unwrap();
        w.notify(1);
        wait_for("the redone parse", || w.latest().is_some());
        let d = w.latest().unwrap();
        assert_eq!(d.update, 2, "the stale document was published");
        assert_eq!(driver_idx(&d), Some(3));
        assert_eq!(w.generation(), 1);
        assert!(src.copies.load(Ordering::SeqCst) >= 2);
    }

    #[test]
    fn a_text_that_never_settles_does_not_spin_or_publish() {
        let src = Fake::new(1, A);
        src.tears.store(i32::MAX, Ordering::SeqCst);
        let w = YamlWorker::spawn(src.clone()).unwrap();
        w.notify(1);
        thread::sleep(Duration::from_millis(100));
        assert!(w.latest().is_none());
        assert_eq!(src.copies.load(Ordering::SeqCst), COPY_TRIES as i32);
        // The next notice tries again.
        src.tears.store(0, Ordering::SeqCst);
        w.notify(2);
        wait_for("the document", || w.latest().is_some());
    }

    #[test]
    fn close_stops_the_thread() {
        let src = Fake::new(1, A);
        let w = YamlWorker::spawn(src.clone()).unwrap();
        w.notify(1);
        wait_for("the document", || w.latest().is_some());
        w.close();
        // The worker held the only other reference to the source.
        assert_eq!(Arc::strong_count(&src), 1);
    }

    #[test]
    fn an_ibt_is_a_source() {
        let ibt = crate::testing::synthetic_ibt(3, 10);
        let w = YamlWorker::spawn(Arc::new(ibt)).unwrap();
        w.notify(0);
        wait_for("the document", || w.latest().is_some());
        assert!(w.latest().unwrap().info.weekend.is_some());
    }

    #[test]
    fn a_session_doc_from_an_ibt_has_its_counter() {
        let ibt = crate::testing::synthetic_ibt(3, 10);
        let d = ibt.session();
        assert_eq!(d.update, ibt.header().session_info_update);
        assert!(d.errors.is_empty(), "{:?} {:?}", d.errors, d.text);
    }

    #[test]
    fn view_copy_checks_bounds() {
        use crate::view::SliceView;
        let mut img = vec![0u8; 64];
        img[12..16].copy_from_slice(&5i32.to_le_bytes());
        img[16..20].copy_from_slice(&8i32.to_le_bytes());
        img[20..24].copy_from_slice(&32i32.to_le_bytes());
        img[32..40].copy_from_slice(b"---\nabcd");
        let mut dst = Vec::new();
        assert!(copy_from_view(&SliceView::new(&img), &mut dst));
        assert_eq!(dst, b"---\nabcd");
        img[16..20].copy_from_slice(&40i32.to_le_bytes());
        assert!(!copy_from_view(&SliceView::new(&img), &mut dst));
        img[16..20].copy_from_slice(&(-1i32).to_le_bytes());
        assert!(!copy_from_view(&SliceView::new(&img), &mut dst));
    }

    #[cfg(windows)]
    #[test]
    fn the_live_map_is_a_source_and_a_new_text_is_picked_up() {
        use crate::live::Live;
        use crate::testing::LayoutBuilder;
        use crate::testing::map::TestMap;
        let id = format!("{}_worker", std::process::id());
        let (m, e) = (
            format!("Local\\StintmateTestMap_{id}"),
            format!("Local\\StintmateTestEvent_{id}"),
        );
        let l = LayoutBuilder::new().tick_rate(60).build();
        let mut map = TestMap::create(&m, &e, &l, A.as_bytes(), 256 * 1024).unwrap();
        let live = Live::open_named(&m, &e).unwrap();
        let src: Arc<dyn YamlSource> = Arc::new(live.view());
        let first = src.update();
        let w = YamlWorker::spawn(src.clone()).unwrap();
        w.notify(first);
        wait_for("the first text", || w.latest().is_some());
        assert_eq!(driver_idx(&w.latest().unwrap()), Some(2));
        map.set_yaml(B.as_bytes());
        w.notify(src.update());
        wait_for("the second text", || w.generation() == 2);
        let d = w.latest().unwrap();
        assert_eq!(driver_idx(&d), Some(3));
        assert_eq!(d.update, src.update());
    }
}
