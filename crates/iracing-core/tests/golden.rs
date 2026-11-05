//! Golden scenarios: a drive goes through the tracker and every call it makes to the sink is
//! compared with `tests/golden/<name>.json`. Run with `UPDATE_GOLDEN=1` to rewrite the files.

use std::path::PathBuf;
use std::sync::Arc;

use iracing_core::ibt_source::IbtSource;
use iracing_core::io::Sink;
use iracing_core::io::{Source, SourceEvent};
use iracing_core::meta::{self, SessionMeta};
use iracing_core::rivals::{CatalogWatch, RivalTracker};
use iracing_core::testing::{
    FixedClock, LapProfile, RecordingSink, Scenario, Side, TyreReadings, YamlBuilder, full_layout,
    table_of,
};
use iracing_core::tick::Tick;
use iracing_core::tracker::{State, Tracker};
use iracing_core::vars::Vars;
use irsdk::defs::incident_flags;
use irsdk::row::Row;
use irsdk::testing::RowBuilder;
use irsdk::worker::SessionDoc;

fn meta_of(text: &[u8], update: i32, session_num: i32) -> Arc<SessionMeta> {
    Arc::new(meta::build(
        &SessionDoc::from_raw(update, text),
        session_num,
    ))
}

/// Runs the rows of a scenario through a tracker, with the session text changes at their rows.
///
/// A scenario with no text changes and no gaps goes through [`IbtSource`] as well, and both
/// runs must give the same calls to the sink.
fn run(s: &Scenario) -> (RecordingSink, Tracker, State) {
    run_with(s, false)
}

fn run_with(s: &Scenario, rivals: bool) -> (RecordingSink, Tracker, State) {
    let direct = run_rows(
        s.layout(),
        s.rows(),
        s.initial_yaml(),
        s.yaml_updates()
            .iter()
            .map(|u| (u.at_row, u.text.as_slice()))
            .collect(),
        s.disconnects(),
        rivals,
    );
    if s.yaml_updates().is_empty() && s.disconnects().is_empty() {
        let via = run_ibt(s, rivals);
        assert_eq!(
            via.to_json(),
            direct.0.to_json(),
            "through IbtSource the calls differ"
        );
    }
    direct
}

fn run_ibt(s: &Scenario, rivals: bool) -> RecordingSink {
    let mut src = IbtSource::new(s.ibt()).expect("source");
    let meta = Arc::clone(src.meta().expect("meta"));
    let mut tracker = Tracker::new(1024);
    let mut sink = RecordingSink::new();
    let clock = FixedClock::default();
    let mut tick = Tick::new();
    let mut glue = rivals.then(Glue::new);
    while src.next(&mut tick) == SourceEvent::Row {
        step_all(&mut tracker, glue.as_mut(), &tick, &meta, &mut sink, &clock);
    }
    if let Some(g) = glue.as_mut() {
        g.finish(&tick, &mut sink);
    }
    tracker.stop(true, &mut sink, &clock);
    sink
}

/// `gaps` are `(row, seconds)`: before that row the simulator is gone for `seconds` of wall
/// time, which shows as rows with `connected` false (the session time stands still).
fn run_rows(
    layout: &irsdk::testing::Layout,
    rows: &[Vec<u8>],
    initial: &[u8],
    updates: Vec<(usize, &[u8])>,
    gaps: &[(usize, f64)],
    rivals: bool,
) -> (RecordingSink, Tracker, State) {
    let table = table_of(layout);
    let vars = Vars::resolve(&table).expect("vars");
    let mut tick = Tick::new();
    let mut tracker = Tracker::new(1024);
    let mut sink = RecordingSink::new();
    let clock = FixedClock::default();
    let mut first = Row::from_slice(&rows[0]);
    first.set_tick(0);
    tick.fill(&vars, &first);
    let mut meta = meta_of(initial, 1, tick.session_num);
    let mut update = 1;
    let mut glue = rivals.then(Glue::new);
    for (i, bytes) in rows.iter().enumerate() {
        for (_, text) in updates.iter().filter(|(at, _)| *at == i) {
            update += 1;
            let mut r = Row::from_slice(bytes);
            r.set_tick(i as i32);
            tick.fill(&vars, &r);
            meta = meta_of(text, update, tick.session_num);
        }
        for &(_, secs) in gaps.iter().filter(|(at, _)| *at == i) {
            tick.connected = false;
            for _ in 0..(secs * 60.0).ceil() as usize {
                clock.advance(1.0 / 60.0);
                step_all(&mut tracker, glue.as_mut(), &tick, &meta, &mut sink, &clock);
            }
        }
        let mut row = Row::from_slice(bytes);
        row.set_tick(i as i32);
        tick.fill(&vars, &row);
        tick.connected = true;
        tick.fresh = true;
        step_all(&mut tracker, glue.as_mut(), &tick, &meta, &mut sink, &clock);
    }
    if let Some(g) = glue.as_mut() {
        g.finish(&tick, &mut sink);
    }
    // Send what is still waiting, without ending the session.
    let state = tracker.state();
    tracker.stop(true, &mut sink, &clock);
    (sink, tracker, state)
}

