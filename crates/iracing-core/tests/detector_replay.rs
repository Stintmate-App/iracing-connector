//! The flag detector over scenario rows: each case builds a drive, fills a tick per row and
//! collects the calls with the session time they fired at.

use iracing_core::announce::detector::Detector;
use iracing_core::announce::{Call, Calls, ParamValue};
use iracing_core::codes::SessionKind;
use iracing_core::meta::{DriverEntry, SessionMeta};
use iracing_core::testing::{LapProfile, Scenario, YamlBuilder, table_of};
use iracing_core::tick::Tick;
use iracing_core::vars::Vars;
use irsdk::defs::session_flags as flag;
use irsdk::row::Row;

/// A lap long enough that a few seconds are a few hundred metres.
const LAP_S: f64 = 60.0;

fn meta(kind: SessionKind) -> SessionMeta {
    let mut m = SessionMeta::empty(0);
    m.kind = kind;
    m.player_idx = 1;
    m.pace_car_idx = 0;
    m.track.length_m = Some(4010.0);
    m.sector_starts = vec![0.0, 0.3, 0.7];
    for (i, name) in [(2, "Ana Rossi"), (3, "Bo Lind"), (4, "Cy Ward")] {
        m.roster[i] = Some(DriverEntry {
            name: name.to_owned(),
            class_est_lap_s: Some(100.0),
            is_real: true,
            ..DriverEntry::default()
        });
    }
    m.roster[1] = Some(DriverEntry {
        name: "Player One".to_owned(),
        class_est_lap_s: Some(100.0),
        is_real: true,
        ..DriverEntry::default()
    });
    m
}

/// A scenario with two quiet laps already driven: the player is on the track and the quiet
/// time is over.
fn race() -> Scenario {
    let mut s = Scenario::new(YamlBuilder::new().roster(4, 1));
    s.garage(1.0).to_stall().drive_out();
    s.lap(LapProfile::new(LAP_S));
    s
}

/// As [`race`], with a stopped rival on the track since before the flag: its speed is known.
fn race_with_rival() -> Scenario {
    let mut s = Scenario::new(YamlBuilder::new().roster(4, 1));
    s.field(1);
    s.garage(1.0).to_stall().drive_out();
    s.lap(LapProfile::new(LAP_S));
    s
}

/// Runs the detector over every row; returns (time, call) pairs.
fn run(s: &Scenario, meta: &SessionMeta) -> Vec<(f64, Call)> {
    let table = table_of(s.layout());
    let vars = Vars::resolve(&table).expect("vars");
    let mut det = Detector::new(2.0);
    let mut tick = Tick::new();
    let mut calls = Calls::new();
    let mut fired = Vec::new();
    for (i, bytes) in s.rows().iter().enumerate() {
        let mut row = Row::from_slice(bytes);
        row.set_tick(i as i32 + 1);
        tick.fill(&vars, &row);
        calls.clear();
        det.step(&tick, Some(meta), &mut calls);
        for c in calls.as_slice() {
            fired.push((tick.session_time, c.clone()));
        }
    }
    fired
}

fn codes(fired: &[(f64, Call)]) -> Vec<&'static str> {
    fired.iter().map(|(_, c)| c.code).collect()
}

fn param<'a>(call: &'a Call, name: &str) -> &'a ParamValue {
    &call
        .set_params()
        .find(|(n, _)| *n == name)
        .unwrap_or_else(|| panic!("no param {name}"))
        .1
}

#[test]
fn detector_yellow_with_a_slow_car_names_its_sector() {
    let mut s = race_with_rival();
    s.flags(flag::YELLOW, 3.0);
    let fired = run(&s, &meta(SessionKind::Race));
    assert_eq!(codes(&fired), ["flag_yellow"]);
    assert_eq!(param(&fired[0].1, "sector"), &ParamValue::Int(3));
}

#[test]
fn detector_yellow_without_a_slow_car_is_ahead() {
    let mut s = race();
    s.flags(flag::YELLOW, 3.0);
    let fired = run(&s, &meta(SessionKind::Race));
    assert_eq!(codes(&fired), ["flag_yellow_ahead"]);
}

