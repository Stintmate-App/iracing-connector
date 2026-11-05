//! What the recorder needs from the session text: the types of [`SessionMeta`].
//!
//! The session text is read on its own thread ([`MetaWorker`]); the recorder takes the finished
//! value as an `Arc<SessionMeta>`. [`build`] makes one from a parsed session text.

use std::collections::BTreeSet;
use std::io;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle, Thread};
use std::time::Duration;

use irsdk::session::{CarSetup, Driver, DriverInfo};
use irsdk::worker::{SessionDoc, YamlWorker};
use irsdk::yaml::{self, fnv1a64, units};

use crate::catalog;
use crate::codes::{self, SessionKind};
use crate::setup::CarSetupSnapshot;
use crate::vars::CARS;

/// The identifiers and names of the track and its layout.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TrackIds {
    /// The track id: a slug of the display name (of the track name when there is none).
    pub track_id: String,
    /// The layout id: the simulator's track id as a decimal number.
    pub layout_id: String,
    /// The track's display name.
    pub track_name: String,
    /// The layout's name (`Default` when the session text gives none).
    pub layout_name: String,
    /// The lap length in metres.
    pub length_m: Option<f64>,
}

/// One car of the grid.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DriverEntry {
    /// The driver's name, on one line.
    pub name: String,
    /// The simulator's user id.
    pub user_id: i64,
    /// The car id: the car's path as written, trimmed.
    pub car_id: String,
    /// The car's display name.
    pub car_name: String,
    /// The class id: the class number as a decimal.
    pub class_id: String,
    /// The class's short name, else the car's display name.
    pub class_name: String,
    /// The class's estimated lap time in seconds.
    pub class_est_lap_s: Option<f64>,
    /// Whether this is a real competitor: not the pace car, not a spectator, with a name.
    pub is_real: bool,
}

/// One line of the current session's results.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ResultEntry {
    /// The overall position, 0 when unknown.
    pub position: i32,
    /// The position in the car's class, 0 when unknown.
    pub class_position: i32,
    /// The simulator's reason-out number (0 when the car is still running).
    pub reason_out_id: i32,
    /// The simulator's reason-out text.
    pub reason_out: String,
}

/// Everything the recorder reads from the session text for one session of the event.
#[derive(Clone, Debug, PartialEq)]
pub struct SessionMeta {
    /// The session text's update counter this was built from.
    pub update: i32,
    /// Counts the metas built, so the recorder can tell a new one with one atomic load.
    pub generation: u64,
    /// The session number this meta describes.
    pub session_num: i32,
    /// `SubSessionID`: 0 offline.
    pub sub_session_id: i64,
    /// `SessionID`: 0 offline.
    pub session_id: i64,
    /// The kind of the session.
    pub kind: SessionKind,
    /// The session type as the session text writes it.
    pub kind_label: String,
    /// The track and layout.
    pub track: TrackIds,
    /// The player's entry, when the roster has it.
    pub player: Option<DriverEntry>,
    /// The player's car slot (`DriverCarIdx`), -1 when unknown.
    pub player_idx: i32,
    /// The pace car's slot, -1 when there is none.
    pub pace_car_idx: i32,
    /// The player's tank capacity in litres.
    pub fuel_capacity_l: Option<f64>,
    /// The incident limit; `None` when unlimited or not written.
    pub incident_limit: Option<u32>,
    /// Whether the event has more than one car class.
    pub multiclass: bool,
    /// The start of each sector as a fraction of the lap, in order.
    pub sector_starts: Vec<f64>,
    /// The roster by car slot; an entry that is not a real competitor is never shown or sent.
    pub roster: [Option<DriverEntry>; CARS],
    /// The current session's results by car slot.
    pub results: [ResultEntry; CARS],
    /// The player's setup, when the session text has one.
    pub setup: Option<Arc<CarSetupSnapshot>>,
    /// The `UpdateCount` of `setup`.
    pub setup_update: Option<i64>,
    /// A hash of what identifies the session: car, track, layout and session type.
    pub identity_hash: u64,
    /// A hash of the set of cars and classes of the roster and the track.
    pub catalog_hash: u64,
    /// A hash of the names by car slot.
    pub roster_hash: u64,
    /// The sections of the session text that did not parse: `(section, message)`.
    pub parse_errors: Vec<(String, String)>,
}

