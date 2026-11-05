//! The scenario builder: rows and session texts, and the round trip through an `.ibt` image.

use iracing_core::testing::{LapProfile, ResultSpec, Scenario, Side, TyreReadings, YamlBuilder};
use iracing_core::tick::Tick;
use iracing_core::vars::Vars;
use irsdk::defs::{incident_flags, session_flags};
use irsdk::ibt::Ibt;
use irsdk::row::Row;

const LAP_S: f64 = 8.0;

fn ticks(s: &Scenario) -> Vec<Tick> {
    let table = iracing_core::testing::table_of(s.layout());
    let vars = Vars::resolve(&table).expect("vars");
    s.rows()
        .iter()
        .enumerate()
        .map(|(i, bytes)| {
            let mut row = Row::from_slice(bytes);
            row.set_tick(i as i32 + 1);
            let mut t = Tick::new();
            t.fill(&vars, &row);
            *t
        })
        .collect()
}

fn practice() -> Scenario {
    let mut s = Scenario::new(YamlBuilder::new());
    s.garage(2.0).to_stall().drive_out();
    s.lap(LapProfile::new(LAP_S));
    for _ in 0..3 {
        s.lap(LapProfile::new(LAP_S));
    }
    s.pit_in().stop(2.0, 20.0, Some(TyreReadings::default()));
    s.drive_out().to_garage();
    s
}

#[test]
fn practice_basic_round_trips_through_an_ibt() {
    let s = practice();
    let ibt: Ibt = s.ibt();
    assert_eq!(ibt.len(), s.rows().len());
    assert_eq!(ibt.row_len(), s.layout().buf_len());
    assert_eq!(ibt.disk().session_lap_count, 6);
    assert_eq!(ibt.row(0).expect("row 0"), &s.rows()[0][..]);
    assert_eq!(
        ibt.row(ibt.len() - 1).expect("last"),
        &s.rows()[s.rows().len() - 1][..]
    );

    // The file's own table resolves, and its rows give the same values as the scenario's.
    let vars = Vars::resolve(ibt.vars()).expect("file variables");
    let mut from_file = Tick::new();
    let mut from_rows = Tick::new();
    let table = iracing_core::testing::table_of(s.layout());
    let own = Vars::resolve(&table).expect("vars");
    for i in (0..ibt.len()).step_by(37) {
        from_file.fill(&vars, &Row::from_slice(ibt.row(i).expect("row")));
        from_rows.fill(&own, &Row::from_slice(&s.rows()[i]));
        assert_eq!(from_file, from_rows, "row {i}");
    }

    let doc = ibt.session();
    assert!(doc.errors.is_empty(), "{:?}", doc.errors);
    let weekend = doc.info.weekend.expect("weekend");
    assert!((weekend.track_length_m().expect("length") - 4010.0).abs() < 1e-6);
    assert_eq!(
        weekend.track_display_name.as_deref(),
        Some("Circuit de Spa-Francorchamps")
    );
}

#[test]
fn practice_basic_tells_the_story_of_the_drive() {
    let s = practice();
    let t = ticks(&s);
    let first = &t[0];
    assert!(first.is_in_garage && !first.is_on_track && first.track_loc == -1);

    let driving: Vec<_> = t
        .iter()
        .filter(|t| t.is_on_track && !t.is_in_garage)
        .collect();
    assert!(driving.iter().any(|t| t.in_pit_stall == Some(true)));
    assert!(driving.iter().any(|t| t.on_pit_road && t.speed > 20.0));
    assert!(driving.iter().any(|t| !t.on_pit_road && t.track_loc == 3));

    // The line is crossed once per lap in `Lap` and `LapCompleted`, and the official time
    // shows up after it.
    let laps: Vec<i32> = t
        .windows(2)
        .filter(|w| w[1].lap > w[0].lap)
        .map(|w| w[1].lap)
        .collect();
    assert_eq!(laps, [1, 2, 3, 4, 5, 6]);
    let last_times: Vec<f32> = t
        .windows(2)
        .filter(|w| w[1].lap_last_time != w[0].lap_last_time)
        .map(|w| w[1].lap_last_time)
        .collect();
    assert_eq!(last_times, [LAP_S as f32]);
    assert_eq!(t.iter().map(|t| t.lap_completed).max(), Some(6));

    // Fuel falls on track and rises in the stall.
    let end_fuel = t.last().expect("rows").fuel_l;
    assert!((85.0..95.0).contains(&end_fuel), "refuelled to {end_fuel}");

    // Time is monotonic at 60 Hz.
    assert!(
        t.windows(2)
            .all(|w| (w[1].session_time - w[0].session_time - 1.0 / 60.0).abs() < 1e-9)
    );
    let end = t.last().expect("rows");
    assert!(end.is_in_garage && !end.is_on_track);
}

