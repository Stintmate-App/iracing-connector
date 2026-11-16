//! The simulator data kinds the connector sends, and their descriptions for Pitwall.

use std::collections::BTreeMap;

use stintmate_connector_wire::messages::{SimDataField, SimDataKind};

/// The kind of the tyre readings iRacing gives only in the pits.
pub const STALL_TYRES: &str = "stall_tyres";

/// The kind of the tow and fast-repair state.
pub const REPAIR: &str = "repair";

fn field(meaning: &str, unit: Option<&str>) -> SimDataField {
    SimDataField {
        meaning: meaning.to_owned(),
        unit: unit.map(str::to_owned),
    }
}

/// The kinds this connector sends, described field by field.
pub fn kinds() -> Vec<SimDataKind> {
    let mut fields = BTreeMap::new();
    fields.insert(
        "occasion".to_owned(),
        field(
            "Why the record was written: pit_stop (the car drove into its pit box), reset (the simulator put it back in the pits) or car_entry (you got into the car: written at every entry, it marks the start of a stint and carries no wheels).",
            None,
        ),
    );
    fields.insert(
        "wheels".to_owned(),
        field(
            "One object per wheel: fl, fr, rl, rr (front left, front right, rear left, rear right); a wheel with no value in range is left out. Not in a car_entry record.",
            None,
        ),
    );
    for (name, meaning) in [
        (
            "carcass_in_c",
            "Carcass temperature at the edge towards the car's centreline.",
        ),
        (
            "carcass_mid_c",
            "Carcass temperature in the middle of the tread.",
        ),
        (
            "carcass_out_c",
            "Carcass temperature at the edge away from the car's centreline.",
        ),
    ] {
        fields.insert(format!("wheels.*.{name}"), field(meaning, Some("C")));
    }
    for (name, meaning) in [
        (
            "tread_in",
            "Tread remaining at the edge towards the car's centreline, 0 to 1, 1 = new.",
        ),
        (
            "tread_mid",
            "Tread remaining in the middle of the tread, 0 to 1, 1 = new.",
        ),
        (
            "tread_out",
            "Tread remaining at the edge away from the car's centreline, 0 to 1, 1 = new.",
        ),
    ] {
        fields.insert(format!("wheels.*.{name}"), field(meaning, Some("fraction")));
    }
    let mut repair_fields = BTreeMap::new();
    repair_fields.insert(
        "tow_time_s".to_owned(),
        field(
            "Seconds of tow left (PlayerCarTowTime): the full tow in the record of a tow start, counting down in the stall. It is the distance to the stall plus a base penalty; repairs start after it. Leaving the car while driving also starts a tow.",
            Some("s"),
        ),
    );
    repair_fields.insert(
        "mandatory_repair_needed".to_owned(),
        field(
            "EngineWarnings bit irsdk_mandRepNeeded: \"car needs mandatory repairs\".",
            None,
        ),
    );
    repair_fields.insert(
        "optional_repair_needed".to_owned(),
        field(
            "EngineWarnings bit irsdk_optRepNeeded: \"car needs optional repairs\".",
            None,
        ),
    );
    repair_fields.insert(
        "repair_left_s".to_owned(),
        field(
            "PitRepairLeft: \"Time left for mandatory pit repairs if repairs are active\". It counts down during a normal repair: in a recording it starts well above 0 when the repair begins and matches the game's pit screen to within about 30 s.",
            Some("s"),
        ),
    );
    repair_fields.insert(
        "optional_repair_left_s".to_owned(),
        field(
            "PitOptRepairLeft: \"Time left for optional repairs if repairs are active\". It counts down during a normal repair: in a recording it starts well above 0 when the repair begins and matches the game's pit screen to within about 30 s.",
            Some("s"),
        ),
    );
    repair_fields.insert(
        "fast_repairs_available".to_owned(),
        field("Fast repairs left; 255 = unlimited.", None),
    );
    repair_fields.insert(
        "fast_repairs_used".to_owned(),
        field("Fast repairs used by the player.", None),
    );
    let repair = SimDataKind {
        kind: REPAIR.to_owned(),
        description: "iRacing's tow and repair state. A record is written when a tow starts, when the car starts or stops needing mandatory or optional repairs, when a repair starts in the stall and when the count of fast repairs used changes. Every record carries every field the simulator published.".to_owned(),
        topic: None,
        fields: repair_fields,
    };
    let stall_tyres = SimDataKind {
        kind: STALL_TYRES.to_owned(),
        description: "Tyre temperatures and tread remaining iRacing gives only when the car comes into the pits (a pit stop or a reset). They are of the set that came in, just before it was changed: they describe the stint that ended, and the car leaves on new tyres. pit_stop and reset records are sent when the values differ from the last set sent. A car_entry record is written at every entry into the car, with no wheels: the start of a stint. Not live.".to_owned(),
        topic: Some("tyres".to_owned()),
        fields,
    };
    vec![stall_tyres, repair]
}