/// The recorder's part for the field: the catalog rule, then the rivals, then the hand-over.
struct Glue {
    rivals: RivalTracker,
    watch: CatalogWatch,
    local: Option<u64>,
}

impl Glue {
    fn new() -> Glue {
        Glue {
            rivals: RivalTracker::new(),
            watch: CatalogWatch::new(),
            local: None,
        }
    }

    fn finish(&mut self, tick: &Tick, sink: &mut RecordingSink) {
        if let (Some(local), Some(rows)) =
            (self.local, self.rivals.take_due(tick.session_time, true))
        {
            sink.participants(local, rows);
        }
    }
}

/// One row through the tracker and, when there is a glue, the rivals.
fn step_all(
    tracker: &mut Tracker,
    glue: Option<&mut Glue>,
    tick: &Tick,
    meta: &Arc<SessionMeta>,
    sink: &mut RecordingSink,
    clock: &FixedClock,
) {
    let laps_before = sink.laps().len();
    tracker.step(tick, Some(meta), sink, clock);
    let Some(g) = glue else { return };
    let local = tracker.session_local();
    if local != g.local {
        g.finish(tick, sink);
        g.rivals.reset();
        match local {
            Some(_) => g.watch.opened(meta),
            None => g.watch.closed(),
        }
        g.local = local;
    }
    let Some(local) = local else { return };
    g.watch.offer(meta, sink);
    g.rivals.step(tick, meta);
    let ended = sink.laps().len() > laps_before;
    if let Some(rows) = g.rivals.take_due(tick.session_time, ended) {
        sink.participants(local, rows);
    }
}

fn check(name: &str, sink: &RecordingSink) {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("golden")
        .join(format!("{name}.json"));
    let got = serde_json::to_string_pretty(&sink.to_json()).expect("json") + "\n";
    if std::env::var_os("UPDATE_GOLDEN").is_some() {
        std::fs::create_dir_all(path.parent().expect("dir")).expect("create dir");
        std::fs::write(&path, &got).expect("write golden");
        return;
    }
    let want = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{}: {e} (run with UPDATE_GOLDEN=1)", path.display()))
        .replace("\r\n", "\n");
    assert_eq!(got, want, "{name} differs from {}", path.display());
}

fn types(sink: &RecordingSink) -> Vec<String> {
    sink.calls("lap")
        .iter()
        .map(|l| {
            format!(
                "{}{}",
                l["type"].as_str().expect("type"),
                if l["partial"] == true { "*" } else { "" }
            )
        })
        .collect()
}

// ---- 1. practice_basic -------------------------------------------------------------------

#[test]
fn golden_practice_basic() {
    let mut s = Scenario::new(YamlBuilder::new());
    s.garage(10.0).to_stall().drive_out();
    for time in [30.0, 30.4, 29.8, 30.2] {
        s.lap(LapProfile::new(time));
    }
    s.pit_in();
    s.to_garage();
    let (sink, _, state) = run(&s);
    check("practice_basic", &sink);

    assert_eq!(types(&sink), ["out", "flying", "flying", "flying", "in*"]);
    let laps = sink.calls("lap");
    for (l, time) in laps[..4].iter().zip([30.0, 30.4, 29.8, 30.2]) {
        assert_eq!(l["time_source"], "official");
        assert_eq!(l["time_s"], time);
        assert_eq!(l["sectors"].as_array().expect("sectors").len(), 3);
    }
    assert_eq!(laps[0]["valid"], false, "the out lap touched the pit road");
    assert_eq!(laps[1]["valid"], true);
    // Opened in the garage: the catalog and the session start, then the setup, nothing else.
    assert_eq!(
        &sink.kinds()[..5],
        ["start_session", "setup", "stall_tyres", "pit_exit", "setup"],
        "pit exit comes before its setup"
    );
    assert_eq!(sink.calls("setup")[0]["reason"], "session_start");
    assert_eq!(sink.calls("setup")[1]["reason"], "pit_exit");
    assert_eq!(state, State::Garage);
}

