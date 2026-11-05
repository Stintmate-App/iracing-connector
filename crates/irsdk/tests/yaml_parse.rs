//! Requoting, section splitting, the typed parse and the unit helpers.

use irsdk::yaml::{self, SectionHashes, decode, requote, sections, units};

fn corpus(name: &str) -> String {
    let path = format!("{}/tests/corpus/yaml/{name}", env!("CARGO_MANIFEST_DIR"));
    decode(&std::fs::read(path).expect("corpus file")).text
}

#[test]
fn every_typed_field_of_the_full_corpus_file() {
    let p = yaml::parse(&corpus("full.yaml"));
    assert!(p.errors.is_empty(), "{:?}", p.errors);

    let w = p.info.weekend.expect("weekend");
    assert_eq!(w.track_name.as_deref(), Some("testtrack gp"));
    assert_eq!(w.track_id, Some(123));
    assert_eq!(w.track_length.as_deref(), Some("5.79 km"));
    assert_eq!(
        w.track_display_name.as_deref(),
        Some("Test Track Grand Prix")
    );
    assert_eq!(w.track_config_name.as_deref(), Some("Grand Prix"));
    assert_eq!(w.track_pit_speed_limit.as_deref(), Some("80.00 kph"));
    assert_eq!(w.track_num_turns, Some(14));
    assert_eq!(w.series_id, Some(77));
    assert_eq!(w.session_id, Some(900_100));
    assert_eq!(w.sub_session_id, Some(900_200));
    assert_eq!(w.event_type.as_deref(), Some("Race"));
    assert_eq!(w.category.as_deref(), Some("Road"));
    assert_eq!(w.sim_mode.as_deref(), Some("full"));
    assert_eq!(w.official, Some(0));
    assert_eq!(w.team_racing, Some(0));
    assert_eq!(w.num_car_classes, Some(1));
    assert_eq!(w.track_weather_type.as_deref(), Some("Static"));
    let o = w.weekend_options.as_ref().expect("options");
    assert_eq!(o.incident_limit.as_deref(), Some("17"));
    assert_eq!(o.is_fixed_setup, Some(1));
    assert_eq!(o.standing_start, Some(0));
    assert_eq!(o.fast_repairs_limit.as_deref(), Some("unlimited"));
    assert!(w.telemetry_options.is_some());
    assert_eq!(w.track_length_m(), Some(5790.0));
    assert_eq!(w.pit_speed_limit_kmh(), Some(80.0));
    assert_eq!(w.incident_limit(), Some(17));

    let s = p.info.session.expect("session");
    assert_eq!(s.current_session_num, Some(1));
    let sessions = s.sessions.as_ref().expect("sessions");
    assert_eq!(sessions.len(), 2);
    let race = s.current(None).expect("current");
    assert_eq!(race.session_type.as_deref(), Some("Race"));
    assert_eq!(race.session_name.as_deref(), Some("RACE"));
    assert_eq!(race.laps(), Some(30));
    assert_eq!(race.time_s(), None);
    let r = &race.results_positions.as_ref().expect("results")[0];
    assert_eq!(r.position, Some(1));
    assert_eq!(r.class_position, Some(0));
    assert_eq!(r.car_idx, Some(3));
    assert_eq!(r.lap, Some(12));
    assert_eq!(r.time, Some(91.4521));
    assert_eq!(r.fastest_lap, Some(7));
    assert_eq!(r.fastest_time, Some(90.8811));
    assert_eq!(r.last_time, Some(91.4521));
    assert_eq!(r.laps_led, Some(12));
    assert_eq!(r.laps_complete, Some(12));
    assert_eq!(r.incidents, Some(2));
    assert_eq!(r.reason_out_id, Some(0));
    assert_eq!(r.reason_out_str.as_deref(), Some("Running"));
    let practice = &sessions[0];
    assert_eq!(practice.laps(), None);
    assert_eq!(practice.time_s(), Some(900.0));

    let q = &p.info.qualify.expect("qualify").results.expect("results")[0];
    assert_eq!(q.car_idx, Some(3));
    assert_eq!(q.fastest_lap, Some(4));
    assert_eq!(q.fastest_time, Some(90.5));
    assert_eq!(q.position, Some(0));
    assert_eq!(q.class_position, Some(0));

    let sectors = p.info.split.expect("split").sectors.expect("sectors");
    assert_eq!(sectors.len(), 3);
    assert_eq!(sectors[1].sector_num, Some(1));
    assert_eq!(sectors[1].sector_start_pct, Some(0.3333));

    let d = p.info.driver.expect("driver");
    assert_eq!(d.driver_car_idx, Some(3));
    assert_eq!(d.pace_car_idx, Some(-1));
    assert_eq!(d.driver_car_fuel_max_ltr, Some(106.0));
    assert_eq!(d.driver_car_fuel_kg_per_ltr, Some(0.75));
    assert_eq!(d.driver_car_max_fuel_pct, Some(0.5));
    assert_eq!(d.driver_car_red_line, Some(7500.0));
    assert_eq!(d.driver_car_est_lap_time, Some(90.1234));
    assert_eq!(d.driver_setup_name.as_deref(), Some("baseline.sto"));
    assert_eq!(d.driver_setup_is_modified, Some(0));
    assert_eq!(d.driver_incident_count, Some(2));
    assert_eq!(d.fuel_capacity_l(), Some(53.0));
    assert_eq!(d.drivers.as_ref().map(Vec::len), Some(4));
    let me = d.player().expect("player");
    assert_eq!(me.car_idx, Some(3));
    assert_eq!(me.user_name.as_deref(), Some("Sam Tester"));
    assert_eq!(me.abbrev_name.as_deref(), Some("Tester, S"));
    assert_eq!(me.initials.as_deref(), Some("ST"));
    assert_eq!(me.user_id, Some(100_003));
    assert_eq!(me.team_id, Some(4));
    assert_eq!(me.team_name.as_deref(), Some("Test Racing"));
    // A plain scalar `7` into a String field.
    assert_eq!(me.car_number.as_deref(), Some("7"));
    assert_eq!(me.car_number_raw, Some(7));
    assert_eq!(me.car_id, Some(55));
    assert_eq!(me.car_path.as_deref(), Some("testcar\\gt3"));
    assert_eq!(me.car_screen_name.as_deref(), Some("Test GT3"));
    assert_eq!(me.car_class_id, Some(11));
    assert_eq!(me.car_class_short_name.as_deref(), Some("GT3"));
    assert_eq!(me.car_class_est_lap_time, Some(91.5));
    assert_eq!(me.car_is_pace_car, Some(0));
    assert_eq!(me.car_is_ai, Some(0));
    assert_eq!(me.is_spectator, Some(0));
    assert_eq!(me.cur_driver_incident_count, Some(2));
    assert_eq!(me.team_incident_count, Some(2));
    assert!(me.is_real());
    let list = d.drivers.as_ref().expect("drivers");
    assert!(list[0].is_real());
    assert!(!list[1].is_real(), "pace car");
    assert!(!list[2].is_real(), "spectator");
    // A quoted number stays text.
    assert_eq!(list[0].car_number.as_deref(), Some("12"));

    assert_eq!(p.info.car_setup.expect("setup").update_count(), Some(5));
}

