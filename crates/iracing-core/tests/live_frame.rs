//! The live frame against the connector protocol's JSON schema, in every phase.

use iracing_core::codes::SessionKind;
use iracing_core::io::{LapType, StallTyres};
use iracing_core::live::{LiveSnapshot, frame_json};
use iracing_core::meta::{DriverEntry, SessionMeta};
use iracing_core::tracker::State;
use jsonschema::Validator;
use serde_json::{Value, json};

fn validator() -> Validator {
    let schema: Value = serde_json::from_str(include_str!("fixtures/live-frame.schema.json"))
        .expect("schema is JSON");
    jsonschema::validator_for(&schema).expect("schema compiles")
}

fn assert_valid(v: &Validator, frame: &Value) {
    let errors: Vec<String> = v.iter_errors(frame).map(|e| format!("{e}")).collect();
    assert!(errors.is_empty(), "{errors:#?}\n{frame:#}");
}

fn entry(name: &str, car: &str) -> DriverEntry {
    DriverEntry {
        name: name.into(),
        car_id: car.into(),
        car_name: "Test Car".into(),
        class_id: "4001".into(),
        class_name: "GT".into(),
        class_est_lap_s: Some(100.0),
        is_real: true,
        ..DriverEntry::default()
    }
}

fn meta() -> SessionMeta {
    let mut m = SessionMeta::empty(0);
    m.kind = SessionKind::Race;
    m.track.track_id = "spa".into();
    m.track.layout_id = "spa-gp".into();
    m.track.track_name = "Spa".into();
    m.track.layout_name = "Grand Prix".into();
    m.track.length_m = Some(7004.0);
    m.fuel_capacity_l = Some(45.0);
    m.player_idx = 3;
    m.pace_car_idx = 0;
    m.roster[0] = Some(entry("Pace Car", "pace"));
    m.roster[3] = Some(entry("Me", "porsche"));
    m.roster[5] = Some(entry("Rival", "porsche"));
    m.roster[7] = Some(entry("Gone", "porsche"));
    m.player = m.roster[3].clone();
    m.results[5].position = 1;
    m
}

fn snapshot(state: State) -> LiveSnapshot {
    let mut s = LiveSnapshot::empty();
    s.view.state = state;
    s.view.session_kind = SessionKind::Race;
    s.view.timed = state == State::Track;
    s.view.lap_clock_s = 31.25;
    s.view.lap_type = Some(LapType::Flying);
    s.view.sector = Some(1);
    s.view.pit_mode = if state == State::Garage { 4 } else { 0 };
    s.mono_s = 100.0;
    let t = &mut s.tick;
    t.speed = 50.0;
    t.rpm = 6500.0;
    t.gear = 4;
    t.fuel_l = 30.5;
    t.lap = 3;
    t.lap_completed = 2;
    t.lap_dist = 1200.0;
    t.lap_last_time = 101.234;
    t.lap_best_time = Some(100.5);
    t.laps_total = Some(10);
    t.time_total = Some(604_800.0);
    t.session_state = 4;
    t.air_c = Some(21.0);
    t.track_c = Some(30.0);
    t.precip = Some(0.25);
    t.wind_ms = Some(3.0);
    t.car_loc[3] = 3;
    t.car_loc[5] = 3;
    t.car_loc[0] = 3;
    t.car_loc[7] = -1;
    t.car_pct[3] = 0.2;
    t.car_pct[5] = 0.25;
    t.car_est = Some([0.0; 64]);
    t.car_position = Some([0; 64]);
    t.car_position.as_mut().expect("set")[3] = 2;
    t.car_position.as_mut().expect("set")[5] = 1;
    t.car_f2 = Some([0.0; 64]);
    t.car_f2.as_mut().expect("set")[3] = 4.5;
    s
}

#[test]
fn menus_frame_has_only_the_head() {
    let v = validator();
    let f = frame_json(
        &snapshot(State::Off),
        None,
        100.1,
        "2026-01-02T18:00:00.123+00:00",
    );
    assert_valid(&v, &f);
    assert_eq!(f["phase"], "menus");
    assert_eq!(f.as_object().expect("object").len(), 4);
    // A session without its data yet is menus too.
    let f = frame_json(
        &snapshot(State::Track),
        None,
        100.1,
        "2026-01-02T18:00:00.123+00:00",
    );
    assert_eq!(f["phase"], "menus");
}