#[test]
fn tracker_replays_an_ibt_through_ibt_source() {
    let mut s = Scenario::new(YamlBuilder::new());
    s.garage(2.0).to_stall().drive_out();
    s.lap(LapProfile::new(20.0));
    s.lap(LapProfile::new(20.3));
    let (want, _, _) = run(&s);

    let sink = run_ibt(&s, false);
    assert_eq!(types(&sink), types(&want));
    assert_eq!(sink.kinds(), want.kinds());
}

// ---- 2. invalid_laps ---------------------------------------------------------------------

#[test]
fn golden_invalid_laps() {
    let off = incident_flags::REPORT_OFF_TRACK;
    let mut s = Scenario::new(YamlBuilder::new());
    s.garage(1.0).to_stall().drive_out();
    s.lap(LapProfile::new(30.1)); // 1 out lap
    s.off_track(0.4, 1.0);
    s.lap(LapProfile::new(30.2)); // 2 off track
    s.cut(0.4, 2.0);
    s.lap(LapProfile::new(30.3)); // 3 cut: no rule, stays valid
    s.incident_at(0.5, 2, off);
    s.lap(LapProfile::new(30.4)); // 4 incident, certain
    s.incident_at(0.5, 4, incident_flags::REPORT_CAR_CONTACT);
    s.lap(LapProfile::new(30.5)); // 5 incident, cause unknown
    s.lap(LapProfile::new(30.6)); // 6 clean
    s.alongside(Side::Left, 10.0);
    s.tow(); // 7 partial
    let (sink, _, _) = run(&s);
    check("invalid_laps", &sink);

    let laps = sink.calls("lap");
    let valid: Vec<bool> = laps.iter().map(|l| l["valid"] == true).collect();
    assert_eq!(valid, [false, false, true, false, false, true, false]);
    assert_eq!(laps[1]["cause"], "off_track");
    assert_eq!(laps[1]["cause_certain"], false);
    assert_eq!(laps[3]["cause"], "off_track");
    assert_eq!(laps[3]["cause_certain"], true);
    assert_eq!(laps[4]["cause"], "unknown");
    assert_eq!(laps[6]["partial"], true);
    assert_eq!(laps[6]["cause"], "unknown");
    assert!(laps[6]["time_s"].is_null());
}

// ---- 3. pit_stop -------------------------------------------------------------------------

#[test]
fn golden_pit_stop() {
    let mut s = Scenario::new(YamlBuilder::new());
    s.garage(1.0).to_stall().drive_out();
    s.lap(LapProfile::new(30.1));
    s.lap(LapProfile::new(30.2).fuel(3.0));
    s.pit_in();
    s.stop(6.0, 25.0, Some(TyreReadings::default()));
    s.drive_out(); // the in lap crosses the line in the pit lane
    s.lap(LapProfile::new(30.3));
    s.drive_through();
    s.lap(LapProfile::new(30.4));
    let (sink, tracker, _) = run(&s);
    check("pit_stop", &sink);

    let kinds = sink.kinds();
    // Each pit exit comes after the lap that was waiting for its time, and before its setup.
    let exits: Vec<usize> = (0..kinds.len())
        .filter(|&i| kinds[i] == "pit_exit")
        .collect();
    assert_eq!(exits.len(), 3, "first exit, after the stop, drive-through");
    for &i in &exits {
        assert_eq!(kinds[i + 1], "setup");
    }
    assert_eq!(
        kinds[exits[1] - 1],
        "lap",
        "the stop's in lap is sent first"
    );
    assert_eq!(sink.calls("stall_tyres").len(), 2, "first stall, the stop");
    assert!(tracker.stall_tyres().is_some());
    let laps = sink.calls("lap");
    assert_eq!(laps[2]["type"], "in");
    assert_eq!(laps[2]["touched_pit"], true);
    assert_eq!(laps[2]["valid"], false);
    // The fuel burnt by the in lap does not count the refuelling.
    assert!(laps[2]["fuel_used_l"].as_f64().expect("fuel") < 10.0);
}