#[test]
fn the_stop_with_readings_updates_the_setup_text() {
    let s = practice();
    let updates = s.yaml_updates();
    assert_eq!(updates.len(), 1);
    assert!(!updates[0].restart);
    assert!(updates[0].at_row > 0 && updates[0].at_row <= s.rows().len());
    let text = String::from_utf8(updates[0].text.clone()).expect("ascii");
    assert!(text.contains("UpdateCount: 2") && text.contains("LastHotPressure"));
    assert!(!String::from_utf8_lossy(s.initial_yaml()).contains("LastHotPressure"));
}

#[test]
fn scheduled_effects_happen_where_they_were_put() {
    let mut s = Scenario::new(YamlBuilder::new());
    s.garage(0.5).to_stall().drive_out();
    s.off_track(0.5, 1.0)
        .cut(0.8, 0.5)
        .incident_at(0.9, 2, incident_flags::REPORT_CAR_CONTACT);
    s.lap(LapProfile::new(LAP_S));
    let t = ticks(&s);

    let off: Vec<_> = t.iter().filter(|t| t.track_loc == 0).collect();
    assert!(!off.is_empty());
    assert!(
        off.iter()
            .all(|t| (0.49..0.7).contains(&t.lap_dist_pct) && t.surface == Some(15))
    );
    let cut: Vec<_> = t
        .iter()
        .filter(|t| t.surface == Some(15) && t.track_loc == 3)
        .collect();
    assert!(!cut.is_empty() && cut.iter().all(|t| t.lap_dist_pct >= 0.79));

    let hit = t
        .iter()
        .position(|t| t.incident_flags.unwrap_or(0) != 0)
        .expect("incident row");
    assert_eq!(t[hit].incidents, Some(2));
    assert_eq!(
        t[hit].incident_flags,
        Some(incident_flags::REPORT_CAR_CONTACT | incident_flags::PENALTY_2X)
    );
    assert_eq!(
        t.iter()
            .filter(|t| t.incident_flags.unwrap_or(0) != 0)
            .count(),
        1
    );
    assert_eq!(t.last().expect("rows").incidents, Some(2));
}

#[test]
fn immediate_steps_change_what_the_rows_show() {
    let mut s = Scenario::new(YamlBuilder::new());
    s.to_stall().drive_out().lap(LapProfile::new(LAP_S));
    s.incident(1, incident_flags::REPORT_OFF_TRACK);
    let n = s.rows().len();
    s.flags(session_flags::BLUE, 1.0);
    let flagged = s.rows().len();
    s.alongside(Side::Left, 0.5);
    let beside = s.rows().len();
    s.caution(1.0);
    let cautioned = s.rows().len();
    s.tow();
    s.chequered(3);
    let t = ticks(&s);

    assert_eq!(t[n - 1].incidents, Some(1));
    assert!(
        t[n..flagged]
            .iter()
            .all(|t| t.session_flags & session_flags::BLUE != 0)
    );
    assert_eq!(t[flagged].session_flags & session_flags::BLUE, 0);
    assert!(
        t[flagged..beside]
            .iter()
            .all(|t| t.car_left_right == Some(2))
    );
    assert_eq!(t[beside].car_left_right, Some(1));
    assert!(
        t[beside..cautioned]
            .iter()
            .all(|t| t.pits_open == Some(false) && t.session_flags & session_flags::CAUTION != 0)
    );
    assert_eq!(t[cautioned].pits_open, Some(true));

    let after_tow = t
        .iter()
        .rposition(|t| t.tow_time.unwrap_or(0.0) > 0.0)
        .expect("tow");
    assert!(t[after_tow + 1].in_pit_stall == Some(true) && t[after_tow + 1].speed == 0.0);
    let end = t.last().expect("rows");
    assert_eq!((end.session_state, end.position), (5, Some(3)));
    assert_ne!(end.session_flags & session_flags::CHECKERED, 0);
}