impl SessionMeta {
    /// A meta with nothing read yet, for session `session_num`.
    pub fn empty(session_num: i32) -> SessionMeta {
        SessionMeta {
            update: 0,
            generation: 0,
            session_num,
            sub_session_id: 0,
            session_id: 0,
            kind: SessionKind::Other,
            kind_label: String::new(),
            track: TrackIds::default(),
            player: None,
            player_idx: -1,
            pace_car_idx: -1,
            fuel_capacity_l: None,
            incident_limit: None,
            multiclass: false,
            sector_starts: Vec::new(),
            roster: std::array::from_fn(|_| None),
            results: std::array::from_fn(|_| ResultEntry::default()),
            setup: None,
            setup_update: None,
            identity_hash: 0,
            catalog_hash: 0,
            roster_hash: 0,
            parse_errors: Vec::new(),
        }
    }
}

/// How often the worker looks for a new session text or session number.
const POLL: Duration = Duration::from_millis(50);

fn clean_name(name: &str) -> String {
    let one_line: String = name
        .chars()
        .map(|c| if c == '\r' || c == '\n' { ' ' } else { c })
        .collect();
    one_line.trim().to_owned()
}

fn entry_of(d: &Driver) -> DriverEntry {
    let car_name = d.car_screen_name.clone().unwrap_or_default();
    let class_short = d.car_class_short_name.clone().unwrap_or_default();
    let name = clean_name(d.user_name.as_deref().unwrap_or(""));
    DriverEntry {
        is_real: d.is_real() && !name.is_empty(),
        name,
        user_id: d.user_id.unwrap_or(0),
        car_id: catalog::car_id(d.car_path.as_deref().unwrap_or("")),
        class_id: d.car_class_id.map(catalog::class_id).unwrap_or_default(),
        class_name: if class_short.is_empty() {
            car_name.clone()
        } else {
            class_short
        },
        car_name,
        class_est_lap_s: d.car_class_est_lap_time.filter(|t| *t > 0.0),
    }
}

/// Whether the line starts a key (`Key: value`, `- Key: value` or `Key:`).
fn is_key_line(line: &str) -> bool {
    let body = line.trim_start_matches(' ');
    let body = body
        .strip_prefix("- ")
        .map_or(body, |r| r.trim_start_matches(' '));
    let key_len = body
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .unwrap_or(body.len());
    key_len > 0
        && body[key_len..]
            .strip_prefix(':')
            .is_some_and(|rest| rest.is_empty() || rest.starts_with(' '))
}

/// The text with every continuation line in the `DriverInfo` section joined to the line before it
/// by a space: a name with a line break turns the section into invalid text. `None` when no line
/// changes.
fn join_broken_names(text: &str) -> Option<String> {
    let mut out = String::with_capacity(text.len());
    let mut in_driver = false;
    let mut changed = false;
    for line in text.lines() {
        let header = line.trim_end().strip_suffix(':').filter(|n| {
            !n.is_empty() && !n.contains([' ', ':']) && !line.starts_with([' ', '\t', '-'])
        });
        if let Some(name) = header {
            in_driver = name == "DriverInfo";
            out.push_str(line);
            out.push('\n');
            continue;
        }
        if in_driver
            && !line.trim().is_empty()
            && !line.starts_with("...")
            && !line.starts_with("---")
            && !is_key_line(line)
        {
            if out.ends_with('\n') {
                out.pop();
            }
            out.push(' ');
            out.push_str(line.trim());
            out.push('\n');
            changed = true;
        } else {
            out.push_str(line);
            out.push('\n');
        }
    }
    changed.then_some(out)
}