// ---- 4. sessions -------------------------------------------------------------------------

#[test]
fn golden_sessions() {
    let mut s = Scenario::new(YamlBuilder::new());
    s.garage(1.0).to_stall().drive_out();
    s.lap(LapProfile::new(20.1));
    s.to_garage();
    s.session_change(1, "Lone Qualify");
    s.to_stall().drive_out();
    s.lap(LapProfile::new(20.2));
    s.to_garage();
    s.session_change(2, "Race");
    s.session_state(3);
    s.to_stall().drive_out();
    s.lap(LapProfile::new(20.3)); // the parade lap
    s.session_state(4);
    s.lap(LapProfile::new(20.4));
    s.to_garage();
    // A new event with no lap.
    s.new_event(YamlBuilder::new().ids(1_000_009, 2_000_009));
    s.garage(2.0);
    // The simulator is gone for six seconds: the session ends and a new one opens.
    s.disconnect(6.0);
    s.to_stall().drive_out();
    s.lap(LapProfile::new(20.5));
    // Three seconds: the same session goes on.
    s.disconnect(3.0);
    s.lap(LapProfile::new(20.6));
    let (sink, _, _) = run(&s);
    check("sessions", &sink);

    let starts = sink.calls("start_session");
    let kinds: Vec<&str> = starts
        .iter()
        .map(|c| c["kind"].as_str().expect("kind"))
        .collect();
    assert_eq!(
        kinds,
        ["practice", "qualifying", "race", "practice", "practice"]
    );
    // Three changes of session and the six second gap end a session each; the last one is
    // still open.
    assert_eq!(sink.calls("end_session").len(), 4);
    let race_laps: Vec<_> = sink
        .calls("lap")
        .into_iter()
        .filter(|l| l["local"] == 3)
        .collect();
    assert_eq!(race_laps[0]["type"], "formation");
    assert_eq!(race_laps[1]["type"], "flying");
    let kept: Vec<_> = sink
        .calls("lap")
        .into_iter()
        .filter(|l| l["local"] == 5)
        .collect();
    assert_eq!(
        kept.len(),
        2,
        "the lap that spans the three second gap is kept"
    );
    let last = sink.calls("lap").last().copied().expect("lap");
    assert_eq!(last["local"], 5, "the three second gap kept the session");
    assert_eq!(last["type"], "flying");
}

// ---- 5. multiclass_rivals ---------------------------------------------------------------

/// Twenty cars in two classes; every rival crosses the line at the start of each player lap.
fn field_laps(s: &mut Scenario, lap: u32, slots: std::ops::RangeInclusive<usize>) {
    for idx in slots.filter(|i| *i != 1) {
        let class_gap = if idx % 2 == 0 { 0.0 } else { 6.0 };
        s.rival(idx)
            .lap(80.0 + class_gap + idx as f32 * 0.1 + lap as f32 * 0.01);
    }
}

#[test]
fn golden_multiclass_rivals() {
    let mut s = Scenario::new(YamlBuilder::new().roster(20, 2));
    s.field(19);
    s.garage(1.0).to_stall().drive_out();
    s.lap(LapProfile::new(30.0)); // the out lap: every rival's first counter is a baseline
    field_laps(&mut s, 1, 2..=20);
    s.lap(LapProfile::new(30.1));
    // A car in the pits for a lap: not valid, one stop.
    s.rival(3).pit();
    field_laps(&mut s, 2, 2..=20);
    s.lap(LapProfile::new(30.2));
    s.rival(3).leave_pits();
    field_laps(&mut s, 3, 2..=20);
    s.lap(LapProfile::new(30.3));
    let (sink, _, _) = run_with(&s, true);
    check("multiclass_rivals", &sink);

    let rows: Vec<&serde_json::Value> = sink
        .calls("participants")
        .iter()
        .flat_map(|c| c["rows"].as_array().expect("rows").iter())
        .collect();
    let of = |idx: u64| -> Vec<&&serde_json::Value> {
        rows.iter().filter(|r| r["participant"] == idx).collect()
    };
    assert_eq!(of(2).len(), 3, "three counted laps after the baseline");
    assert_eq!(of(2)[0]["time_source"], "official");
    assert_eq!(of(2)[0]["valid"], true);
    assert_eq!(of(2)[0]["position"], 2, "the position from the telemetry");
    let pit = of(3);
    assert_eq!(pit[1]["pit"], true);
    assert_eq!(pit[1]["valid"], false);
    assert_eq!(pit[1]["pit_stops"], 1);
    assert_eq!(pit[0]["pit_stops"], 0);
    // The player is a row too, and the classes differ by car.
    assert!(rows.iter().any(|r| r["is_player"] == true));
    assert_ne!(of(2)[0]["car_id"], of(3)[0]["car_id"]);
    // No catalog was sent: the roster did not change.
    assert!(sink.calls("catalog").is_empty());
}