#[test]
fn detector_waving_yellow_is_the_second_level() {
    let mut s = race_with_rival();
    s.flags(flag::YELLOW | flag::YELLOW_WAVING, 3.0);
    let fired = run(&s, &meta(SessionKind::Race));
    assert_eq!(codes(&fired), ["flag_double_yellow"]);
    assert_eq!(param(&fired[0].1, "sector"), &ParamValue::Int(3));

    let mut s = race();
    s.flags(flag::YELLOW_WAVING, 3.0);
    assert_eq!(
        codes(&run(&s, &meta(SessionKind::Race))),
        ["flag_double_yellow_ahead"]
    );
}

#[test]
fn detector_yellow_that_turns_to_waving_is_called_again() {
    let mut s = race();
    s.flags(flag::YELLOW, 2.0);
    s.flags(flag::YELLOW | flag::YELLOW_WAVING, 2.0);
    let fired = run(&s, &meta(SessionKind::Race));
    assert_eq!(
        codes(&fired),
        ["flag_yellow_ahead", "flag_double_yellow_ahead"]
    );
}

#[test]
fn detector_debris_says_the_cause() {
    let mut s = race();
    s.flags(flag::DEBRIS, 2.0);
    let fired = run(&s, &meta(SessionKind::Race));
    assert_eq!(codes(&fired), ["flag_yellow_cause"]);
    assert_eq!(
        param(&fired[0].1, "cause"),
        &ParamValue::Str("debris".into())
    );
}

#[test]
fn detector_clear_follows_a_called_yellow_after_three_seconds() {
    let mut s = race();
    s.flags(flag::YELLOW, 2.0);
    s.flags(0, 6.0);
    let fired = run(&s, &meta(SessionKind::Race));
    assert_eq!(codes(&fired), ["flag_yellow_ahead", "flag_clear"]);
    let gap = fired[1].0 - fired[0].0;
    assert!((3.0..6.0).contains(&gap), "clear after {gap}");
}

#[test]
fn detector_no_clear_without_a_called_yellow() {
    let mut s = race();
    s.flags(0, 8.0);
    assert!(run(&s, &meta(SessionKind::Race)).is_empty());
}

#[test]
fn detector_own_yellow_is_a_note_and_never_cleared() {
    let mut s = race();
    s.off_track(0.02, 1.0);
    s.flags(0, 3.0);
    s.flags(flag::YELLOW, 3.0);
    s.flags(0, 6.0);
    let fired = run(&s, &meta(SessionKind::Race));
    assert_eq!(codes(&fired), ["flag_yellow_self"]);
    assert_eq!(
        param(&fired[0].1, "situation"),
        &ParamValue::Str("off_track".into())
    );
}

#[test]
fn detector_blue_names_the_lapping_car() {
    let mut s = race();
    s.blue(2);
    let fired = run(&s, &meta(SessionKind::Race));
    assert_eq!(codes(&fired), ["flag_blue"]);
    assert_eq!(
        param(&fired[0].1, "driver"),
        &ParamValue::Str("Ana Rossi".into())
    );
}

#[test]
fn detector_blue_without_a_known_car_is_unknown() {
    let mut s = race();
    s.blue(9);
    assert_eq!(
        codes(&run(&s, &meta(SessionKind::Race))),
        ["flag_blue_unknown"]
    );
}

#[test]
fn detector_blue_train_names_the_next_car_after_the_gap() {
    let mut s = race();
    s.blue(2);
    s.blue(3);
    s.flags(flag::BLUE, 2.0);
    let fired = run(&s, &meta(SessionKind::Race));
    assert_eq!(codes(&fired), ["flag_blue", "flag_blue_next"]);
    assert_eq!(
        param(&fired[1].1, "driver"),
        &ParamValue::Str("Bo Lind".into())
    );
    assert!(fired[1].0 - fired[0].0 >= 4.0);
}

