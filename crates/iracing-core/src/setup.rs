//! The car setup: the session text's `CarSetup` flattened, turned into Pitwall's setup message,
//! and the tyre readings the session text carries.
//!
//! Only what the user sets goes to the setup message. The readings (hot pressure, temperatures,
//! tread, corner weights) are never in it: they are kept apart in [`TyreReadings`].

use std::collections::BTreeMap;

use serde_json::Value;
use stintmate_connector_wire::messages::{ExtraEntry, ExtraValue, Setup};

/// The wheels in the order the recorder uses: left front, right front, left rear, right rear.
pub const WHEEL_NAMES: [&str; 4] = ["LeftFront", "RightFront", "LeftRear", "RightRear"];

/// The longest key of an `extra` entry.
const EXTRA_KEY_MAX: usize = 64;

/// What the session text says about one tyre after the last stop.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct WheelReadings {
    /// The hot pressure in kPa, when the text has one.
    pub hot_pressure_kpa: Option<f64>,
    /// The temperatures in degrees Celsius, outer, middle, inner (the car's outside first).
    pub temps_c: Option<[f64; 3]>,
    /// The tread remaining in percent, three values in the order the text writes them.
    pub tread_remaining: Option<[f64; 3]>,
}

/// The tyre readings of the session text, per wheel (left front, right front, left rear, right
/// rear).
///
/// `LastTempsOMI` is read as outer, middle, inner and `LastTempsIMO` as inner, middle, outer and
/// reversed, so [`WheelReadings::temps_c`] always starts at the outside. The readings are shown in
/// the live frame only; they are not part of a setup message (the readings taken in the pits go in
/// their own `stall_tyres` simulator data records).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TyreReadings {
    /// One entry per wheel.
    pub wheels: [WheelReadings; 4],
    /// The setup's `UpdateCount` the readings were read at.
    pub at_update: Option<i64>,
}

impl TyreReadings {
    /// Whether no wheel has any reading.
    pub fn is_empty(&self) -> bool {
        self.wheels == [WheelReadings::default(); 4]
    }
}

/// The car setup flattened to path and text pairs, in the order the value gives them.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CarSetupSnapshot {
    /// The setup's `UpdateCount`, which grows when the driver changes it.
    pub update_count: Option<i64>,
    /// Every leaf of the setup that is a setting: the dotted path of its keys and its text.
    /// Readings and the update counter are not here.
    pub entries: Vec<(String, String)>,
    /// The tyre readings found in the setup.
    pub readings: TyreReadings,
    /// Kilograms of fuel per litre, to read a fuel level given in kilograms.
    pub fuel_kg_per_l: Option<f64>,
    /// Whether the event uses a fixed setup (`IsFixedSetup`).
    pub fixed_setup: Option<bool>,
}

impl CarSetupSnapshot {
    /// Flattens the `CarSetup` value. `fuel_kg_per_l` is the car's fuel density when known.
    pub fn from_value(value: &Value, fuel_kg_per_l: Option<f64>) -> CarSetupSnapshot {
        let mut snap = CarSetupSnapshot {
            update_count: value.get("UpdateCount").and_then(Value::as_i64),
            fuel_kg_per_l,
            ..CarSetupSnapshot::default()
        };
        snap.readings.at_update = snap.update_count;
        let mut path = Vec::new();
        snap.walk(value, &mut path);
        snap
    }

    fn walk(&mut self, value: &Value, path: &mut Vec<String>) {
        match value {
            Value::Object(map) => {
                for (key, v) in map {
                    path.push(key.clone());
                    self.walk(v, path);
                    path.pop();
                }
            }
            Value::Array(items) => {
                for (i, v) in items.iter().enumerate() {
                    path.push(i.to_string());
                    self.walk(v, path);
                    path.pop();
                }
            }
            Value::Null => {}
            leaf => {
                let text = match leaf {
                    Value::String(s) => s.trim().to_owned(),
                    other => other.to_string(),
                };
                self.leaf(path, text);
            }
        }
    }

    fn leaf(&mut self, path: &[String], text: String) {
        let Some(last) = path.last() else { return };
        if path.len() == 1 && last.eq_ignore_ascii_case("UpdateCount") {
            return;
        }
        if self.reading(path, &text) {
            return;
        }
        if is_reading_key(last) || text.is_empty() {
            return;
        }
        self.entries.push((path.join("."), text));
    }