#[test]
fn garage_frame_is_valid_and_has_no_stop_wheels_before_a_stop() {
    let v = validator();
    let mut s = snapshot(State::Garage);
    s.tick.fuel_l = 0.0;
    let f = frame_json(&s, Some(&meta()), 100.2, "2026-01-02T18:00:00.123+00:00");
    assert_valid(&v, &f);
    assert_eq!(f["phase"], "garage");
    assert!(
        f["car"]["fuel_l"].is_null(),
        "fuel 0 out of the car is unknown"
    );
    assert!(f["car"].get("wheels").is_none());
    assert_eq!(f["car"]["pit_mode"], 4);
}

#[test]
fn on_track_frame_is_valid_and_complete() {
    let v = validator();
    let mut s = snapshot(State::Track);
    s.tyres = Some(StallTyres {
        at_s: 10.0,
        occasion: iracing_core::io::StallOccasion::PitStop,
        temp_c: [[80.0, 85.0, 90.0]; 4],
        wear: [[0.9, 0.9, 0.9]; 4],
    });
    s.tick.session_flags = irsdk::defs::session_flags::BLACK;
    let f = frame_json(&s, Some(&meta()), 100.2, "2026-01-02T18:00:00.123+00:00");
    assert_valid(&v, &f);
    assert_eq!(f["phase"], "on_track");
    assert_eq!(f["session"]["type"], "race");
    assert_eq!(f["session"]["laps_in_event"], 10);
    assert!(f["session"]["duration_s"].is_null());
    assert_eq!(f["car"]["speed_kmh"], 180.0);
    assert_eq!(f["car"]["penalty"], "other");
    assert_eq!(f["lap"]["sector"], 2);
    assert_eq!(f["lap"]["time_s"], 31.25);
    assert_eq!(f["lap"]["last_lap_s"], 101.234);
    assert_eq!(f["conditions"]["rain"], 0.25);
    assert_eq!(f["splits"]["ahead_s"], 4.5);
    // Left wheels: the outside is the left reading; right wheels the opposite.
    assert_eq!(f["car"]["wheels"]["fl"]["tread_out_c"], 80.0);
    assert_eq!(f["car"]["wheels"]["fr"]["tread_out_c"], 90.0);
    assert_eq!(f["car"]["wheels"]["rl"]["wear"], 0.1);
    assert!(f["car"]["wheels"]["fl"]["pressure_kpa"].is_null());
    let parts = f["participants"].as_array().expect("array");
    let idx: Vec<i64> = parts
        .iter()
        .map(|p| p["index"].as_i64().expect("i"))
        .collect();
    assert_eq!(idx, [3, 5], "the pace car and an absent slot are left out");
    assert_eq!(parts[0]["is_player"], true);
    assert_eq!(parts[1]["position"], 1);
}

#[test]
fn paused_frame_after_a_second_without_rows() {
    let v = validator();
    let f = frame_json(
        &snapshot(State::Track),
        Some(&meta()),
        102.0,
        "2026-01-02T18:00:00.123+00:00",
    );
    assert_valid(&v, &f);
    assert_eq!(f["phase"], "paused");
    assert_eq!(f["data_age_s"], 2.0);
}

#[test]
fn practice_gap_wraps_by_the_class_estimate() {
    let mut s = snapshot(State::Track);
    s.view.session_kind = SessionKind::Practice;
    let est = s.tick.car_est.as_mut().expect("set");
    est[3] = 98.0;
    est[5] = 2.0;
    let m = meta();
    let f = frame_json(&s, Some(&m), 100.1, "2026-01-02T18:00:00.123+00:00");
    assert_valid(&validator(), &f);
    assert_eq!(json!(f["splits"]["ahead_s"]), json!(4.0));
}

#[test]
fn race_positions_before_the_first_timed_lap_come_from_the_running_order() {
    let v = validator();
    let mut s = snapshot(State::Track);
    let mut m = meta();
    m.results[5].position = 0;
    s.tick.car_position = Some([0; 64]);
    s.tick.car_lap_completed[3] = 0;
    s.tick.car_lap_completed[5] = 0;
    s.tick.car_pct[3] = 0.2;
    s.tick.car_pct[5] = 0.25;
    let f = frame_json(&s, Some(&m), 100.2, "2026-01-02T18:00:00.123+00:00");
    assert_valid(&v, &f);
    let parts = f["participants"].as_array().expect("array");
    assert_eq!(parts[0]["position"], 2, "the player is behind the rival");
    assert_eq!(parts[1]["position"], 1);
    // Outside a race there is no order to count.
    s.view.session_kind = SessionKind::Practice;
    let f = frame_json(&s, Some(&m), 100.2, "2026-01-02T18:00:00.123+00:00");
    assert!(f["participants"][0]["position"].is_null());
}