#[test]
fn session_lookup_falls_back_to_the_telemetry_number() {
    let p = yaml::parse(
        "SessionInfo:\n CurrentSessionNum: 9\n Sessions:\n - SessionNum: 0\n   SessionType: A\n - SessionNum: 2\n   SessionType: B\n",
    );
    let s = p.info.session.expect("session");
    assert_eq!(
        s.current(Some(2)).and_then(|x| x.session_type.as_deref()),
        Some("B")
    );
    assert!(s.current(None).is_none());
    assert!(s.current(Some(7)).is_none());
}

#[test]
fn a_broken_section_loses_only_itself() {
    let p = yaml::parse(&corpus("broken_car_setup.yaml"));
    assert_eq!(p.errors.len(), 1, "{:?}", p.errors);
    assert_eq!(p.errors[0].0, "CarSetup");
    assert!(p.info.car_setup.is_none());
    assert!(p.info.weekend.is_some());
    assert!(p.info.driver.is_some());
    assert!(p.info.session.is_some());
}

#[test]
fn a_name_with_a_line_break_loses_the_driver_section_only() {
    let p = yaml::parse(&corpus("name_line_break.yaml"));
    assert_eq!(p.errors.len(), 1, "{:?}", p.errors);
    assert_eq!(p.errors[0].0, "DriverInfo");
    assert!(p.info.driver.is_none());
    assert_eq!(
        p.info.weekend.expect("weekend").track_name.as_deref(),
        Some("testtrack gp")
    );
    assert!(p.info.car_setup.is_some());
}

