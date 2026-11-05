//! The flag detector over scenario rows: each case builds a drive, fills a tick per row and
//! collects the calls with the session time they fired at.

use iracing_core::announce::detector::Detector;
use iracing_core::announce::spotter::Spotter;
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

// ---- penalties, the finish, rivals, incidents ---------------------------------------------

/// Runs the detector with a hook that may change each row, or call `on_meta`, before the step.
fn run_hook(
    s: &Scenario,
    meta: &SessionMeta,
    mut hook: impl FnMut(&mut Tick, &mut Detector, &mut Calls),
) -> Vec<(f64, Call)> {
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
        hook(&mut tick, &mut det, &mut calls);
        det.step(&tick, Some(meta), &mut calls);
        for c in calls.as_slice() {
            fired.push((tick.session_time, c.clone()));
        }
        // Calls made by `on_meta` in the hook are kept too.
    }
    fired
}

fn text(call: &Call, name: &str) -> String {
    match param(call, name) {
        ParamValue::Str(s) => s.to_string(),
        other => panic!("{name} is {other:?}"),
    }
}

/// `run_hook` for hooks that call `on_meta`: its calls are collected with the step's.
fn run_meta(
    s: &Scenario,
    meta: &SessionMeta,
    mut hook: impl FnMut(&mut Tick, &mut Detector, &mut Calls),
) -> Vec<(f64, Call)> {
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
        hook(&mut tick, &mut det, &mut calls);
        for c in calls.as_slice() {
            fired.push((tick.session_time, c.clone()));
        }
    }
    fired
}

#[test]
fn detector_black_flag_then_served_in_the_pits() {
    let mut s = race();
    s.flags(flag::BLACK, 6.0);
    s.flags(0, 2.0);
    let m = meta(SessionKind::Race);
    let fired = run(&s, &m);
    assert_eq!(codes(&fired), ["penalty_black_flag"], "never in the pits");

    let mut first_black = None;
    let fired = run_hook(&s, &m, |t, _, _| {
        if t.session_flags & flag::BLACK != 0 {
            let at = *first_black.get_or_insert(t.session_time);
            t.on_pit_road = t.session_time - at > 2.0;
        }
    });
    assert_eq!(codes(&fired), ["penalty_black_flag", "penalty_served"]);
}

#[test]
fn detector_disqualification_is_told_once_and_is_not_a_penalty() {
    let mut s = race();
    s.flags(flag::DISQUALIFY | flag::BLACK, 2.0);
    s.flags(0, 1.0);
    s.flags(flag::DISQUALIFY | flag::BLACK, 2.0);
    let fired = run(&s, &meta(SessionKind::Race));
    assert_eq!(codes(&fired), ["disqualified"]);
}

#[test]
fn detector_final_lap_by_the_white_flag_once() {
    let mut s = race();
    s.flags(flag::WHITE, 2.0);
    s.flags(0, 1.0);
    s.flags(flag::WHITE, 2.0);
    assert_eq!(codes(&run(&s, &meta(SessionKind::Race))), ["final_lap"]);
    let mut s = race();
    s.flags(flag::WHITE, 2.0);
    assert!(run(&s, &meta(SessionKind::Practice)).is_empty());
}

#[test]
fn detector_final_lap_for_the_leader_when_the_flag_is_missing() {
    let mut s = race();
    s.lap(LapProfile::new(LAP_S));
    let m = meta(SessionKind::Race);
    let fired = run_hook(&s, &m, |t, _, _| {
        t.laps_remain = Some(1);
        t.position = Some(1);
    });
    assert_eq!(codes(&fired), ["final_lap"]);
    let fired = run_hook(&s, &m, |t, _, _| {
        t.laps_remain = Some(1);
        t.position = Some(4);
    });
    assert!(fired.is_empty(), "not the leader");
}

#[test]
fn detector_chequered_with_the_position_or_the_class_position() {
    let mut s = race();
    s.chequered(3);
    let fired = run(&s, &meta(SessionKind::Race));
    assert_eq!(codes(&fired), ["chequered"]);
    assert_eq!(param(&fired[0].1, "position"), &ParamValue::Int(3));

    let mut m = meta(SessionKind::Race);
    m.multiclass = true;
    let fired = run_hook(&s, &m, |t, _, _| t.class_position = Some(2));
    assert_eq!(param(&fired[0].1, "position"), &ParamValue::Int(2));

    let mut s = race();
    s.chequered(0);
    assert_eq!(
        codes(&run(&s, &meta(SessionKind::Race))),
        ["chequered_no_position"]
    );
}

