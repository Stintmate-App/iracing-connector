//! The `tyre_readings` message the recorder builds validates against the write API schema, with
//! the left and right readings mapped to the edge towards the car's centreline.

use iracing_core::io::{StallOccasion, StallTyres};
use iracing_recorder::sink::tyre_readings_message;
use serde_json::{Value, json};
use stintmate_connector_wire::schema_check::{PROTOCOL_SCHEMA, WRITE_API_SCHEMA, definition};

fn tyres(occasion: StallOccasion) -> StallTyres {
    // Rows of a real stop: left, middle and right reading of each wheel (left front, right
    // front, left rear, right rear).
    StallTyres {
        at_s: 1119.77,
        occasion,
        temp_c: [
            [73.9, 75.56, 74.1],
            [71.2, 73.0, 72.4],
            [70.0, 71.0, 72.0],
            [68.0, 69.0, 70.0],
        ],
        wear: [
            [0.9822, 0.9817, 0.9811],
            [0.9805, 0.9801, 0.9790],
            [0.99, 0.99, 0.99],
            [0.98, 0.98, 0.98],
        ],
    }
}

fn message(t: &StallTyres) -> Value {
    let m = tyre_readings_message("2026-11-07T11:52:41.512+01:00", t).expect("a message");
    let mut v = serde_json::to_value(&m).expect("json");
    let o = v.as_object_mut().expect("object");
    o.insert("type".into(), json!("tyre_readings"));
    o.insert("session_id".into(), json!(1));
    v
}

fn validate(v: &Value) -> Result<(), String> {
    let docs: Vec<Value> = [WRITE_API_SCHEMA, PROTOCOL_SCHEMA]
        .iter()
        .map(|s| serde_json::from_str(s).expect("schema"))
        .collect();
    definition(&docs, "tyre_readings", v)
}

#[test]
fn every_occasion_validates() {
    for o in [
        StallOccasion::PitStop,
        StallOccasion::Reset,
        StallOccasion::CarEntry,
    ] {
        let v = message(&tyres(o));
        assert_eq!(validate(&v), Ok(()), "{v}");
        assert_eq!(v["occasion"], o.as_str());
    }
}

#[test]
fn inner_is_towards_the_centreline_on_each_side() {
    let v = message(&tyres(StallOccasion::PitStop));
    // Left wheel: the right reading is the inner one.
    assert_eq!(v["wheels"]["fl"]["carcass_out_c"], 73.9);
    assert_eq!(v["wheels"]["fl"]["carcass_in_c"], 74.1);
    assert_eq!(v["wheels"]["fl"]["tread_out"], 0.9822);
    assert_eq!(v["wheels"]["fl"]["tread_in"], 0.9811);
    // Right wheel: the left reading is the inner one.
    assert_eq!(v["wheels"]["fr"]["carcass_in_c"], 71.2);
    assert_eq!(v["wheels"]["fr"]["carcass_out_c"], 72.4);
    assert_eq!(v["wheels"]["rr"]["carcass_in_c"], 68.0);
    assert_eq!(v["wheels"]["rl"]["carcass_in_c"], 72.0);
    assert!(v["wheels"]["fl"].get("pressure_hot_kpa").is_none());
    assert!(v["wheels"]["fl"].get("surface_mid_c").is_none());
}

#[test]
fn a_value_outside_its_range_is_left_out_not_clamped() {
    let mut t = tyres(StallOccasion::Reset);
    t.temp_c[0][1] = 900.0;
    t.wear[1][0] = 1.5;
    t.wear[2][2] = f32::NAN;
    let v = message(&t);
    assert_eq!(validate(&v), Ok(()), "{v}");
    assert!(v["wheels"]["fl"].get("carcass_mid_c").is_none());
    assert!(v["wheels"]["fr"].get("tread_in").is_none());
    assert!(v["wheels"]["rl"].get("tread_in").is_none());
}

#[test]
fn a_wheel_with_every_value_dropped_is_left_out() {
    let mut t = tyres(StallOccasion::PitStop);
    t.temp_c[1] = [900.0; 3];
    t.wear[1] = [f32::NAN; 3];
    let v = message(&t);
    assert_eq!(validate(&v), Ok(()), "{v}");
    assert!(v["wheels"].get("fr").is_none());
    assert!(v["wheels"].get("fl").is_some());
}

#[test]
fn no_message_when_no_wheel_is_left() {
    let mut t = tyres(StallOccasion::PitStop);
    t.temp_c = [[900.0; 3]; 4];
    t.wear = [[f32::NAN; 3]; 4];
    assert!(tyre_readings_message("2026-11-07T11:52:41.512+01:00", &t).is_none());
}