#[test]
fn golden_late_joiner_and_reused_slot() {
    let mut s = Scenario::new(YamlBuilder::new().roster(20, 2));
    s.field(19);
    s.garage(1.0).to_stall().drive_out();
    s.lap(LapProfile::new(30.0));
    // Slot 21 is in the telemetry but not in the session text yet.
    s.rival(21).lap(85.0);
    s.lap(LapProfile::new(30.1));
    s.rival(21).lap(85.5);
    s.rival(5).lap(81.0);
    s.flags(0, 3.0);
    // The text catches up: the catalog goes first, then the held row with the name.
    s.update_yaml(|y| y.roster(21, 3));
    s.lap(LapProfile::new(30.2));
    // Slot 5 now holds another driver: its counters start over.
    s.update_yaml(|y| y.new_driver(5, "Replacement"));
    s.rival(5).lap(82.0);
    s.lap(LapProfile::new(30.3));
    s.rival(5).lap(82.5);
    s.lap(LapProfile::new(30.4));
    // A slot that never gets a name: sent after the wait with the slot in the name.
    s.rival(40).lap(90.0);
    s.lap(LapProfile::new(30.5));
    s.rival(40).lap(90.5);
    s.lap(LapProfile::new(30.6));
    let (sink, _, _) = run_with(&s, true);
    check("late_joiner", &sink);

    let kinds = sink.kinds();
    let cat = kinds.iter().position(|k| *k == "catalog").expect("catalog");
    assert_eq!(
        sink.calls("catalog").len(),
        1,
        "the joiner brings a new class; a reused slot none"
    );
    let rows: Vec<&serde_json::Value> = sink
        .calls("participants")
        .iter()
        .flat_map(|c| c["rows"].as_array().expect("rows").iter())
        .collect();
    let joiner = rows
        .iter()
        .filter(|r| r["participant"] == 21)
        .collect::<Vec<_>>();
    assert_eq!(joiner.len(), 1);
    assert_ne!(joiner[0]["name"], "Car 21", "named after the text arrived");
    let first_row = kinds
        .iter()
        .enumerate()
        .position(|(i, k)| i > cat && *k == "participants")
        .expect("rows after the catalog");
    assert!(cat < first_row);
    let reused: Vec<_> = rows.iter().filter(|r| r["participant"] == 5).collect();
    assert_eq!(
        reused.len(),
        2,
        "one before, one after the new driver's baseline"
    );
    assert_eq!(reused[1]["name"], "Replacement");
    assert!(
        rows.iter()
            .any(|r| r["participant"] == 40 && r["name"] == "Car 40")
    );
}

// ---- 6. encoding -------------------------------------------------------------------------

#[test]
fn golden_encoding() {
    let y = YamlBuilder::new()
        .roster(3, 1)
        .driver_name(1, "Jos\u{e9} M\u{fc}ller")
        .driver_name(2, "Max\nVerstappen");
    let mut s = Scenario::new(y);
    s.garage(1.0).to_stall().drive_out();
    s.lap(LapProfile::new(20.0));
    let (sink, _, _) = run(&s);
    check("encoding", &sink);

    let start = sink.calls("start_session")[0];
    assert_eq!(start["player_name"], "Jos\u{e9} M\u{fc}ller");
    assert!(
        start["roster"]
            .as_array()
            .expect("roster")
            .iter()
            .any(|n| n == "Max Verstappen"),
        "the line break became a space: {}",
        start["roster"]
    );
}

// ---- 7. units ----------------------------------------------------------------------------

#[test]
fn golden_units() {
    let y = YamlBuilder::new().imperial().fuel(26.4, 1.0);
    let mut s = Scenario::new(y);
    s.garage(1.0).to_stall().drive_out();
    s.lap(LapProfile::new(20.0));
    s.lap(LapProfile::new(20.0).fuel(1.0));
    let (sink, _, _) = run(&s);
    check("units", &sink);

    let layout = &sink.calls("start_session")[0]["catalog"]["layouts"][0];
    let length = layout["length_m"].as_f64().expect("length");
    assert!(
        (length - 4010.0).abs() < 20.0,
        "miles became metres: {length}"
    );
}