#[test]
fn empty_sections_parse_as_absent() {
    let p = yaml::parse(&corpus("empty_sections.yaml"));
    assert!(p.errors.is_empty(), "{:?}", p.errors);
    assert_eq!(p.info, Default::default());
}

#[test]
fn missing_and_unknown_sections_are_fine() {
    let p = yaml::parse(
        "---\nBrandNewSection:\n A: 1\nWeekendInfo:\n TrackID: 4\n NewKey: [1, 2]\n...\n",
    );
    assert!(p.errors.is_empty(), "{:?}", p.errors);
    assert_eq!(p.info.weekend.expect("weekend").track_id, Some(4));
    assert!(p.info.driver.is_none());
    assert_eq!(yaml::parse(""), Default::default());
}

#[test]
fn cp1252_names_parse_after_decoding() {
    let path = format!(
        "{}/tests/corpus/yaml/cp1252_names.yaml",
        env!("CARGO_MANIFEST_DIR")
    );
    let d = decode(&std::fs::read(path).unwrap());
    let p = yaml::parse(&d.text);
    assert!(p.errors.is_empty(), "{:?}", p.errors);
    let drv = p.info.driver.expect("driver");
    let me = drv.player().expect("player");
    assert_eq!(me.user_name.as_deref(), Some("Jos\u{E9} M\u{FC}ller"));
    assert_eq!(
        me.team_name.as_deref(),
        Some("Caf\u{E9} \u{201C}Racing\u{201D} \u{20AC}")
    );
    assert_eq!(
        p.info.weekend.expect("weekend").track_name.as_deref(),
        Some("caf\u{E9} ring")
    );
}

#[test]
fn utf8_names_parse() {
    let path = format!(
        "{}/tests/corpus/yaml/utf8_marker.yaml",
        env!("CARGO_MANIFEST_DIR")
    );
    let d = decode(&std::fs::read(path).unwrap());
    assert!(d.utf8);
    let p = yaml::parse(&d.text);
    assert!(p.errors.is_empty(), "{:?}", p.errors);
    let drv = p.info.driver.expect("driver");
    assert_eq!(
        drv.player().unwrap().user_name.as_deref(),
        Some("Jos\u{E9} M\u{FC}ller")
    );
}

#[test]
fn a_grid_of_64_cars() {
    let p = yaml::parse(&corpus("grid_64.yaml"));
    assert!(p.errors.is_empty(), "{:?}", p.errors);
    let d = p.info.driver.expect("driver");
    let list = d.drivers.as_ref().expect("drivers");
    assert_eq!(list.len(), 64);
    assert_eq!(d.player().unwrap().car_idx, Some(17));
    assert_eq!(
        d.player().unwrap().user_name.as_deref(),
        Some("Driver 17 Example")
    );
    assert!(list.iter().all(|x| x.is_real()));
}