/// A race where the player stopped: the simulator's positions still hold the order of the last
/// line crossing (the player first), while on the road he is behind two cars and ahead of one.
fn after_a_stop() -> (LiveSnapshot, SessionMeta) {
    let mut s = snapshot(State::Track);
    let mut m = meta();
    m.results[5].position = 0;
    m.roster[6] = Some(entry("Slow", "porsche"));
    m.roster[8] = Some(entry("Leader", "porsche"));
    let t = &mut s.tick;
    for (i, completed, pct, est, pos) in [
        (3, 3, 0.5, 50.0, 1),
        (5, 3, 0.9, 90.0, 2),
        (6, 3, 0.2, 20.0, 3),
        (8, 4, 0.1, 10.0, 4),
    ] {
        t.car_loc[i] = 3;
        t.car_lap_completed[i] = completed;
        t.car_pct[i] = pct;
        t.car_est.as_mut().expect("set")[i] = est;
        t.car_position.as_mut().expect("set")[i] = pos;
    }
    t.car_f2 = Some([0.0; 64]);
    (s, m)
}

#[test]
fn race_position_and_gaps_follow_the_road_after_a_stop() {
    let (mut s, m) = after_a_stop();
    let f = frame_json(&s, Some(&m), 100.2, "2026-01-02T18:00:00.123+00:00");
    assert_valid(&validator(), &f);
    let pos = |f: &Value, i: i64| {
        f["participants"]
            .as_array()
            .expect("array")
            .iter()
            .find(|p| p["index"] == i)
            .expect("car")["position"]
            .clone()
    };
    assert_eq!(pos(&f, 3), 3, "the player is third on the road, not first");
    assert_eq!(
        (pos(&f, 8), pos(&f, 5), pos(&f, 6)),
        (json!(1), json!(2), json!(4))
    );
    assert_eq!(f["splits"]["ahead_s"], 40.0, "the car ahead on the road");
    assert_eq!(f["splits"]["behind_s"], 30.0, "the car behind on the road");

    // The nearest car ahead is a lap up: the gap adds the whole lap at the class estimate.
    s.tick.car_lap_completed[5] = 4;
    s.tick.car_pct[5] = 0.4;
    s.tick.car_est.as_mut().expect("set")[5] = 40.0;
    let f = frame_json(&s, Some(&m), 100.2, "2026-01-02T18:00:00.123+00:00");
    assert_eq!(f["splits"]["ahead_s"], 60.0);

    // Before the green flag the simulator's positions stand.
    let (mut s, m) = after_a_stop();
    s.tick.session_state = 3;
    let f = frame_json(&s, Some(&m), 100.2, "2026-01-02T18:00:00.123+00:00");
    assert_eq!(pos(&f, 3), 1);

    // Outside a race they stand too.
    let (mut s, m) = after_a_stop();
    s.view.session_kind = SessionKind::Practice;
    let f = frame_json(&s, Some(&m), 100.2, "2026-01-02T18:00:00.123+00:00");
    assert_eq!(pos(&f, 3), 1);
}

#[test]
fn repair_needed_follows_the_repair_session_flag_inside_a_session() {
    let v = validator();
    let mut s = snapshot(State::Track);
    s.tick.session_flags = irsdk::defs::session_flags::REPAIR;
    let f = frame_json(&s, Some(&meta()), 100.2, "2026-01-02T18:00:00.123+00:00");
    assert_valid(&v, &f);
    assert_eq!(f["car"]["repair_needed"], true);
    // Cleared when the repair is done.
    s.tick.session_flags = 0;
    let f = frame_json(&s, Some(&meta()), 100.3, "2026-01-02T18:00:00.123+00:00");
    assert_valid(&v, &f);
    assert_eq!(f["car"]["repair_needed"], false);
    // Out of a session there is no car, so no field.
    let f = frame_json(
        &snapshot(State::Idle),
        Some(&meta()),
        100.4,
        "2026-01-02T18:00:00.123+00:00",
    );
    assert!(
        f.get("car")
            .is_none_or(|c| c.get("repair_needed").is_none())
    );
}