// ---- a recorded session's sequence ----------------------------------------------------------------

/// Synthetic rows that follow the order of events of a recorded session: out of the car before
/// the first drive, an out lap through the pit lane, a reset to the pits, leaving the car and
/// coming back, and a stop with four tyres.
struct Rows {
    layout: irsdk::testing::Layout,
    rows: Vec<Vec<u8>>,
    t: f64,
    length: f64,
    lap: i32,
    completed: i32,
    pct: f64,
    last: f32,
    pending_last: Option<(u32, f32)>,
    next_official: Option<f32>,
    loc: i32,
    pit_road: bool,
    stall: bool,
    on_track: bool,
    on_track_car: bool,
    garage: bool,
    garage_visible: bool,
    replay: bool,
    fuel: f32,
    speed: f32,
    service: bool,
    sv: i32,
    temp: f32,
    wear: f32,
}

const DT: f64 = 1.0 / 60.0;

impl Rows {
    fn new() -> Rows {
        Rows {
            layout: full_layout(),
            rows: Vec::new(),
            t: 0.0,
            length: 4010.0,
            lap: 0,
            completed: 0,
            pct: 0.0313,
            last: 0.0,
            pending_last: None,
            next_official: None,
            loc: -1,
            pit_road: false,
            stall: false,
            on_track: false,
            on_track_car: false,
            garage: false,
            garage_visible: false,
            replay: true,
            fuel: 0.0,
            speed: 0.0,
            service: false,
            sv: 0,
            temp: 42.0,
            wear: 1.0,
        }
    }

    fn row(&mut self) {
        self.t += DT;
        if let Some((n, v)) = self.pending_last {
            if n == 0 {
                self.last = v;
                self.pending_last = None;
            } else {
                self.pending_last = Some((n - 1, v));
            }
        }
        let mut b = RowBuilder::new(&self.layout);
        b.set_f64("SessionTime", self.t)
            .set_i32("SessionUniqueID", 1)
            .set_i32("SessionState", 4)
            .set_bool("IsOnTrack", self.on_track)
            .set_bool("IsOnTrackCar", self.on_track_car)
            .set_bool("IsInGarage", self.garage)
            .set_bool("IsGarageVisible", self.garage_visible)
            .set_bool("IsReplayPlaying", self.replay)
            .set_i32("ReplayFrameNumEnd", 1)
            .set_i32("PlayerCarIdx", 1)
            .set_i32("Lap", self.lap)
            .set_i32("LapCompleted", self.completed)
            .set_f32("LapDist", (self.pct * self.length) as f32)
            .set_f32("LapDistPct", self.pct as f32)
            .set_f32("LapLastLapTime", self.last)
            .set_f32("Speed", self.speed)
            .set_f32("FuelLevel", self.fuel)
            .set_bool("OnPitRoad", self.pit_road)
            .set_bool("PlayerCarInPitStall", self.stall)
            .set_bool("PitstopActive", self.service)
            .set_i32("PlayerCarPitSvStatus", self.sv)
            .set_i32("PlayerTrackSurface", self.loc)
            .set_i32("PlayerTrackSurfaceMaterial", 1)
            .set_bool("PitsOpen", true)
            .set_f32("AirTemp", 20.0)
            .set_f32("TrackTempCrew", 28.0);
        for wheel in ["LF", "RF", "LR", "RR"] {
            for pos in ["L", "M", "R"] {
                b.set_f32(&format!("{wheel}tempC{pos}"), self.temp)
                    .set_f32(&format!("{wheel}wear{pos}"), self.wear);
            }
        }
        self.rows.push(b.bytes().to_vec());
    }

    fn hold(&mut self, secs: f64) {
        for _ in 0..(secs / DT).ceil() as u32 {
            self.row();
        }
    }

    /// Drives `secs` seconds at `speed` m/s; the counters rise at the line and the simulator's
    /// time follows half a second later.
    fn go(&mut self, secs: f64, speed: f32) {
        self.speed = speed;
        for _ in 0..(secs / DT).ceil() as u32 {
            self.pct += f64::from(speed) * DT / self.length;
            if self.pct >= 1.0 {
                self.pct -= 1.0;
                self.lap += 1;
                self.completed += 1;
                if let Some(v) = self.next_official.take() {
                    self.pending_last = Some((30, v));
                }
            }
            self.row();
        }
    }

