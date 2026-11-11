//! The `stall_tyres` simulator data the recorder builds passes the runtime's checks, matches
//! the kind's description field by field, and maps the left and right readings to the edge
//! towards the car's centreline.

use iracing_core::io::{StallOccasion, StallTyres};
use iracing_core::sim_data::{STALL_TYRES, kinds};
use iracing_recorder::sink::stall_tyres_data;
use serde_json::{Map, Value};
use stintmate_connector_wire::sim_data::{check_data, check_kinds};

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

fn data(t: &StallTyres) -> Value {
    Value::Object(stall_tyres_data(t).expect("data"))
}

fn validate(v: &Value) -> Result<(), String> {
    let map: &Map<String, Value> = v.as_object().expect("object");
    check_data(STALL_TYRES, map).map_err(|e| e.to_string())
}

/// Every leaf of `v` as a field path, with array/object keys as they are.
fn leaf_paths(prefix: &str, v: &Value, out: &mut Vec<String>) {
    match v {
        Value::Object(m) => {
            for (k, child) in m {
                let p = if prefix.is_empty() {
                    k.clone()
                } else {
                    format!("{prefix}.{k}")
                };
                leaf_paths(&p, child, out);
            }
        }
        _ => out.push(prefix.to_owned()),
    }
}

/// Whether a concrete path matches a declared one (`*` stands for any one segment).
fn matches(declared: &str, path: &str) -> bool {
    let (d, p): (Vec<_>, Vec<_>) = (declared.split('.').collect(), path.split('.').collect());
    d.len() == p.len() && d.iter().zip(&p).all(|(d, p)| *d == "*" || d == p)
}

#[test]
fn the_kind_description_passes_the_checks() {
    assert_eq!(check_kinds(&kinds()), Ok(()));
    assert_eq!(kinds()[0].kind, STALL_TYRES);
}

#[test]
fn every_leaf_of_a_full_record_is_a_declared_field() {
    let k = &kinds()[0];
    let v = data(&tyres(StallOccasion::PitStop));
    let mut leaves = Vec::new();
    leaf_paths("", &v, &mut leaves);
    // occasion + 4 wheels x 6 values.
    assert_eq!(leaves.len(), 25);
    for leaf in &leaves {
        assert!(
            k.fields.keys().any(|d| matches(d, leaf)),
            "{leaf} is not described"
        );
    }
    // And every described leaf field appears in a full record.
    for d in k.fields.keys().filter(|d| !matches_container(d)) {
        assert!(leaves.iter().any(|l| matches(d, l)), "{d} never sent");
    }
}

fn matches_container(path: &str) -> bool {
    path == "wheels"
}

#[test]
fn every_occasion_validates() {
    for o in [
        StallOccasion::PitStop,
        StallOccasion::Reset,
        StallOccasion::CarEntry,
    ] {
        let v = data(&tyres(o));
        assert_eq!(validate(&v), Ok(()), "{v}");
        assert_eq!(v["occasion"], o.as_str());
    }
}

#[test]
fn inner_is_towards_the_centreline_on_each_side() {
    let v = data(&tyres(StallOccasion::PitStop));
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
    let v = data(&t);
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
    let v = data(&t);
    assert_eq!(validate(&v), Ok(()), "{v}");
    assert!(v["wheels"].get("fr").is_none());
    assert!(v["wheels"].get("fl").is_some());
}

#[test]
fn a_car_entry_carries_only_its_occasion() {
    let v = data(&tyres(StallOccasion::CarEntry));
    assert_eq!(validate(&v), Ok(()), "{v}");
    assert_eq!(v, serde_json::json!({"occasion": "car_entry"}));
    // Even with no reading in range it is still written: the entry itself is the record.
    let mut t = tyres(StallOccasion::CarEntry);
    t.temp_c = [[900.0; 3]; 4];
    t.wear = [[f32::NAN; 3]; 4];
    assert!(stall_tyres_data(&t).is_some());
}

#[test]
fn no_message_when_no_wheel_is_left() {
    let mut t = tyres(StallOccasion::PitStop);
    t.temp_c = [[900.0; 3]; 4];
    t.wear = [[f32::NAN; 3]; 4];
    assert!(stall_tyres_data(&t).is_none());
}