#[test]
fn detector_retired_comes_from_the_session_text_once() {
    let mut s = race();
    s.flags(0, 6.0);
    let base = meta(SessionKind::Race);
    let mut out = base.clone();
    out.results[1].reason_out_id = 3;
    let mut n = 0;
    let fired = run_meta(&s, &base, |t, det, calls| {
        n += 1;
        if n == 1 {
            det.on_meta(&base, calls);
        }
        if t.session_time > 40.0 {
            det.on_meta(&out, calls);
        }
    });
    assert_eq!(codes(&fired), ["retired"]);

    // A car that is already out when the text first arrives is the baseline, not news.
    let fired = run_meta(&s, &base, |_, det, calls| det.on_meta(&out, calls));
    assert!(fired.is_empty());
}

#[test]
fn detector_rival_out_by_the_session_text_once_per_identity() {
    let mut s = race();
    s.flags(0, 6.0);
    let base = meta(SessionKind::Race);
    let mut out = base.clone();
    out.results[2].reason_out_id = 2;
    let mut n = 0;
    let fired = run_meta(&s, &base, |t, det, calls| {
        n += 1;
        if n == 1 {
            det.on_meta(&base, calls);
        }
        if t.session_time > 40.0 {
            det.on_meta(&out, calls);
        }
        if t.session_time > 45.0 {
            det.on_meta(&base, calls);
        }
        if t.session_time > 50.0 {
            det.on_meta(&out, calls);
        }
    });
    assert_eq!(codes(&fired), ["rival_out"]);
    assert_eq!(text(&fired[0].1, "driver"), "Ana Rossi");
}

#[test]
fn detector_rival_disqualified_by_flags_and_then_not_out() {
    let mut s = race();
    s.flags(0, 8.0);
    let base = meta(SessionKind::Race);
    let mut out = base.clone();
    out.results[2].reason_out_id = 4;
    let mut n = 0;
    let fired = {
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
            n += 1;
            calls.clear();
            if n == 1 {
                det.on_meta(&base, &mut calls);
            }
            if tick.session_time > 40.0 {
                tick.car_flags.as_mut().expect("flags")[2] |= flag::DISQUALIFY;
            }
            det.step(&tick, Some(&base), &mut calls);
            if tick.session_time > 50.0 {
                det.on_meta(&out, &mut calls);
            }
            for c in calls.as_slice() {
                fired.push((tick.session_time, c.clone()));
            }
        }
        fired
    };
    assert_eq!(codes(&fired), ["rival_disqualified"]);
    assert_eq!(text(&fired[0].1, "driver"), "Ana Rossi");
}

#[test]
fn detector_incident_one_point() {
    let mut s = race();
    s.incident(1, 0);
    s.flags(0, 3.0);
    let fired = run(&s, &meta(SessionKind::Race));
    assert_eq!(codes(&fired), ["incident"]);
    assert_eq!(param(&fired[0].1, "points"), &ParamValue::Int(1));
    assert_eq!(param(&fired[0].1, "total"), &ParamValue::Int(1));
}

#[test]
fn detector_incident_in_practice_too_and_two_and_two_make_four() {
    let mut s = race();
    s.incident(2, 0);
    s.flags(0, 1.0);
    s.incident(2, 0);
    s.flags(0, 3.0);
    let fired = run(&s, &meta(SessionKind::Practice));
    assert_eq!(codes(&fired), ["incident"]);
    assert_eq!(param(&fired[0].1, "points"), &ParamValue::Int(4));
    assert_eq!(param(&fired[0].1, "total"), &ParamValue::Int(4));
}