fn hash_of(parts: impl IntoIterator<Item = String>) -> u64 {
    let mut text = String::new();
    for p in parts {
        text.push_str(&p);
        text.push('\u{1f}');
    }
    fnv1a64(&text)
}

fn int32(v: Option<i64>) -> i32 {
    v.and_then(|x| i32::try_from(x).ok()).unwrap_or(0)
}

/// Builds the meta of session `session_num` from a parsed session text. Pure: the typed parse
/// first, the line scan for every field the typed parse lost.
///
/// The `generation` is left at 0 (the worker numbers the metas it publishes). A driver name with
/// a line break breaks the whole `DriverInfo` section; the section is then read again with the
/// break replaced by a space, and only when that fails too does the player come from the scan.
pub fn build(doc: &SessionDoc, session_num: i32) -> SessionMeta {
    let fb = &doc.fallback;
    let mut meta = SessionMeta::empty(session_num);
    meta.update = doc.update;
    meta.parse_errors = doc.errors.clone();

    let repaired: Option<DriverInfo> =
        if doc.info.driver.is_none() && doc.errors.iter().any(|(s, _)| s == "DriverInfo") {
            join_broken_names(&doc.text).and_then(|t| yaml::parse(&t).info.driver)
        } else {
            None
        };
    if repaired.is_some() {
        meta.parse_errors.retain(|(s, _)| s != "DriverInfo");
    }
    let driver = doc.info.driver.as_ref().or(repaired.as_ref());
    let weekend = doc.info.weekend.as_ref();

    // Track and layout.
    let display = weekend
        .and_then(|w| w.track_display_name.clone())
        .or_else(|| fb.track_display_name.clone())
        .unwrap_or_default();
    let name = weekend
        .and_then(|w| w.track_name.clone())
        .or_else(|| fb.track_name.clone())
        .unwrap_or_default();
    let config = weekend
        .and_then(|w| w.track_config_name.clone())
        .or_else(|| fb.track_config_name.clone())
        .unwrap_or_default();
    let track_num = weekend.and_then(|w| w.track_id).or(fb.track_id);
    meta.track = TrackIds {
        track_id: catalog::track_id(&display, &name),
        layout_id: track_num.map_or_else(String::new, catalog::layout_id),
        track_name: if display.trim().is_empty() {
            name
        } else {
            display
        },
        layout_name: if config.trim().is_empty() {
            "Default".to_owned()
        } else {
            config.trim().to_owned()
        },
        length_m: weekend
            .and_then(|w| w.track_length_m())
            .or_else(|| fb.track_length.as_deref().and_then(units::length_m)),
    };
    if !meta.track.track_id.is_empty() && meta.track.layout_id.is_empty() {
        meta.track.layout_id = "0".to_owned();
    }
    meta.sub_session_id = weekend
        .and_then(|w| w.sub_session_id)
        .or(fb.sub_session_id)
        .unwrap_or(0);
    meta.session_id = weekend
        .and_then(|w| w.session_id)
        .or(fb.session_id)
        .unwrap_or(0);
    meta.incident_limit = weekend.and_then(|w| w.incident_limit());

    // The session.
    let num = i64::from(session_num);
    let session = doc.info.session.as_ref().and_then(|s| {
        s.sessions
            .as_deref()?
            .iter()
            .find(|x| x.session_num == Some(num))
    });
    meta.kind_label = session
        .and_then(|s| s.session_type.clone())
        .or_else(|| {
            fb.session_types
                .iter()
                .find(|(n, _)| *n == num)
                .map(|(_, t)| t.clone())
        })
        .unwrap_or_default();
    meta.kind = if meta.kind_label.is_empty() {
        SessionKind::Other
    } else {
        codes::session_type(&meta.kind_label)
    };
    for r in session
        .and_then(|s| s.results_positions.as_deref())
        .unwrap_or_default()
    {
        let Some(slot) = r
            .car_idx
            .and_then(|i| usize::try_from(i).ok())
            .filter(|i| *i < CARS)
        else {
            continue;
        };
        meta.results[slot] = ResultEntry {
            position: int32(r.position),
            class_position: int32(r.class_position),
            reason_out_id: int32(r.reason_out_id),
            reason_out: r.reason_out_str.clone().unwrap_or_default(),
        };
    }

    // Sectors.
    meta.sector_starts = doc
        .info
        .split
        .as_ref()
        .and_then(|s| s.sectors.as_deref())
        .map(|v| v.iter().filter_map(|s| s.sector_start_pct).collect())
        .filter(|v: &Vec<f64>| !v.is_empty())
        .unwrap_or_else(|| fb.sector_start_pcts.clone());

    // The roster and the player.
    meta.player_idx = driver
        .and_then(|d| d.driver_car_idx)
        .or(fb.driver_car_idx)
        .and_then(|i| i32::try_from(i).ok())
        .unwrap_or(-1);
    meta.pace_car_idx = driver
        .and_then(|d| d.pace_car_idx)
        .and_then(|i| i32::try_from(i).ok())
        .unwrap_or(-1);
    for d in driver
        .and_then(|d| d.drivers.as_deref())
        .unwrap_or_default()
    {
        let Some(slot) = d
            .car_idx
            .and_then(|i| usize::try_from(i).ok())
            .filter(|i| *i < CARS)
        else {
            continue;
        };
        let e = entry_of(d);
        if e.is_real || !e.name.is_empty() {
            meta.roster[slot] = Some(e);
        }
    }
    meta.player = usize::try_from(meta.player_idx)
        .ok()
        .and_then(|i| meta.roster.get(i))
        .and_then(Clone::clone)
        .or_else(|| {
            // Only the scan found the player: no name, so not a competitor to show.
            let path = fb.player_car_path.as_deref()?;
            let screen = fb.player_car_screen_name.clone().unwrap_or_default();
            let class = fb.player_car_class_short_name.clone().unwrap_or_default();
            Some(DriverEntry {
                car_id: catalog::car_id(path),
                class_id: fb
                    .player_car_class_id
                    .map(catalog::class_id)
                    .unwrap_or_default(),
                class_name: if class.is_empty() {
                    screen.clone()
                } else {
                    class
                },
                car_name: screen,
                ..DriverEntry::default()
            })
        });
    let tank = |max: Option<f64>, pct: Option<f64>| max.map(|m| m * pct.unwrap_or(1.0));
    meta.fuel_capacity_l = driver
        .and_then(|d| tank(d.driver_car_fuel_max_ltr, d.driver_car_max_fuel_pct))
        .or_else(|| tank(fb.fuel_max_ltr, fb.max_fuel_pct))
        .filter(|f| f.is_finite() && *f > 0.0);
    let classes: BTreeSet<&str> = meta
        .roster
        .iter()
        .flatten()
        .filter(|d| d.is_real && !d.class_id.is_empty())
        .map(|d| d.class_id.as_str())
        .collect();
    meta.multiclass = weekend.and_then(|w| w.num_car_classes).unwrap_or(0) > 1 || classes.len() > 1;

    // The setup.
    meta.setup_update = doc
        .info
        .car_setup
        .as_ref()
        .and_then(CarSetup::update_count)
        .or(fb.car_setup_update_count);
    meta.setup = doc.info.car_setup.as_ref().map(|c| {
        let mut snap =
            CarSetupSnapshot::from_value(&c.0, driver.and_then(|d| d.driver_car_fuel_kg_per_ltr));
        snap.fixed_setup = weekend
            .and_then(|w| w.weekend_options.as_ref())
            .and_then(|o| o.is_fixed_setup)
            .map(|v| v != 0);
        Arc::new(snap)
    });

    // Hashes.
    let player = meta.player.as_ref();
    let car = player.map_or("", |p| p.car_id.as_str());
    meta.identity_hash = hash_of([
        car.to_owned(),
        meta.track.track_id.clone(),
        meta.track.layout_id.clone(),
        meta.kind.as_str().to_owned(),
    ]);
    let mut pairs: BTreeSet<(String, String)> = meta
        .roster
        .iter()
        .flatten()
        .filter(|d| d.is_real)
        .map(|d| (d.car_id.clone(), d.class_id.clone()))
        .collect();
    if let Some(p) = player {
        pairs.insert((p.car_id.clone(), p.class_id.clone()));
    }
    meta.catalog_hash = hash_of(
        pairs
            .into_iter()
            .map(|(c, k)| format!("{c}\u{1e}{k}"))
            .chain([meta.track.track_id.clone(), meta.track.layout_id.clone()]),
    );
    meta.roster_hash = hash_of(meta.roster.iter().map(|e| match e {
        Some(d) => format!("{}\u{1e}{}", d.name, d.user_id),
        None => String::new(),
    }));
    meta
}

