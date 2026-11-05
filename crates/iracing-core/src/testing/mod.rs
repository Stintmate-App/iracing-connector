//! Synthetic data for tests (feature `testing`): the full variable layout, session text
//! builders and scripted scenarios.
//!
//! The builders panic on misuse: they are test tools.

mod scenario;
mod yaml;

pub use scenario::{LapProfile, Scenario, Side, YamlUpdate};
pub use yaml::{ResultSpec, SessionSpec, TyreReadings, YamlBuilder};

use irsdk::defs::VarType::{self, BitField, Bool, Double, Float, Int};
use irsdk::header::Header;
use irsdk::testing::{Layout, LayoutBuilder, live_image};
use irsdk::vars::VarTable;

use crate::vars::CARS;

const TYRE_POSITIONS: [&str; 3] = ["L", "M", "R"];
const WHEELS: [&str; 4] = ["LF", "RF", "LR", "RR"];

/// Every scalar variable of the recorder: name, type, unit.
const SCALARS: &[(&str, VarType, &str)] = &[
    ("SessionTime", Double, "s"),
    ("SessionNum", Int, ""),
    ("SessionUniqueID", Int, ""),
    ("SessionState", Int, "irsdk_SessionState"),
    ("SessionFlags", BitField, "irsdk_Flags"),
    ("SessionTimeRemain", Double, "s"),
    ("SessionLapsRemainEx", Int, ""),
    ("SessionTimeTotal", Double, "s"),
    ("SessionLapsTotal", Int, ""),
    ("IsOnTrack", Bool, ""),
    ("IsOnTrackCar", Bool, ""),
    ("IsInGarage", Bool, ""),
    ("IsReplayPlaying", Bool, ""),
    ("PlayerCarIdx", Int, ""),
    ("CamCarIdx", Int, ""),
    ("Lap", Int, ""),
    ("LapCompleted", Int, ""),
    ("LapDist", Float, "m"),
    ("LapDistPct", Float, "%"),
    ("LapCurrentLapTime", Float, "s"),
    ("LapLastLapTime", Float, "s"),
    ("LapBestLapTime", Float, "s"),
    ("Speed", Float, "m/s"),
    ("RPM", Float, "revs/min"),
    ("Gear", Int, ""),
    ("ThrottleRaw", Float, "%"),
    ("Throttle", Float, "%"),
    ("BrakeRaw", Float, "%"),
    ("Brake", Float, "%"),
    ("ClutchRaw", Float, "%"),
    ("Clutch", Float, "%"),
    ("SteeringWheelAngle", Float, "rad"),
    ("SteeringWheelAngleMax", Float, "rad"),
    ("LatAccel", Float, "m/s^2"),
    ("LongAccel", Float, "m/s^2"),
    ("VertAccel", Float, "m/s^2"),
    ("VelocityX", Float, "m/s"),
    ("VelocityY", Float, "m/s"),
    ("Yaw", Float, "rad"),
    ("FuelLevel", Float, "l"),
    ("BrakeABSactive", Bool, ""),
    ("OnPitRoad", Bool, ""),
    ("PlayerCarInPitStall", Bool, ""),
    ("PlayerTrackSurface", Int, "irsdk_TrkLoc"),
    ("PlayerTrackSurfaceMaterial", Int, "irsdk_TrkSurf"),
    ("PlayerCarMyIncidentCount", Int, ""),
    ("PlayerIncidents", BitField, "irsdk_IncidentFlags"),
    ("PlayerCarTowTime", Float, "s"),
    ("PlayerCarPosition", Int, ""),
    ("PlayerCarClassPosition", Int, ""),
    ("PitsOpen", Bool, ""),
    ("PaceMode", Int, "irsdk_PaceMode"),
    ("CarLeftRight", Int, "irsdk_CarLeftRight"),
    ("AirTemp", Float, "C"),
    ("TrackTempCrew", Float, "C"),
    ("Precipitation", Float, "%"),
    ("WindVel", Float, "m/s"),
];

/// Every per-car variable ([`CARS`] elements): name, type, unit.
const PER_CAR: &[(&str, VarType, &str)] = &[
    ("CarIdxLap", Int, ""),
    ("CarIdxLapCompleted", Int, ""),
    ("CarIdxLapDistPct", Float, "%"),
    ("CarIdxOnPitRoad", Bool, ""),
    ("CarIdxTrackSurface", Int, "irsdk_TrkLoc"),
    ("CarIdxPosition", Int, ""),
    ("CarIdxClassPosition", Int, ""),
    ("CarIdxLastLapTime", Float, "s"),
    ("CarIdxBestLapTime", Float, "s"),
    ("CarIdxEstTime", Float, "s"),
    ("CarIdxF2Time", Float, "s"),
    ("CarIdxSessionFlags", BitField, "irsdk_Flags"),
    ("CarIdxClass", Int, ""),
];

/// A layout with every variable the recorder reads, at the SDK's types, 60 ticks per second and
/// three buffers. Per-car variables have 64 elements.
pub fn full_layout() -> Layout {
    let mut b = LayoutBuilder::new().tick_rate(60).num_buf(3);
    for &(name, ty, unit) in SCALARS {
        b = b.var(name, ty, 1, unit, name);
    }
    for kind in ["temp", "wear"] {
        for wheel in WHEELS {
            for pos in TYRE_POSITIONS {
                let name = match kind {
                    "temp" => format!("{wheel}tempC{pos}"),
                    _ => format!("{wheel}wear{pos}"),
                };
                b = b.var(
                    &name,
                    Float,
                    1,
                    if kind == "temp" { "C" } else { "%" },
                    &name,
                );
            }
        }
    }
    for &(name, ty, unit) in PER_CAR {
        b = b.var(name, ty, CARS, unit, name);
    }
    b.build()
}

/// The variable table `layout` would have in a file or in the live map.
pub fn table_of(layout: &Layout) -> VarTable {
    let image = live_image(layout, "", &[]);
    let header = Header::parse(&image).expect("header of a synthetic image");
    VarTable::parse(&image, &header).expect("table of a synthetic image")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_full_layout_has_every_variable_once() {
        let layout = full_layout();
        let names: std::collections::HashSet<_> =
            layout.vars().iter().map(|v| v.name.as_str()).collect();
        assert_eq!(names.len(), layout.vars().len(), "duplicate names");
        assert_eq!(layout.vars().len(), SCALARS.len() + 24 + PER_CAR.len());
        assert_eq!(layout.var("CarIdxLap").expect("var").count, CARS);
        assert_eq!(layout.var("SessionTime").expect("var").ty, Double);
        assert_eq!((layout.tick_rate(), layout.num_buf()), (60, 3));
    }

    #[test]
    fn the_table_of_a_layout_lists_all_its_variables() {
        let layout = full_layout();
        let table = table_of(&layout);
        assert_eq!(table.len(), layout.vars().len());
        assert!(table.skipped().is_empty());
    }
}