#[test]
fn detector_blue_only_in_a_race() {
    let mut s = race();
    s.blue(2);
    assert!(run(&s, &meta(SessionKind::Practice)).is_empty());
}

#[test]
fn detector_red_and_meatball() {
    let mut s = race();
    s.flags(flag::RED, 2.0);
    s.flags(0, 1.0);
    s.flags(flag::REPAIR, 2.0);
    let fired = run(&s, &meta(SessionKind::Practice));
    assert_eq!(codes(&fired), ["flag_red", "flag_black_orange"]);
}

#[test]
fn detector_red_respects_its_cooldown() {
    let mut s = race();
    s.flags(flag::RED, 1.0);
    s.flags(0, 1.0);
    s.flags(flag::RED, 1.0);
    assert_eq!(codes(&run(&s, &meta(SessionKind::Race))), ["flag_red"]);
}

#[test]
fn detector_caution_start_pits_open_and_restart() {
    let mut s = race();
    s.caution(3.0);
    s.flags(0, 2.0);
    let fired = run(&s, &meta(SessionKind::Race));
    // The yellow bits that come with the caution are not a local yellow.
    assert_eq!(codes(&fired), ["fcy_start", "fcy_pits_open", "fcy_restart"]);
}

#[test]
fn detector_last_lap_to_green_counts_only_under_a_caution_in_a_race() {
    let bits = flag::YELLOW | flag::CAUTION | flag::CAUTION_WAVING | flag::ONE_LAP_TO_GREEN;
    let mut s = race();
    s.flags(bits, 3.0);
    s.flags(0, 2.0);
    let fired = run(&s, &meta(SessionKind::Race));
    assert_eq!(codes(&fired), ["fcy_start", "fcy_last_lap", "fcy_restart"]);

    let mut s = race();
    s.flags(flag::ONE_LAP_TO_GREEN, 3.0);
    assert!(run(&s, &meta(SessionKind::Race)).is_empty());

    let mut s = race();
    s.flags(bits, 3.0);
    assert!(
        codes(&run(&s, &meta(SessionKind::Practice)))
            .iter()
            .all(|c| !c.starts_with("fcy"))
    );
}

#[test]
fn detector_is_quiet_right_after_the_session_opens() {
    let mut s = Scenario::new(YamlBuilder::new());
    s.garage(1.0);
    s.flags(flag::RED, 2.0);
    assert!(run(&s, &meta(SessionKind::Race)).is_empty());
}

#[test]
fn detector_says_nothing_without_a_session_or_in_a_replay() {
    let mut s = race();
    s.flags(flag::RED, 2.0);
    let table = table_of(s.layout());
    let vars = Vars::resolve(&table).expect("vars");
    let mut det = Detector::new(2.0);
    let mut tick = Tick::new();
    let mut calls = Calls::new();
    for (i, bytes) in s.rows().iter().enumerate() {
        let mut row = Row::from_slice(bytes);
        row.set_tick(i as i32 + 1);
        tick.fill(&vars, &row);
        det.step(&tick, None, &mut calls);
        tick.is_replay = true;
        det.step(&tick, Some(&meta(SessionKind::Race)), &mut calls);
    }
    assert!(calls.is_empty());
}

#[test]
fn detector_reset_starts_a_quiet_time() {
    let mut s = race();
    s.flags(flag::RED, 2.0);
    let table = table_of(s.layout());
    let vars = Vars::resolve(&table).expect("vars");
    let m = meta(SessionKind::Race);
    let mut det = Detector::new(2.0);
    let mut tick = Tick::new();
    let mut calls = Calls::new();
    let mut total = 0;
    for (i, bytes) in s.rows().iter().enumerate() {
        let mut row = Row::from_slice(bytes);
        row.set_tick(i as i32 + 1);
        tick.fill(&vars, &row);
        if tick.session_flags & flag::RED != 0 {
            det.reset();
        }
        calls.clear();
        det.step(&tick, Some(&m), &mut calls);
        total += calls.len();
    }
    assert_eq!(total, 0);
}