struct Shared {
    stop: AtomicBool,
    generation: AtomicU64,
    latest: Mutex<Option<Arc<SessionMeta>>>,
}

/// The thread `iracing-meta`: builds a [`SessionMeta`] whenever the session text or the session
/// number changes, off the sampling thread.
///
/// The thread wakes every 50 ms. The sampling loop compares [`generation`](Self::generation) with
/// the one it holds (one atomic load per row) and takes [`latest`](Self::latest) only on a change.
pub struct MetaWorker {
    shared: Arc<Shared>,
    waker: Thread,
    handle: Option<JoinHandle<()>>,
}

impl MetaWorker {
    /// Starts the thread. `yaml` supplies the parsed session texts and `session_num` is the
    /// `SessionNum` the sampling loop reads from the telemetry.
    pub fn spawn(yaml: Arc<YamlWorker>, session_num: Arc<AtomicI32>) -> io::Result<MetaWorker> {
        let shared = Arc::new(Shared {
            stop: AtomicBool::new(false),
            generation: AtomicU64::new(0),
            latest: Mutex::new(None),
        });
        let theirs = Arc::clone(&shared);
        let handle = thread::Builder::new()
            .name("iracing-meta".into())
            .spawn(move || run(&yaml, &session_num, &theirs))?;
        let waker = handle.thread().clone();
        Ok(MetaWorker {
            shared,
            waker,
            handle: Some(handle),
        })
    }