#[test]
fn sessions_events_and_gaps_are_recorded_for_the_replay() {
    let mut s = Scenario::new(YamlBuilder::new());
    s.garage(0.5).session_change(1, "Race").garage(0.5);
    s.disconnect(6.0).garage(0.5);
    s.new_event(YamlBuilder::new().session(0, "Lone Qualify"));
    let t = ticks(&s);

    let u = s.yaml_updates();
    assert_eq!(u.len(), 2);
    assert!(!u[0].restart && u[1].restart && u[0].at_row < u[1].at_row);
    assert!(String::from_utf8_lossy(&u[0].text).contains("CurrentSessionNum: 1"));
    assert!(String::from_utf8_lossy(&u[1].text).contains("Lone Qualify"));

    assert_eq!(t[0].session_num, 0);
    let race = &t[u[0].at_row];
    assert_eq!(race.session_num, 1);
    assert_eq!(race.session_unique_id, t[0].session_unique_id + 1);

    let &(gap_row, secs) = s.disconnects().first().expect("gap");
    assert_eq!(secs, 6.0);
    let jump = t[gap_row].session_time - t[gap_row - 1].session_time;
    assert!((jump - (6.0 + 1.0 / 60.0)).abs() < 1e-6, "{jump}");
    assert!(t[u[1].at_row].session_unique_id > race.session_unique_id);
}

#[test]
fn a_field_and_its_rivals_show_in_the_per_car_arrays() {
    let yaml = YamlBuilder::new()
        .roster(6, 2)
        .results(0, vec![ResultSpec::running(1, 1)]);
    let mut s = Scenario::new(yaml);
    s.field(3).to_stall().drive_out();
    s.rival(2).lap(92.5);
    s.rival(3).pit();
    s.stop(0.1, 0.0, None);
    let t = ticks(&s);
    let end = t.last().expect("rows");
    let active: Vec<usize> = (0..64).filter(|&i| end.car_loc[i] != -1).collect();
    assert_eq!(active.len(), 4, "player and three rivals: {active:?}");
    assert!(active.contains(&1), "the player's slot");
    assert_eq!(end.car_last.expect("present")[2], 92.5);
    assert_eq!(end.car_best.expect("present")[2], 92.5);
    assert_eq!(end.car_lap[2], 1);
    assert!(end.car_on_pit_road[3] && !end.car_on_pit_road[2]);
    assert_eq!(end.car_loc[0], -1, "the pace car is not on track");
}

#[test]
fn building_twice_gives_the_same_bytes() {
    let (a, b) = (practice(), practice());
    assert_eq!(a.rows(), b.rows());
    assert_eq!(a.initial_yaml(), b.initial_yaml());
    assert_eq!(a.yaml_updates(), b.yaml_updates());
}

#[cfg(windows)]
#[test]
fn replay_into_pushes_every_row_and_applies_the_text_changes() {
    use irsdk::testing::map::TestMap;
    use std::sync::atomic::AtomicBool;

    let mut s = Scenario::new(YamlBuilder::new());
    s.garage(0.2)
        .session_change(1, "Race")
        .disconnect(0.1)
        .garage(0.2);
    let name = format!("iracing_core_test_{}", std::process::id());
    let size = TestMap::required_size(s.layout(), s.initial_yaml().len());
    let mut map = TestMap::create(
        &name,
        &format!("{name}_event"),
        s.layout(),
        s.initial_yaml(),
        size,
    )
    .expect("test map");
    let pushed = s.replay_into(&mut map, 6000.0, &AtomicBool::new(false));
    assert_eq!(pushed, s.rows().len());
    assert_eq!(map.var_count(), s.layout().vars().len());
}