#[test]
fn detector_incident_limit_warning_once() {
    let mut s = race();
    s.incident(2, 0);
    s.flags(0, 3.0);
    s.incident(2, 0);
    s.flags(0, 3.0);
    s.incident(1, 0);
    s.flags(0, 3.0);
    let mut m = meta(SessionKind::Race);
    m.incident_limit = Some(8);
    let fired = run(&s, &m);
    assert_eq!(
        codes(&fired),
        ["incident", "incident", "incident_limit_warning", "incident"]
    );
    assert_eq!(param(&fired[2].1, "total"), &ParamValue::Int(4));
    assert_eq!(param(&fired[2].1, "limit"), &ParamValue::Int(8));

    m.incident_limit = None;
    assert!(
        codes(&run(&s, &m))
            .iter()
            .all(|c| *c != "incident_limit_warning")
    );
}

#[test]
fn detector_is_quiet_after_a_session_change() {
    let mut s = race();
    s.session_change(1, "Race");
    s.flags(flag::RED, 2.0);
    assert!(run(&s, &meta(SessionKind::Race)).is_empty());
}

#[test]
fn detector_no_flag_calls_on_pit_road() {
    let mut s = race();
    s.flags(flag::YELLOW, 3.0);
    let fired = run_hook(&s, &meta(SessionKind::Race), |t, _, _| t.on_pit_road = true);
    assert!(fired.is_empty());
}

#[test]
fn detector_blue_from_a_faster_class_just_behind() {
    let mut s = race();
    s.flags(flag::BLUE, 3.0);
    let behind = |t: &mut Tick| {
        t.car_loc[2] = 3;
        t.car_lap[2] = if t.lap_dist_pct < 0.02 {
            t.lap - 1
        } else {
            t.lap
        };
        t.car_pct[2] = (t.lap_dist_pct - 0.02).rem_euclid(1.0);
    };
    let mut m = meta(SessionKind::Race);
    m.multiclass = true;
    m.roster[2].as_mut().expect("car 2").class_est_lap_s = Some(90.0);
    let fired = run_hook(&s, &m, |t, _, _| behind(t));
    assert_eq!(codes(&fired), ["flag_blue"]);
    assert_eq!(text(&fired[0].1, "driver"), "Ana Rossi");

    m.multiclass = false;
    let fired = run_hook(&s, &m, |t, _, _| behind(t));
    assert_eq!(codes(&fired), ["flag_blue_unknown"]);
}

// ---- the spotter --------------------------------------------------------------------------

/// Runs the spotter over a drive of `secs` seconds after the quiet laps; `word(rel)` is
/// `CarLeftRight` `rel` seconds into it.
fn spot(
    kind: SessionKind,
    green: Option<f64>,
    secs: f64,
    word: impl Fn(f64) -> i32,
    mut each: impl FnMut(&mut Tick),
) -> Vec<(f64, Call)> {
    let mut s = race();
    let before = s.rows().len();
    s.flags(0, secs);
    let table = table_of(s.layout());
    let vars = Vars::resolve(&table).expect("vars");
    let m = meta(kind);
    let mut spotter = Spotter::new();
    let mut tick = Tick::new();
    let mut calls = Calls::new();
    let mut t0 = None;
    let mut fired = Vec::new();
    for (i, bytes) in s.rows().iter().enumerate().skip(before) {
        let mut row = Row::from_slice(bytes);
        row.set_tick(i as i32 + 1);
        tick.fill(&vars, &row);
        let t0 = *t0.get_or_insert(tick.session_time);
        let rel = tick.session_time - t0;
        tick.car_left_right = Some(word(rel));
        each(&mut tick);
        calls.clear();
        spotter.step(&tick, Some(&m), green, &mut calls);
        for c in calls.as_slice() {
            fired.push((rel, c.clone()));
        }
    }
    fired
}

fn practice(secs: f64, word: impl Fn(f64) -> i32) -> Vec<(f64, Call)> {
    spot(SessionKind::Practice, None, secs, word, |_| {})
}

#[test]
fn spotter_car_left_and_clear_left() {
    let fired = practice(4.0, |r| if r < 1.0 { 2 } else { 1 });
    assert_eq!(codes(&fired), ["spotter_car_left", "spotter_clear_left"]);
    assert!(fired[1].0 - 1.0 >= 0.15 - 1e-9 && fired[1].0 - 1.0 < 0.3);
}

#[test]
fn spotter_car_right_and_clear_right() {
    let fired = practice(4.0, |r| if r < 1.0 { 3 } else { 1 });
    assert_eq!(codes(&fired), ["spotter_car_right", "spotter_clear_right"]);
}