    /// Stores a tyre reading; `true` when `path` is one.
    fn reading(&mut self, path: &[String], text: &str) -> bool {
        let Some((last, rest)) = path.split_last() else {
            return false;
        };
        let key = last.to_ascii_lowercase();
        let known = matches!(
            key.as_str(),
            "lasthotpressure" | "lasttempsomi" | "lasttempsimo" | "treadremaining"
        );
        if !known {
            return false;
        }
        let wheel = rest.last().and_then(|segment| {
            WHEEL_NAMES
                .iter()
                .position(|w| w.eq_ignore_ascii_case(segment))
        });
        let Some(w) = wheel else { return true };
        let r = &mut self.readings.wheels[w];
        match key.as_str() {
            "lasthotpressure" => r.hot_pressure_kpa = pressure_kpa(text),
            "lasttempsomi" => r.temps_c = triple(text, temp_c),
            "lasttempsimo" => {
                r.temps_c =
                    triple(text, temp_c).map(|[inner, middle, outer]| [outer, middle, inner]);
            }
            _ => r.tread_remaining = triple(text, |n, _| Some(n)),
        }
        true
    }

    /// The setup message. `reason` is `session_start` or `pit_exit`, `at` the ISO time it is
    /// taken at, and `fuel_l` the fuel in the tank when known (it replaces the setup's own
    /// fuel level).
    pub fn to_setup(&self, reason: &str, at: &str, fuel_l: Option<f64>) -> Setup {
        to_setup(self, reason, at, fuel_l)
    }
}

/// Whether the key holds a reading that never goes in the setup message.
fn is_reading_key(key: &str) -> bool {
    let k = key.to_ascii_lowercase();
    k == "lasthotpressure"
        || k == "treadremaining"
        || k == "cornerweight"
        || k.starts_with("lasttemps")
}

/// Splits a text such as `"54.5%"`, `"3 (ABS)"` or `"78C"` into a number and the rest.
fn split_number(s: &str) -> Option<(f64, &str)> {
    let s = s.trim();
    let end = s
        .char_indices()
        .find(|&(i, c)| !(c.is_ascii_digit() || c == '.' || (i == 0 && (c == '-' || c == '+'))))
        .map_or(s.len(), |(i, _)| i);
    let n: f64 = s[..end].parse().ok()?;
    n.is_finite().then(|| (n, s[end..].trim()))
}

fn pressure_kpa(text: &str) -> Option<f64> {
    let (n, unit) = split_number(text)?;
    let k = match unit.to_ascii_lowercase().as_str() {
        "kpa" | "" => 1.0,
        "psi" => 6.894_757,
        "bar" => 100.0,
        _ => return None,
    };
    Some(n * k)
}

fn temp_c(n: f64, unit: &str) -> Option<f64> {
    match unit.to_ascii_lowercase().as_str() {
        "c" | "" => Some(n),
        "f" => Some((n - 32.0) / 1.8),
        _ => None,
    }
}

/// Three comma-separated values, each read by `f` from its number and unit.
fn triple(text: &str, f: impl Fn(f64, &str) -> Option<f64>) -> Option<[f64; 3]> {
    let mut out = [0.0; 3];
    let mut parts = text.split(',');
    for slot in &mut out {
        let (n, unit) = split_number(parts.next()?)?;
        *slot = f(n, unit)?;
    }
    Some(out)
}

/// Stores `v` in `slot` when the slot is empty; `true` when it did.
fn put(slot: &mut Option<f64>, v: Option<f64>) -> bool {
    if slot.is_none() && v.is_some() {
        *slot = v;
        true
    } else {
        false
    }
}

fn last_segment(path: &str) -> &str {
    path.rsplit('.').next().unwrap_or(path)
}

/// Whether `path` ends with the segments of `suffix` (a dotted pair), ignoring case.
fn ends_with_path(path: &str, suffix: &str) -> bool {
    let p = path.to_ascii_lowercase();
    let s = suffix.to_ascii_lowercase();
    p == s || p.ends_with(&format!(".{s}"))
}

/// A setting number: the leading number of the text, whatever follows.
fn leading_number(text: &str) -> Option<f64> {
    split_number(text).map(|(n, _)| n)
}

fn fuel_litres(text: &str, kg_per_l: Option<f64>) -> Option<f64> {
    let (n, unit) = split_number(text)?;
    match unit.to_ascii_lowercase().as_str() {
        "l" | "" => Some(n),
        "gal" => Some(n * 3.785_411),
        "kg" => kg_per_l.filter(|k| *k > 0.0).map(|k| n / k),
        _ => None,
    }
}

