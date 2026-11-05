//! The line scanner: every field, from text the typed parse refuses.

use irsdk::yaml::{self, decode, fallback::extract};

fn corpus(name: &str) -> String {
    let path = format!("{}/tests/corpus/yaml/{name}", env!("CARGO_MANIFEST_DIR"));
    decode(&std::fs::read(path).expect("corpus file")).text
}

#[test]
fn every_field_from_a_driver_list_the_typed_parse_refuses() {
    let text = corpus("name_line_break.yaml");
    let parsed = yaml::parse(&text);
    assert!(
        parsed.errors.iter().any(|(s, _)| s == "DriverInfo"),
        "the driver list must fail: {:?}",
        parsed.errors
    );
    assert!(parsed.info.driver.is_none());
    // The other sections are not lost.
    assert!(parsed.info.weekend.is_some());
    assert!(parsed.info.session.is_some());

    let f = extract(&text);
    assert_eq!(f.track_id, Some(123));
    assert_eq!(f.track_name.as_deref(), Some("testtrack gp"));
    assert_eq!(
        f.track_display_name.as_deref(),
        Some("Test Track Grand Prix")
    );
    assert_eq!(f.track_config_name.as_deref(), Some("Grand Prix"));
    assert_eq!(f.track_length.as_deref(), Some("5.79 km"));
    assert_eq!(f.sub_session_id, Some(900_200));
    assert_eq!(f.session_id, Some(900_100));
    assert_eq!(f.current_session_num, Some(1));
    assert_eq!(f.driver_car_idx, Some(3));
    assert_eq!(f.fuel_max_ltr, Some(106.0));
    assert_eq!(f.max_fuel_pct, Some(0.5));
    assert_eq!(f.player_car_path.as_deref(), Some("testcar\\gt3"));
    assert_eq!(f.player_car_screen_name.as_deref(), Some("Test GT3"));
    assert_eq!(f.player_car_class_id, Some(11));
    assert_eq!(f.player_car_class_short_name.as_deref(), Some("GT3"));
    assert_eq!(f.player_car_id, Some(55));
    assert_eq!(
        f.session_types,
        vec![(0, "Practice".to_owned()), (1, "Race".to_owned())]
    );
    assert_eq!(f.sector_start_pcts, vec![0.0, 0.3333, 0.6667]);
    assert_eq!(f.car_setup_update_count, Some(5));
}

#[test]
fn a_broken_car_setup_still_gives_its_update_count() {
    let text = corpus("broken_car_setup.yaml");
    let parsed = yaml::parse(&text);
    assert!(parsed.errors.iter().any(|(s, _)| s == "CarSetup"));
    assert_eq!(extract(&text).car_setup_update_count, Some(5));
}

#[test]
fn the_player_is_picked_by_driver_car_idx_in_a_large_grid() {
    let f = extract(&corpus("grid_64.yaml"));
    assert_eq!(f.driver_car_idx, Some(17));
    assert_eq!(f.player_car_path.as_deref(), Some("testcar\\gt3"));
    assert_eq!(f.player_car_id, Some(55));
}

#[test]
fn player_after_the_list_and_keys_after_the_list_do_not_leak() {
    let text = "DriverInfo:\n Drivers:\n - CarIdx: 0\n   CarPath: a\n - CarIdx: 1\n   CarPath: b\n   CarID: 9\n DriverCarIdx: 1\n";
    let f = extract(text);
    assert_eq!(f.driver_car_idx, Some(1));
    assert_eq!(f.player_car_path.as_deref(), Some("b"));
    assert_eq!(f.player_car_id, Some(9));
}

#[test]
fn quoted_values_lose_their_quotes_and_first_occurrence_wins() {
    let f = extract("WeekendInfo:\n TrackName: \"quoted name\"\n TrackName: other\n TrackID: x\n");
    assert_eq!(f.track_name.as_deref(), Some("quoted name"));
    assert_eq!(f.track_id, None);
}

#[test]
fn nothing_found_gives_the_default() {
    assert_eq!(extract(""), Default::default());
    assert_eq!(extract("garbage\n: :\n- -\n"), Default::default());
    assert_eq!(extract(&corpus("empty_sections.yaml")), Default::default());
}

#[test]
fn keys_in_other_sections_are_not_taken() {
    let f = extract("CameraInfo:\n TrackID: 5\nWeekendInfo:\n TrackID: 6\n");
    assert_eq!(f.track_id, Some(6));
}