    fn drive_to(&mut self, pct: f64, speed: f32) {
        let ahead = (pct - self.pct).rem_euclid(1.0);
        self.go(ahead * self.length / f64::from(speed), speed);
    }

    /// Everything the simulator does in the row the car appears in its stall.
    fn appear_in_stall(&mut self, temp: f32, wear: f32) {
        self.pit_road = true;
        self.stall = true;
        self.loc = 1;
        self.speed = 0.0;
        self.pct = 0.0313;
        self.temp = temp;
        self.wear = wear;
    }

    /// The car stops in its stall: the readings update in the row it arrives, and a service
    /// starts in that row.
    fn arrive_stall(&mut self, temp: f32, wear: f32, service: bool) {
        self.stall = true;
        self.loc = 1;
        self.speed = 0.0;
        self.temp = temp;
        self.wear = wear;
        if service {
            self.service = true;
            self.sv = 1;
        }
        self.row();
    }

    fn leave_stall(&mut self) {
        self.stall = false;
        self.loc = 2;
    }

    fn leave_pits(&mut self) {
        self.pit_road = false;
        self.loc = 3;
    }

    /// The driver gets out: the road car stays for a moment, then the garage screen opens and
    /// closes, and the counters read 0.
    fn leave_car(&mut self) {
        self.on_track = false;
        self.replay = true;
        self.speed = 0.0;
        self.hold(0.3);
        self.on_track_car = false;
        self.garage = true;
        self.garage_visible = true;
        self.loc = -1;
        self.pit_road = false;
        self.stall = false;
        self.lap = 0;
        self.completed = 0;
        self.fuel = 0.0;
        self.hold(0.3);
        self.garage = false;
        self.garage_visible = false;
        self.hold(0.5);
    }

    fn enter_stall(&mut self, fuel: f32, temp: f32, wear: f32) {
        self.on_track = true;
        self.on_track_car = true;
        self.replay = false;
        self.fuel = fuel;
        self.lap = 1;
        self.appear_in_stall(temp, wear);
        self.service = true;
        self.sv = 1;
        self.hold(0.5);
        self.service = false;
        self.sv = 0;
    }
}