/// Which wheels a compound entry is for.
fn compound_side(path: &str) -> (bool, bool) {
    let lower = path.to_ascii_lowercase();
    let front = lower.contains("front");
    let rear = lower.contains("rear");
    match (front, rear) {
        (true, false) => (true, false),
        (false, true) => (false, true),
        _ => (true, true),
    }
}

/// The setup message for `snap`.
///
/// The typed fields are found by the last segment of the path, ignoring case, and the first
/// entry that matches wins; the rest go to `extra`.
pub fn to_setup(snap: &CarSetupSnapshot, reason: &str, at: &str, fuel_l: Option<f64>) -> Setup {
    let mut setup = Setup {
        taken_at: at.to_owned(),
        reason: reason.to_owned(),
        source: "telemetry".to_owned(),
        ..Setup::default()
    };
    let mut used = vec![false; snap.entries.len()];
    let mut own_fuel = None;
    for (i, (path, text)) in snap.entries.iter().enumerate() {
        let key = last_segment(path).to_ascii_lowercase();
        let mut take = false;
        match key.as_str() {
            "brakepressurebias" | "brakebias" | "frontbrakebias" => {
                take |= put(&mut setup.brake_bias_front_pct, leading_number(text));
            }
            "abssetting" | "abs" => take |= put(&mut setup.abs, leading_number(text)),
            "tractioncontrolsetting" | "tcsetting" | "tractioncontrol" => {
                take |= put(&mut setup.tc, leading_number(text));
            }
            "fuellevel" => take |= put(&mut own_fuel, fuel_litres(text, snap.fuel_kg_per_l)),
            "tiretype" | "tirecompound" | "compound" => {
                let (front, rear) = compound_side(path);
                if front && setup.compound_front.is_none() {
                    setup.compound_front = Some(text.clone());
                    take = true;
                }
                if rear && setup.compound_rear.is_none() {
                    setup.compound_rear = Some(text.clone());
                    take = true;
                }
            }
            k if k.starts_with("frontwing") => {
                take |= put(&mut setup.wing_front, leading_number(text))
            }
            k if k.starts_with("rearwing") => {
                take |= put(&mut setup.wing_rear, leading_number(text))
            }
            _ => {
                let slots = [
                    ("LeftFront.ColdPressure", &mut setup.pressure_fl_kpa),
                    ("RightFront.ColdPressure", &mut setup.pressure_fr_kpa),
                    ("LeftRear.ColdPressure", &mut setup.pressure_rl_kpa),
                    ("RightRear.ColdPressure", &mut setup.pressure_rr_kpa),
                ];
                for (suffix, slot) in slots {
                    if ends_with_path(path, suffix) {
                        take |= put(slot, pressure_kpa(text));
                    }
                }
            }
        }
        used[i] = take;
    }
    setup.fuel_l = fuel_l.filter(|f| f.is_finite()).or(own_fuel);
    for ((path, text), used) in snap.entries.iter().zip(used) {
        if !used {
            add_extra(&mut setup.extra, path, text);
        }
    }
    if let Some(fixed) = snap.fixed_setup {
        setup.extra.insert(
            "fixed_setup".to_owned(),
            ExtraEntry {
                label: "Fixed setup".to_owned(),
                value: ExtraValue::Bool(fixed),
                group: Some("Event".to_owned()),
                unit: None,
            },
        );
    }
    setup
}