#[test]
fn special_values() {
    let p = yaml::parse(&corpus("special_values.yaml"));
    // The tab inside the unknown section is ignored with the section.
    assert!(
        p.errors.iter().all(|(s, _)| s == "CarSetup"),
        "{:?}",
        p.errors
    );
    let w = p.info.weekend.expect("weekend");
    assert!((w.track_length_m().unwrap() - 3.60 * 1609.344).abs() < 1e-6);
    assert!((w.pit_speed_limit_kmh().unwrap() - 45.0 * 1.609344).abs() < 1e-9);
    assert_eq!(w.incident_limit(), None);
    let s = p.info.session.expect("session");
    let cur = s.current(None).expect("current");
    // `1e5` is not a lap count the connector accepts as integer text.
    assert_eq!(cur.session_laps.as_deref(), Some("1e5"));
    assert_eq!(cur.laps(), None);
    let d = p.info.driver.expect("driver");
    assert_eq!(d.player().unwrap().car_number.as_deref(), Some("007"));
    assert_eq!(d.fuel_capacity_l(), Some(80.0));
}

#[test]
fn duplicate_keys_in_the_car_setup_never_reach_other_sections() {
    let p = yaml::parse(&corpus("special_values.yaml"));
    // Whether the duplicate is an error or the last value wins, nothing else is lost.
    assert!(p.info.driver.is_some());
    assert!(p.info.weekend.is_some());
    if p.info.car_setup.is_none() {
        assert_eq!(p.errors.len(), 1);
    }
}

#[test]
fn plain_scalars_into_string_fields() {
    let p = yaml::parse("WeekendInfo:\n TrackName: 7\n TrackLength: 5\n Category: true\n");
    assert!(p.errors.is_empty(), "{:?}", p.errors);
    let w = p.info.weekend.unwrap();
    assert_eq!(w.track_name.as_deref(), Some("7"));
    assert_eq!(w.track_length_m(), Some(5.0));
    assert_eq!(w.category.as_deref(), Some("true"));
}

// requote

fn rq(line: &str) -> String {
    requote(line).into_owned()
}

#[test]
fn requote_cases() {
    assert_eq!(rq(" UserName: Sam Tester\n"), " UserName: \"Sam Tester\"\n");
    assert_eq!(
        rq("   UserName: say \"hi\""),
        "   UserName: \"say \\\"hi\\\"\""
    );
    assert_eq!(
        rq(" UserName: C:\\dir\\x\n"),
        " UserName: \"C:\\\\dir\\\\x\"\n"
    );
    assert_eq!(rq(" TeamName: Red: Blue\n"), " TeamName: \"Red: Blue\"\n");
    assert_eq!(rq(" TeamName: a # b\n"), " TeamName: \"a # b\"\n");
    assert_eq!(rq(" UserName: - dash\n"), " UserName: \"- dash\"\n");
    assert_eq!(rq(" UserName: *star\n"), " UserName: \"*star\"\n");
    assert_eq!(rq(" UserName: &anchor\n"), " UserName: \"&anchor\"\n");
    assert_eq!(rq(" UserName: !tag\n"), " UserName: \"!tag\"\n");
    assert_eq!(rq(" UserName: @at\n"), " UserName: \"@at\"\n");
    assert_eq!(rq(" TeamName: \n"), " TeamName: \"\"\n");
    assert_eq!(rq(" TeamName:\n"), " TeamName: \"\"\n");
    assert_eq!(rq(" UserName: \"already\"\n"), " UserName: \"already\"\n");
    assert_eq!(rq(" - UserName: x"), " - UserName: \"x\"");
    assert_eq!(rq("- Initials: AB\n"), "- Initials: \"AB\"\n");
    assert_eq!(rq(" AbbrevName: Doe, J\n"), " AbbrevName: \"Doe, J\"\n");
    assert_eq!(
        rq(" DriverSetupName: base.sto\n"),
        " DriverSetupName: \"base.sto\"\n"
    );
}

#[test]
fn requote_leaves_other_lines_alone() {
    let text = " UserID: 5\n UserNameX: a\n NotUserName: b\n CarPath: x: y\n";
    assert!(matches!(requote(text), std::borrow::Cow::Borrowed(_)));
    assert_eq!(
        rq("A: 1\n UserName: n\nB: 2\n"),
        "A: 1\n UserName: \"n\"\nB: 2\n"
    );
}