#[test]
fn golden_recorded_session() {
    let mut r = Rows::new();
    // Out of the car before the first drive: a session is loaded, so this is the garage.
    r.hold(1.0);
    // First entry: in the stall, the counters read 0 and -1.
    r.on_track = true;
    r.on_track_car = true;
    r.replay = false;
    r.fuel = 22.8;
    r.lap = 0;
    r.completed = -1;
    r.appear_in_stall(54.9, 0.997);
    r.service = true;
    r.sv = 1;
    r.hold(1.0);
    r.service = false;
    r.sv = 0;
    // The out lap: the stall is past the line; the lap ends in the pit lane, where the
    // counters rise, with no time. Then the stall again, without a service.
    r.leave_stall();
    r.go(1.0, 25.0);
    r.leave_pits();
    r.go(20.0, 60.0);
    r.drive_to(0.985, 60.0);
    r.pit_road = true;
    r.loc = 2;
    r.drive_to(0.0313, 25.0);
    r.arrive_stall(56.0, 0.99, false);
    r.hold(1.0);
    r.leave_stall();
    r.go(1.0, 25.0);
    r.leave_pits();
    // A reset to the pits: both flags rise in one row and the fraction jumps back.
    r.go(10.0, 60.0);
    r.fuel = 21.0;
    r.appear_in_stall(58.8, 0.982);
    r.hold(0.5);
    r.leave_stall();
    r.go(1.0, 25.0);
    r.leave_pits();
    // The lap after a reset is not timed (-1).
    r.next_official = Some(-1.0);
    r.drive_to(0.0, 60.0);
    r.go(1.0, 60.0);
    // A timed lap.
    r.next_official = Some(66.5);
    r.drive_to(0.0, 60.0);
    // Leaving the car in the middle of a lap, then re-entering: the counters start over and
    // `Lap` rises in the stall without a crossing.
    r.go(10.0, 60.0);
    r.leave_car();
    r.enter_stall(30.0, 71.8, 0.9);
    r.leave_stall();
    r.go(1.0, 25.0);
    r.leave_pits();
    r.go(20.0, 60.0);
    // The stop: into the pit lane, over the line, into the stall, fuel and four tyres.
    r.drive_to(0.985, 60.0);
    r.pit_road = true;
    r.loc = 2;
    r.next_official = Some(121.8);
    r.drive_to(0.0313, 25.0);
    r.arrive_stall(71.3, 0.977, true);
    for _ in 0..4 {
        r.hold(1.0);
        r.fuel += 6.0;
    }
    r.service = false;
    r.sv = 2;
    r.hold(0.5);
    r.leave_stall();
    r.sv = 0;
    r.go(2.0, 25.0);
    r.leave_pits();
    r.go(5.0, 60.0);
    r.leave_car();

    let y = YamlBuilder::new().build_cp1252();
    let (sink, tracker, state) = run_rows(&r.layout, &r.rows, &y, Vec::new(), &[], false);
    check("recorded_session", &sink);

    // The session opened out of the car, before the first drive.
    assert_eq!(sink.kinds()[0], "start_session");
    assert_eq!(sink.calls("start_session").len(), 1);
    assert_eq!(sink.calls("end_session").len(), 0);
    assert_eq!(state, State::Garage);
    // The counters falling and rising again on leaving and re-entering the car ended no lap:
    // every lap that was sent is accounted for.
    let laps = sink.calls("lap");
    let listed: Vec<String> = types(&sink);
    assert!(
        laps.iter()
            .all(|l| l["sim_lap"].as_i64().expect("lap") >= 0),
        "{listed:?}"
    );
    // The out lap ended in the pit lane with no time.
    assert_eq!(laps[0]["type"], "out");
    assert!(laps[0]["time_s"].is_null());
    assert_eq!(laps[0]["valid"], false);
    // The reset ended its lap as a partial lap, unknown cause.
    let reset = laps
        .iter()
        .find(|l| l["partial"] == true)
        .expect("a reset lap");
    assert_eq!(reset["cause"], "unknown");
    // The lap the simulator did not time is invalid and has no official time.
    let untimed = laps
        .iter()
        .find(|l| l["partial"] == false && l["valid"] == false && l["type"] == "out")
        .expect("an untimed out lap");
    assert_ne!(untimed["time_source"], "official");
    // The timed lap keeps the simulator's time.
    assert!(
        laps.iter()
            .any(|l| l["time_s"] == 66.5 && l["time_source"] == "official")
    );
    // The stall was entered five times: the first entry, the stop-less visit, the reset, the
    // re-entry and the stop.
    assert_eq!(sink.calls("stall_tyres").len(), 5);
    let tyres = tracker.stall_tyres().expect("readings");
    assert_eq!(tyres.temp_c[0][0], 71.3);
    assert_eq!(tyres.wear[3][2], 0.977);
    // The lap the simulator timed at the stop went out before the pit exit.
    let kinds = sink.kinds();
    let last_exit = kinds.iter().rposition(|k| *k == "pit_exit").expect("exit");
    let timed = sink
        .to_json()
        .as_array()
        .expect("calls")
        .iter()
        .position(|e| e["call"] == "lap" && e["time_s"] == 121.8)
        .expect("the timed lap");
    assert!(timed < last_exit);
}

// ---- a disconnect is wall time -----------------------------------------------------------

#[test]
fn golden_disconnect() {
    let mut s = Scenario::new(YamlBuilder::new());
    s.garage(1.0).to_stall().drive_out();
    s.lap(LapProfile::new(20.1));
    // Gone for three seconds: the session goes on and the lap is kept.
    s.disconnect(3.0);
    s.lap(LapProfile::new(20.2));
    // Gone for six: the session ends with the lap open, and a new one opens after.
    s.disconnect(6.0);
    s.lap(LapProfile::new(20.3));
    let (sink, _, _) = run(&s);
    check("disconnect", &sink);

    assert_eq!(sink.calls("start_session").len(), 2);
    assert_eq!(sink.calls("end_session").len(), 1);
    let kinds = sink.kinds();
    let end = kinds.iter().position(|k| *k == "end_session").expect("end");
    assert_eq!(
        kinds[..end].iter().filter(|k| **k == "lap").count(),
        2,
        "the lap over the three second gap is kept; the session ends after the six second one"
    );
}