    /// How many metas were published. Hot: one atomic load.
    pub fn generation(&self) -> u64 {
        self.shared.generation.load(Ordering::Acquire)
    }

    /// The newest meta, if any. Call it only when [`generation`](Self::generation) changed.
    pub fn latest(&self) -> Option<Arc<SessionMeta>> {
        self.shared
            .latest
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Stops the thread and waits for it.
    pub fn close(self) {}
}

impl Drop for MetaWorker {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Release);
        self.waker.unpark();
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

fn run(yaml: &YamlWorker, session_num: &AtomicI32, shared: &Shared) {
    // The text generation and session number the last meta was built from.
    let mut built: Option<(u64, i32)> = None;
    while !shared.stop.load(Ordering::Acquire) {
        let generation = yaml.generation();
        let num = session_num.load(Ordering::Acquire);
        if generation > 0
            && built != Some((generation, num))
            && let Some(doc) = yaml.latest()
        {
            built = Some((generation, num));
            let mut meta = build(&doc, num);
            let next = shared.generation.load(Ordering::Relaxed) + 1;
            meta.generation = next;
            *shared.latest.lock().unwrap_or_else(|e| e.into_inner()) = Some(Arc::new(meta));
            shared.generation.store(next, Ordering::Release);
        }
        thread::park_timeout(POLL);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_meta_has_a_slot_for_every_car_and_no_player() {
        let meta = SessionMeta::empty(2);
        assert_eq!(meta.session_num, 2);
        assert_eq!(meta.roster.len(), CARS);
        assert_eq!(meta.results.len(), CARS);
        assert!(meta.roster.iter().all(Option::is_none));
        assert_eq!((meta.player_idx, meta.pace_car_idx), (-1, -1));
        assert_eq!(meta, meta.clone());
    }

    use crate::testing::{ResultSpec, TyreReadings, YamlBuilder};
    use irsdk::worker::YamlSource;
    use std::sync::atomic::AtomicI32;
    use std::time::Instant;

    fn doc_of(b: &YamlBuilder) -> SessionDoc {
        SessionDoc::from_raw(7, b.build().as_bytes())
    }

    #[test]
    fn build_reads_the_track_the_player_and_the_event() {
        let b = YamlBuilder::new().roster(3, 1);
        let meta = build(&doc_of(&b), 0);
        assert_eq!((meta.update, meta.session_num, meta.generation), (7, 0, 0));
        assert_eq!(meta.track.track_id, "circuit-de-spa-francorchamps");
        assert_eq!(meta.track.layout_id, "261");
        assert_eq!(meta.track.track_name, "Circuit de Spa-Francorchamps");
        assert_eq!(meta.track.layout_name, "Grand Prix Pits");
        assert_eq!(meta.track.length_m, Some(4010.0));
        assert_eq!(
            (meta.sub_session_id, meta.session_id),
            (2_000_002, 1_000_001)
        );
        assert_eq!(meta.kind, SessionKind::Practice);
        assert_eq!(meta.kind_label, "Practice");
        assert_eq!(meta.incident_limit, Some(17));
        assert_eq!(meta.sector_starts.len(), 3);
        assert_eq!((meta.player_idx, meta.pace_car_idx), (b.player(), 0));
        assert_eq!(meta.fuel_capacity_l, Some(100.0));
        assert!(!meta.multiclass);
        let player = meta.player.as_ref().expect("player");
        assert_eq!(player.name, "Test Driver");
        assert!(player.is_real);
        assert_eq!(player.car_id, "porsche992rgt3");
        assert_eq!(player.class_id, "74");
        assert_eq!(player.class_name, "GT3");
        assert_eq!(player.class_est_lap_s, Some(98.0));
        let real = meta.roster.iter().flatten().filter(|d| d.is_real).count();
        assert_eq!(real, 3);
        // The pace car is listed but is not a competitor.
        assert!(!meta.roster[0].as_ref().expect("pace car").is_real);
        assert!(meta.parse_errors.is_empty());
        let setup = meta.setup.as_ref().expect("setup");
        assert_eq!((setup.update_count, meta.setup_update), (Some(1), Some(1)));
        assert_eq!(setup.fixed_setup, Some(false));
    }

    #[test]
    fn the_session_is_the_one_the_number_names_and_results_follow_it() {
        let b = YamlBuilder::new()
            .roster(3, 1)
            .session(1, "Qualify")
            .session(2, "Race")
            .results(
                2,
                vec![
                    ResultSpec::running(1, 2),
                    ResultSpec {
                        reason_out_id: 3,
                        reason_out: "Disconnected".to_owned(),
                        ..ResultSpec::running(2, 3)
                    },
                ],
            );
        let doc = doc_of(&b);
        let q = build(&doc, 1);
        assert_eq!(q.kind, SessionKind::Qualifying);
        assert_eq!(q.results[1], ResultEntry::default());
        let race = build(&doc, 2);
        assert_eq!(race.kind, SessionKind::Race);
        assert_eq!(race.results[1].position, 2);
        assert_eq!(race.results[2].reason_out_id, 3);
        assert_eq!(race.results[2].reason_out, "Disconnected");
        // A session the text does not list is of no known kind.
        let none = build(&doc, 9);
        assert_eq!(
            (none.kind, none.kind_label.as_str()),
            (SessionKind::Other, "")
        );
    }

    #[test]
    fn two_classes_make_the_field_multiclass() {
        let meta = build(&doc_of(&YamlBuilder::new().roster(8, 3)), 0);
        assert!(meta.multiclass);
        assert_eq!(
            meta.roster.iter().flatten().filter(|d| d.is_real).count(),
            8
        );
    }

    #[test]
    fn a_missing_config_is_default_and_missing_names_fall_back() {
        let b = YamlBuilder::new().track(5, "road atlanta", "", "");
        let meta = build(&doc_of(&b), 0);
        assert_eq!(meta.track.track_id, "road-atlanta");
        assert_eq!(meta.track.track_name, "road atlanta");
        assert_eq!(meta.track.layout_name, "Default");
        assert_eq!(meta.track.layout_id, "5");
    }

    #[test]
    fn a_driver_name_with_a_line_break_does_not_lose_the_roster() {
        let b = YamlBuilder::new()
            .roster(4, 1)
            .driver_name(1, "Test\nDriver");
        let doc = doc_of(&b);
        assert!(
            doc.errors.iter().any(|(s, _)| s == "DriverInfo"),
            "the typed parse should fail: {:?}",
            doc.errors
        );
        let meta = build(&doc, 0);
        let player = meta.player.as_ref().expect("player");
        assert_eq!(player.name, "Test Driver");
        assert!(player.is_real);
        assert_eq!(meta.player_idx, 1);
        assert_eq!(
            meta.roster.iter().flatten().filter(|d| d.is_real).count(),
            4
        );
        assert_eq!(meta.fuel_capacity_l, Some(100.0));
        assert!(meta.parse_errors.is_empty(), "{:?}", meta.parse_errors);
    }

    #[test]
    fn a_line_break_in_another_drivers_name_keeps_the_rest() {
        let b = YamlBuilder::new()
            .roster(4, 2)
            .driver_name(2, "Max\r\nPower");
        let meta = build(&doc_of(&b), 0);
        let other = meta.roster[2].as_ref().expect("slot 2");
        assert_eq!(other.name, "Max Power");
        assert!(meta.roster.iter().flatten().filter(|d| d.is_real).count() >= 4);
        assert!(meta.multiclass);
    }

    #[test]
    fn when_the_driver_section_stays_broken_the_player_comes_from_the_scan() {
        let mut text = YamlBuilder::new().roster(3, 1).build();
        text = text.replace("DriverInfo:\n", "DriverInfo:\n DriverCarIdx: 1\n");
        let doc = SessionDoc::from_raw(3, text.as_bytes());
        assert!(doc.info.driver.is_none());
        let meta = build(&doc, 0);
        assert_eq!(meta.player_idx, 1);
        let player = meta.player.as_ref().expect("player from the scan");
        assert_eq!(player.car_id, "porsche992rgt3");
        assert_eq!(player.class_id, "74");
        assert!(!player.is_real, "no name was found");
        assert!(meta.roster.iter().all(Option::is_none));
        assert_eq!(meta.fuel_capacity_l, Some(100.0));
        assert!(meta.parse_errors.iter().any(|(s, _)| s == "DriverInfo"));
        // The catalog still names the player's car.
        let c = crate::catalog::session_catalog(&meta);
        assert_eq!(c.cars.len(), 1);
    }

    #[test]
    fn the_hashes_move_only_with_what_they_cover() {
        let one = build(&doc_of(&YamlBuilder::new().roster(3, 1)), 0);
        let same = build(&doc_of(&YamlBuilder::new().roster(3, 1)), 0);
        assert_eq!(one.identity_hash, same.identity_hash);
        assert_eq!(one.catalog_hash, same.catalog_hash);
        assert_eq!(one.roster_hash, same.roster_hash);
        // A new class changes the catalog; a renamed driver only the roster.
        let more = build(&doc_of(&YamlBuilder::new().roster(3, 2)), 0);
        assert_ne!(one.catalog_hash, more.catalog_hash);
        let renamed = build(
            &doc_of(
                &YamlBuilder::new()
                    .roster(3, 1)
                    .driver_name(2, "Someone Else"),
            ),
            0,
        );
        assert_ne!(one.roster_hash, renamed.roster_hash);
        assert_eq!(one.catalog_hash, renamed.catalog_hash);
        assert_eq!(one.identity_hash, renamed.identity_hash);
        // The session type is part of the identity.
        let race = build(
            &doc_of(&YamlBuilder::new().roster(3, 1).session(0, "Race")),
            0,
        );
        assert_ne!(one.identity_hash, race.identity_hash);
    }

    #[test]
    fn the_setup_snapshot_carries_the_readings_and_the_fixed_flag() {
        let b = YamlBuilder::new().setup(Some(9), Some(TyreReadings::default()));
        let meta = build(&doc_of(&b), 0);
        let snap = meta.setup.as_ref().expect("setup");
        assert_eq!(snap.update_count, Some(9));
        assert!(!snap.readings.is_empty());
        let none = build(&doc_of(&YamlBuilder::new().setup(None, None)), 0);
        assert!(none.setup.is_none());
        assert_eq!(none.setup_update, None);
    }

    /// A source whose text and counter the test sets.
    struct Fake {
        update: AtomicI32,
        text: Mutex<Vec<u8>>,
    }

    impl YamlSource for Fake {
        fn update(&self) -> i32 {
            self.update.load(Ordering::SeqCst)
        }
        fn copy_yaml(&self, dst: &mut Vec<u8>) -> bool {
            dst.clear();
            dst.extend_from_slice(&self.text.lock().unwrap());
            true
        }
    }

    fn wait_for(what: &str, mut ok: impl FnMut() -> bool) {
        let start = Instant::now();
        while !ok() {
            assert!(
                start.elapsed() < Duration::from_secs(5),
                "timed out: {what}"
            );
            thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn the_worker_publishes_a_meta_per_text_and_per_session_number() {
        let src = Arc::new(Fake {
            update: AtomicI32::new(1),
            text: Mutex::new(YamlBuilder::new().session(1, "Race").build().into_bytes()),
        });
        let yaml = Arc::new(YamlWorker::spawn(src.clone()).unwrap());
        let num = Arc::new(AtomicI32::new(0));
        let worker = MetaWorker::spawn(Arc::clone(&yaml), Arc::clone(&num)).unwrap();
        assert_eq!((worker.generation(), worker.latest()), (0, None));

        yaml.notify(1);
        wait_for("first meta", || worker.generation() >= 1);
        let first = worker.latest().expect("meta");
        assert_eq!(
            (first.generation, first.session_num, first.update),
            (1, 0, 1)
        );
        assert_eq!(first.kind, SessionKind::Practice);

        // A new session number rebuilds from the same text.
        num.store(1, Ordering::SeqCst);
        wait_for("session number", || worker.generation() >= 2);
        let second = worker.latest().expect("meta");
        assert_eq!((second.generation, second.session_num), (2, 1));
        assert_eq!(second.kind, SessionKind::Race);

        // A new text rebuilds too.
        *src.text.lock().unwrap() = YamlBuilder::new()
            .session(1, "Race")
            .roster(5, 1)
            .build()
            .into_bytes();
        src.update.store(2, Ordering::SeqCst);
        yaml.notify(2);
        wait_for("new text", || worker.generation() >= 3);
        let third = worker.latest().expect("meta");
        assert_eq!(third.update, 2);
        assert_eq!(
            third.roster.iter().flatten().filter(|d| d.is_real).count(),
            5
        );

        // Nothing changed: no more metas.
        thread::sleep(Duration::from_millis(200));
        assert_eq!(worker.generation(), 3);
        worker.close();
    }
}