#[test]
fn spotter_middle_then_all_clear() {
    let word = |r: f64| match r {
        r if r < 1.0 => 2,
        r if r < 2.0 => 4,
        _ => 1,
    };
    let fired = practice(4.0, word);
    assert_eq!(
        codes(&fired),
        [
            "spotter_car_left",
            "spotter_in_the_middle",
            "spotter_all_clear"
        ]
    );
}

#[test]
fn spotter_three_wide_that_drops_to_one_side_is_called_again() {
    let word = |r: f64| match r {
        r if r < 1.0 => 4,
        r if r < 2.0 => 2,
        _ => 1,
    };
    let fired = practice(4.0, word);
    assert_eq!(
        codes(&fired),
        [
            "spotter_in_the_middle",
            "spotter_car_left",
            "spotter_clear_left"
        ]
    );
}

#[test]
fn spotter_still_there_rotates_its_wordings() {
    let fired = practice(11.0, |r| if r < 10.0 { 2 } else { 1 });
    assert_eq!(
        codes(&fired),
        [
            "spotter_car_left",
            "spotter_still_left",
            "spotter_still_left",
            "spotter_still_left",
            "spotter_clear_left"
        ]
    );
    let variants: Vec<u8> = fired[1..4].iter().map(|(_, c)| c.variant).collect();
    assert_eq!(variants, [0, 1, 2]);
    assert!((fired[1].0 - 3.0).abs() < 0.05, "{}", fired[1].0);
}

#[test]
fn spotter_two_cars_on_a_side_count_as_one() {
    let fired = practice(4.0, |r| match r {
        r if r < 0.5 => 2,
        r if r < 1.0 => 5,
        r if r < 2.0 => 2,
        _ => 1,
    });
    assert_eq!(codes(&fired), ["spotter_car_left", "spotter_clear_left"]);
}

#[test]
fn spotter_bounce_is_called_only_if_it_still_holds() {
    // Back after 0.3 s and gone again before the bounce window ends: nothing more is said.
    let gone = practice(5.0, |r| match r {
        r if r < 1.0 => 2,
        r if (1.3..1.8).contains(&r) => 2,
        _ => 1,
    });
    assert_eq!(codes(&gone), ["spotter_car_left", "spotter_clear_left"]);
    // Back and held: called again once the window is over.
    let held = practice(5.0, |r| match r {
        r if r < 1.0 => 2,
        r if r >= 1.3 => 2,
        _ => 1,
    });
    assert_eq!(
        codes(&held),
        ["spotter_car_left", "spotter_clear_left", "spotter_car_left"]
    );
    assert!(held[2].0 - held[1].0 >= 1.5 - 1e-9);
}

#[test]
fn spotter_gates_pit_road_caution_speed_and_the_games_own_spotter() {
    let left = |_: f64| 2;
    let pits = spot(SessionKind::Practice, None, 3.0, left, |t| {
        t.on_pit_road = true
    });
    assert!(pits.is_empty(), "pit road");
    let caution = spot(SessionKind::Practice, None, 3.0, left, |t| {
        t.session_flags |= flag::CAUTION
    });
    assert!(caution.is_empty(), "caution");
    let slow = spot(SessionKind::Practice, None, 3.0, left, |t| t.speed = 5.0);
    assert!(slow.is_empty(), "slow");
    let off = practice(3.0, |_| 0);
    assert!(off.is_empty(), "the game's spotter is off");
    let menus = spot(SessionKind::Other, None, 3.0, left, |_| {});
    assert!(menus.is_empty(), "another kind of session");
}

#[test]
fn spotter_race_waits_twenty_seconds_after_the_green() {
    let open = spot(SessionKind::Race, Some(0.0), 3.0, |_| 2, |_| {});
    assert_eq!(codes(&open), ["spotter_car_left"]);
    let early = spot(SessionKind::Race, Some(1.0e9), 3.0, |_| 2, |_| {});
    assert!(early.is_empty(), "inside the delay");
    let none = spot(SessionKind::Race, None, 3.0, |_| 2, |_| {});
    assert!(none.is_empty(), "no green yet");
}