#[test]
fn requoted_difficult_names_parse() {
    let text = "DriverInfo:\n DriverCarIdx: 0\n Drivers:\n - CarIdx: 0\n   UserName: Dan \"Dash\" O'Neil: #1 *fast*\n   TeamName: \\\\server\\team\n   Initials: \n";
    let p = yaml::parse(text);
    assert!(p.errors.is_empty(), "{:?}", p.errors);
    let me = p.info.driver.unwrap().player().cloned().unwrap();
    assert_eq!(
        me.user_name.as_deref(),
        Some("Dan \"Dash\" O'Neil: #1 *fast*")
    );
    assert_eq!(me.team_name.as_deref(), Some("\\\\server\\team"));
    assert_eq!(me.initials.as_deref(), Some(""));
}

// sections

#[test]
fn sections_split_at_unindented_headers() {
    let text = "---\nA:\n x: 1\n - y\nB:\n z: 2\n...\nC:\n";
    let s = sections(text);
    let names: Vec<&str> = s.iter().map(|x| x.0).collect();
    assert_eq!(names, ["A", "B", "C"]);
    assert_eq!(s[0].1, "A:\n x: 1\n - y\n");
    assert_eq!(s[1].1, "B:\n z: 2\n...\n");
    assert_eq!(s[2].1, "C:\n");
    assert!(sections("").is_empty());
    assert!(sections("---\n...\n").is_empty());
}

#[test]
fn section_hashes_tell_what_changed() {
    let a = corpus("full.yaml");
    let h = SectionHashes::of(&a);
    assert!(h.weekend != 0 && h.session != 0 && h.qualify != 0);
    assert!(h.split != 0 && h.driver != 0 && h.car_setup != 0 && h.other != 0);
    assert_eq!(h, SectionHashes::of(&a));
    let b = a.replace("UpdateCount: 5", "UpdateCount: 6");
    let hb = SectionHashes::of(&b);
    assert_ne!(hb.car_setup, h.car_setup);
    assert_eq!(hb.weekend, h.weekend);
    assert_eq!(hb.driver, h.driver);
    assert_eq!(SectionHashes::of(""), SectionHashes::default());
}

// units

#[test]
fn unit_helpers() {
    assert_eq!(units::number_unit("5.79 km"), Some((5.79, "km")));
    assert_eq!(units::number_unit("  12  "), Some((12.0, "")));
    assert_eq!(units::number_unit("unlimited"), None);
    assert_eq!(units::number_unit(""), None);
    assert_eq!(units::number_unit("nan"), None);
    assert_eq!(units::length_m("5.79 km"), Some(5790.0));
    assert_eq!(units::length_m("120 m"), Some(120.0));
    assert!((units::length_m("1 mi").unwrap() - 1609.344).abs() < 1e-9);
    assert_eq!(units::length_m("3 furlong"), None);
    assert_eq!(units::speed_kmh("80.00 kph"), Some(80.0));
    assert_eq!(units::speed_kmh("80 km/h"), Some(80.0));
    assert!((units::speed_kmh("50 mph").unwrap() - 80.4672).abs() < 1e-9);
    assert_eq!(units::pressure_kpa("172 kPa"), Some(172.0));
    assert!((units::pressure_kpa("1 bar").unwrap() - 100.0).abs() < 1e-9);
    assert_eq!(units::temp_c("25.5 C"), Some(25.5));
    assert!((units::temp_c("212 F").unwrap() - 100.0).abs() < 1e-9);
}

#[test]
fn derived_helpers_without_data() {
    let w = irsdk::session::WeekendInfo::default();
    assert_eq!(w.track_length_m(), None);
    assert_eq!(w.pit_speed_limit_kmh(), None);
    assert_eq!(w.incident_limit(), None);
    let d = irsdk::session::DriverInfo::default();
    assert!(d.player().is_none());
    assert_eq!(d.fuel_capacity_l(), None);
    assert!(!irsdk::session::Driver::default().is_real());
}