/// Reduces a path segment to the characters an `extra` key allows.
fn key_segment(segment: &str) -> String {
    segment
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// "CrossWeight" becomes "Cross weight".
fn label_of(segment: &str) -> String {
    let mut out = String::with_capacity(segment.len() + 4);
    let mut prev_lower = false;
    for c in segment.chars() {
        if c.is_ascii_uppercase() && prev_lower {
            out.push(' ');
            out.push(c.to_ascii_lowercase());
        } else {
            out.push(c);
        }
        prev_lower = c.is_ascii_lowercase() || c.is_ascii_digit();
    }
    out
}

/// A number with a plain unit (`23.8 L`, `-7 clicks`, `50.0%`), else the text.
fn extra_value(text: &str) -> (ExtraValue, Option<String>) {
    if let Some((n, unit)) = split_number(text) {
        let plain = unit
            .chars()
            .all(|c| c.is_alphabetic() || matches!(c, '%' | '/' | '\u{b0}'));
        if plain {
            return (
                ExtraValue::Number(n),
                (!unit.is_empty()).then(|| unit.to_owned()),
            );
        }
    }
    (ExtraValue::Text(text.to_owned()), None)
}

fn add_extra(extra: &mut BTreeMap<String, ExtraEntry>, path: &str, text: &str) {
    let segments: Vec<&str> = path.split('.').collect();
    let mut key: String = segments
        .iter()
        .map(|s| key_segment(s))
        .collect::<Vec<_>>()
        .join(".");
    key.truncate(EXTRA_KEY_MAX);
    let base = key.clone();
    let mut n = 2;
    while extra.contains_key(&key) {
        let suffix = format!("-{n}");
        key = base.clone();
        key.truncate(EXTRA_KEY_MAX - suffix.len());
        key.push_str(&suffix);
        n += 1;
    }
    let (value, unit) = extra_value(text);
    extra.insert(
        key,
        ExtraEntry {
            label: label_of(segments.last().copied().unwrap_or(path)),
            value,
            group: segments.first().map(|s| (*s).to_owned()),
            unit,
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{TyreReadings as Built, YamlBuilder};
    use irsdk::worker::SessionDoc;

    fn snapshot(builder: &YamlBuilder) -> CarSetupSnapshot {
        let doc = SessionDoc::from_raw(1, builder.build().as_bytes());
        let setup = doc.info.car_setup.expect("setup");
        CarSetupSnapshot::from_value(&setup.0, Some(0.75))
    }

    fn with_readings() -> YamlBuilder {
        YamlBuilder::new().setup(Some(5), Some(Built::default()))
    }

    #[test]
    fn typed_fields_come_from_the_last_path_segment() {
        let snap = snapshot(&with_readings());
        let setup = snap.to_setup("session_start", "2026-11-05T10:00:00Z", None);
        assert_eq!(setup.source, "telemetry");
        assert_eq!(setup.reason, "session_start");
        assert_eq!(setup.taken_at, "2026-11-05T10:00:00Z");
        assert_eq!(setup.brake_bias_front_pct, Some(54.5));
        assert_eq!(setup.abs, Some(3.0));
        assert_eq!(setup.tc, Some(4.0));
        assert_eq!(setup.pressure_fl_kpa, Some(152.0));
        assert_eq!(setup.pressure_fr_kpa, Some(152.0));
        assert_eq!(setup.pressure_rl_kpa, Some(148.0));
        assert_eq!(setup.pressure_rr_kpa, Some(148.0));
        assert_eq!(setup.fuel_l, Some(80.0));
        assert_eq!(setup.compound_front.as_deref(), Some("Dry"));
        assert_eq!(setup.compound_rear.as_deref(), Some("Dry"));
    }

    #[test]
    fn the_live_fuel_replaces_the_setup_fuel() {
        let snap = snapshot(&with_readings());
        let setup = snap.to_setup("pit_exit", "t", Some(31.5));
        assert_eq!(setup.fuel_l, Some(31.5));
    }

    #[test]
    fn other_settings_go_to_extra_with_label_group_and_unit() {
        let snap = snapshot(&with_readings());
        let setup = snap.to_setup("session_start", "t", None);
        let arb = &setup.extra["Chassis.Front.ArbSetting"];
        assert_eq!(arb.label, "Arb setting");
        assert_eq!(arb.group.as_deref(), Some("Chassis"));
        assert_eq!(arb.value, ExtraValue::Number(3.0));
        // A typed field is not repeated in `extra`.
        assert!(!setup.extra.contains_key("Chassis.Front.BrakePressureBias"));
        assert!(!setup.extra.contains_key("Systems.Fuel.FuelLevel"));
        assert!(!setup.extra.contains_key("UpdateCount"));
        assert!(setup.extra.keys().all(|k| k.len() <= 64));
    }

    #[test]
    fn readings_are_never_in_the_setup() {
        let snap = snapshot(&with_readings());
        let setup = snap.to_setup("session_start", "t", None);
        let reading = |k: &String| {
            let k = k.to_ascii_lowercase();
            k.contains("lasthot") || k.contains("lasttemps") || k.contains("treadremaining")
        };
        assert!(!setup.extra.keys().any(reading), "{:?}", setup.extra.keys());
        assert!(!snap.entries.iter().any(|(p, _)| reading(p)));
        assert!(!snap.readings.is_empty());
        assert_eq!(snap.readings.at_update, Some(5));
        assert_eq!(snap.update_count, Some(5));
    }

    #[test]
    fn omi_temperatures_are_outer_first_and_imo_are_reversed() {
        let value: Value = serde_json::json!({
            "UpdateCount": 3,
            "Suspension": {
                "LeftFront": {
                    "LastHotPressure": "172.0 kPa",
                    "LastTempsOMI": "70C, 80C, 90C",
                    "TreadRemaining": "97%, 96%, 95%",
                    "CornerWeight": "1200 N"
                },
                "RightFront": { "LastTempsIMO": "70C, 80C, 90C", "LastHotPressure": "25.0 psi" }
            }
        });
        let snap = CarSetupSnapshot::from_value(&value, None);
        let lf = snap.readings.wheels[0];
        assert_eq!(lf.temps_c, Some([70.0, 80.0, 90.0]));
        assert_eq!(lf.hot_pressure_kpa, Some(172.0));
        assert_eq!(lf.tread_remaining, Some([97.0, 96.0, 95.0]));
        let rf = snap.readings.wheels[1];
        assert_eq!(rf.temps_c, Some([90.0, 80.0, 70.0]));
        assert!((rf.hot_pressure_kpa.unwrap() - 172.368).abs() < 0.01);
        assert_eq!(snap.readings.wheels[2], WheelReadings::default());
        assert!(snap.entries.is_empty(), "{:?}", snap.entries);
    }

    #[test]
    fn a_setup_without_readings_has_empty_readings() {
        let snap = snapshot(&YamlBuilder::new());
        assert!(snap.readings.is_empty());
    }

    #[test]
    fn units_are_converted_to_kpa_litres_and_the_compound_goes_by_side() {
        let value = serde_json::json!({
            "UpdateCount": 1,
            "Tires": {
                "FrontTire": { "TireType": "Soft" },
                "RearTire": { "TireType": "Medium" },
                "LeftFront": { "ColdPressure": "25.0 psi" },
                "RightFront": { "ColdPressure": "1.8 bar" },
            },
            "Aero": { "FrontWingAngle": "4.5 deg", "RearWingAngle": 7 },
            "Fuel": { "FuelLevel": "60 kg" },
            "Chassis": { "BrakeBias": "56.0", "TractionControl": "off" }
        });
        let snap = CarSetupSnapshot::from_value(&value, Some(0.75));
        let setup = snap.to_setup("pit_exit", "t", None);
        assert_eq!(setup.compound_front.as_deref(), Some("Soft"));
        assert_eq!(setup.compound_rear.as_deref(), Some("Medium"));
        assert!((setup.pressure_fl_kpa.unwrap() - 172.37).abs() < 0.01);
        assert_eq!(setup.pressure_fr_kpa, Some(180.0));
        assert_eq!(setup.wing_front, Some(4.5));
        assert_eq!(setup.wing_rear, Some(7.0));
        assert_eq!(setup.fuel_l, Some(80.0));
        assert_eq!(setup.brake_bias_front_pct, Some(56.0));
        // A traction control value that is not a number stays in `extra` as text.
        assert_eq!(
            setup.extra["Chassis.TractionControl"].value,
            ExtraValue::Text("off".to_owned())
        );
        assert_eq!(setup.tc, None);
    }

    #[test]
    fn fixed_setup_is_an_extra_and_keys_are_reduced_and_unique() {
        let value = serde_json::json!({
            "UpdateCount": 1,
            "A b": { "C!d": "1", "C?d": "2" },
        });
        let mut snap = CarSetupSnapshot::from_value(&value, None);
        snap.fixed_setup = Some(true);
        let setup = snap.to_setup("session_start", "t", None);
        assert_eq!(setup.extra["fixed_setup"].value, ExtraValue::Bool(true));
        assert!(setup.extra.contains_key("A_b.C_d"));
        assert!(setup.extra.contains_key("A_b.C_d-2"));
    }

    #[test]
    fn long_keys_are_cut_to_the_limit() {
        let long = "x".repeat(100);
        let value = serde_json::json!({ "UpdateCount": 1, "G": { long: "5 L" } });
        let setup = CarSetupSnapshot::from_value(&value, None).to_setup("session_start", "t", None);
        let key = setup.extra.keys().find(|k| k.starts_with("G.")).unwrap();
        assert_eq!(key.len(), 64);
    }
}
