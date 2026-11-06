//! The SDK variables the recorder reads, resolved once by name into typed handles.
//!
//! `variables!` holds the one list of variables. [`Vars`] (here) and [`crate::tick::Tick`] are
//! both generated from it, so a variable is declared once.

use irsdk::defs::VarType;
use irsdk::vars::{ArrayVar, Var, VarError, VarTable, VarValue};

/// Number of car slots the per-car arrays are read for.
pub const CARS: usize = 64;

/// Why the variables could not be resolved.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VarsError {
    /// A required variable is not in the table.
    Missing(&'static str),
    /// A required variable has a type that cannot be read as the expected one.
    WrongType(&'static str, VarType),
}

impl std::fmt::Display for VarsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            VarsError::Missing(name) => write!(f, "required variable {name} is missing"),
            VarsError::WrongType(name, ty) => {
                write!(f, "required variable {name} has the unexpected type {ty:?}")
            }
        }
    }
}

impl std::error::Error for VarsError {}

/// The list of variables, passed to the macro named `$m`.
///
/// `req` and `opt` are scalars, `req_arr` and `opt_arr` per-car arrays of [`CARS`] elements.
/// A required variable that is absent stops the recorder; an optional one reads as absent.
macro_rules! variables {
    ($m:ident) => {
        $m! {
            req: [
                session_time: f64 = "SessionTime",
                session_num: i32 = "SessionNum",
                session_unique_id: i32 = "SessionUniqueID",
                session_state: i32 = "SessionState",
                session_flags: u32 = "SessionFlags",
                // Values in the garage screen, the stall, spectating and replays are not
                // observed yet: a car is driving when on track, on track car and not in a replay.
                is_on_track: bool = "IsOnTrack",
                is_on_track_car: bool = "IsOnTrackCar",
                is_in_garage: bool = "IsInGarage",
                is_replay: bool = "IsReplayPlaying",
                player_idx: i32 = "PlayerCarIdx",
                // Whether `Lap` rises on the out-lap crossing and in the pit lane is not observed
                // yet: a recorder should treat either counter as a crossing.
                lap: i32 = "Lap",
                lap_completed: i32 = "LapCompleted",
                lap_dist: f32 = "LapDist",
                lap_dist_pct: f32 = "LapDistPct",
                lap_last_time: f32 = "LapLastLapTime",
                speed: f32 = "Speed",
                rpm: f32 = "RPM",
                gear: i32 = "Gear",
                steer_rad: f32 = "SteeringWheelAngle",
                lat_accel: f32 = "LatAccel",
                // Observed: lateral acceleration and the steering angle are above 0 to the left, and
                // braking gives a negative longitudinal acceleration (docs/FINDINGS.md, Signs and scales).
                long_accel: f32 = "LongAccel",
                fuel_l: f32 = "FuelLevel",
                on_pit_road: bool = "OnPitRoad",
                track_loc: i32 = "PlayerTrackSurface",
            ],
            opt: [
                time_remain: f64 = "SessionTimeRemain",
                laps_remain: i32 = "SessionLapsRemainEx",
                time_total: f64 = "SessionTimeTotal",
                laps_total: i32 = "SessionLapsTotal",
                cam_idx: i32 = "CamCarIdx",
                lap_current_time: f32 = "LapCurrentLapTime",
                lap_best_time: f32 = "LapBestLapTime",
                steer_max_rad: f32 = "SteeringWheelAngleMax",
                vert_accel: f32 = "VertAccel",
                vel_x: f32 = "VelocityX",
                vel_y: f32 = "VelocityY",
                yaw: f32 = "Yaw",
                abs_active: bool = "BrakeABSactive",
                in_pit_stall: bool = "PlayerCarInPitStall",
                // Observed: true with the garage screen open, also when `IsInGarage` is false.
                is_garage_visible: bool = "IsGarageVisible",
                // Observed: 1 while the player is out of the car (the live replay), more in a
                // real replay.
                replay_frame_end: i32 = "ReplayFrameNumEnd",
                // Observed: starts on the row the car enters its stall.
                pit_service_active: bool = "PitstopActive",
                // Observed: 1 while the service runs, 2 when it is complete, then 0.
                pit_sv_status: i32 = "PlayerCarPitSvStatus",
                // Which material shows for a wheel on a kerb or on grass is not observed yet.
                surface: i32 = "PlayerTrackSurfaceMaterial",
                incidents: i32 = "PlayerCarMyIncidentCount",
                // That this word names the cause of an incident is not observed yet.
                incident_flags: u32 = "PlayerIncidents",
                tow_time: f32 = "PlayerCarTowTime",
                position: i32 = "PlayerCarPosition",
                class_position: i32 = "PlayerCarClassPosition",
                // Whether the pits are reported closed under a caution is not observed yet.
                pits_open: bool = "PitsOpen",
                // The numbers of the pace modes are not observed yet.
                pace_mode: i32 = "PaceMode",
                car_left_right: i32 = "CarLeftRight",
                air_c: f32 = "AirTemp",
                track_c: f32 = "TrackTempCrew",
                // The scale of precipitation (a fraction or a percent) is not observed yet.
                precip: f32 = "Precipitation",
                wind_ms: f32 = "WindVel",
            ],
            req_arr: [
                car_lap: i32 = "CarIdxLap",
                car_lap_completed: i32 = "CarIdxLapCompleted",
                car_pct: f32 = "CarIdxLapDistPct",
                car_on_pit_road: bool = "CarIdxOnPitRoad",
                car_loc: i32 = "CarIdxTrackSurface",
            ],
            opt_arr: [
                // Whether the positions are filled outside races is not observed yet.
                car_position: i32 = "CarIdxPosition",
                car_class_position: i32 = "CarIdxClassPosition",
                car_last: f32 = "CarIdxLastLapTime",
                car_best: f32 = "CarIdxBestLapTime",
                // The meaning of the estimated and gap times for other cars is not observed yet.
                car_est: f32 = "CarIdxEstTime",
                car_f2: f32 = "CarIdxF2Time",
                car_flags: u32 = "CarIdxSessionFlags",
                car_class: i32 = "CarIdxClass",
            ],
        }
    };
}
pub(crate) use variables;

/// Tyre temperature variable names: wheels in the order left front, right front, left rear,
/// right rear; positions left, middle, right of the carcass.
pub(crate) const TYRE_TEMP: [[&str; 3]; 4] = [
    ["LFtempCL", "LFtempCM", "LFtempCR"],
    ["RFtempCL", "RFtempCM", "RFtempCR"],
    ["LRtempCL", "LRtempCM", "LRtempCR"],
    ["RRtempCL", "RRtempCM", "RRtempCR"],
];

/// Tyre wear variable names, in the order of [`TYRE_TEMP`].
pub(crate) const TYRE_WEAR: [[&str; 3]; 4] = [
    ["LFwearL", "LFwearM", "LFwearR"],
    ["RFwearL", "RFwearM", "RFwearR"],
    ["LRwearL", "LRwearM", "LRwearR"],
    ["RRwearL", "RRwearM", "RRwearR"],
];

fn bad(table: &VarTable, name: &'static str, e: VarError) -> VarsError {
    match e {
        VarError::Missing => VarsError::Missing(name),
        VarError::WrongType { found } => VarsError::WrongType(name, found),
        VarError::NotScalar { .. } => {
            VarsError::WrongType(name, table.get(name).map_or(VarType::Char, |h| h.ty))
        }
    }
}

fn req<T: VarValue>(table: &VarTable, name: &'static str) -> Result<Var<T>, VarsError> {
    table.find(name).map_err(|e| bad(table, name, e))
}

fn req_arr<T: VarValue>(table: &VarTable, name: &'static str) -> Result<ArrayVar<T>, VarsError> {
    table.find_array(name).map_err(|e| bad(table, name, e))
}

fn opt<T: VarValue>(
    table: &VarTable,
    name: &'static str,
    missing: &mut Vec<&'static str>,
) -> Option<Var<T>> {
    let found = table.find(name).ok();
    if found.is_none() {
        missing.push(name);
    }
    found
}

fn opt_arr<T: VarValue>(
    table: &VarTable,
    name: &'static str,
    missing: &mut Vec<&'static str>,
) -> Option<ArrayVar<T>> {
    let found = table.find_array(name).ok();
    if found.is_none() {
        missing.push(name);
    }
    found
}

/// The first of two variables that the table has (the raw pedal, else the processed one).
fn either(
    table: &VarTable,
    first: &'static str,
    second: &'static str,
) -> Result<Var<f32>, VarsError> {
    match table.find(first) {
        Ok(v) => Ok(v),
        Err(VarError::Missing) => req(table, second),
        Err(e) => Err(bad(table, first, e)),
    }
}

/// Twelve tyre variables, or `None` when any of them is absent (every absent name is listed).
fn tyres(
    table: &VarTable,
    names: &[[&'static str; 3]; 4],
    missing: &mut Vec<&'static str>,
) -> Option<[[Var<f32>; 3]; 4]> {
    let found: Vec<Option<Var<f32>>> = names
        .iter()
        .flatten()
        .map(|name| table.find::<f32>(name).ok())
        .collect();
    if found.iter().any(Option::is_none) {
        for (name, v) in names.iter().flatten().zip(&found) {
            if v.is_none() {
                missing.push(name);
            }
        }
        return None;
    }
    Some(std::array::from_fn(|w| {
        std::array::from_fn(|p| found[w * 3 + p].expect("checked above"))
    }))
}

macro_rules! define_vars {
    (
        req: [$($rf:ident : $rt:ty = $rn:literal,)*],
        opt: [$($of:ident : $ot:ty = $on:literal,)*],
        req_arr: [$($raf:ident : $rat:ty = $ran:literal,)*],
        opt_arr: [$($oaf:ident : $oat:ty = $oan:literal,)*],
    ) => {
        /// Typed handles on every variable the recorder reads, for one variable table.
        ///
        /// Resolve it again whenever the table changes (an attach, or a reset of the sampler).
        #[derive(Clone, Debug)]
        pub struct Vars {
            $(pub(crate) $rf: Var<$rt>,)*
            $(pub(crate) $of: Option<Var<$ot>>,)*
            $(pub(crate) $raf: ArrayVar<$rat>,)*
            $(pub(crate) $oaf: Option<ArrayVar<$oat>>,)*
            pub(crate) throttle: Var<f32>,
            pub(crate) brake: Var<f32>,
            pub(crate) clutch: Option<Var<f32>>,
            pub(crate) tyre_temp: Option<[[Var<f32>; 3]; 4]>,
            pub(crate) tyre_wear: Option<[[Var<f32>; 3]; 4]>,
            missing: Vec<&'static str>,
        }

        impl Vars {
            /// Resolves the variables of `table`. A required variable that is absent or has
            /// another type is an error; optional ones that are absent are listed by
            /// [`Vars::missing_optional`] and read as absent.
            pub fn resolve(table: &VarTable) -> Result<Vars, VarsError> {
                let mut missing = Vec::new();
                $(let $rf = req(table, $rn)?;)*
                $(let $raf = req_arr(table, $ran)?;)*
                let throttle = either(table, "ThrottleRaw", "Throttle")?;
                let brake = either(table, "BrakeRaw", "Brake")?;
                let clutch = either(table, "ClutchRaw", "Clutch").ok();
                if clutch.is_none() {
                    missing.push("ClutchRaw/Clutch");
                }
                $(let $of = opt(table, $on, &mut missing);)*
                $(let $oaf = opt_arr(table, $oan, &mut missing);)*
                let tyre_temp = tyres(table, &TYRE_TEMP, &mut missing);
                let tyre_wear = tyres(table, &TYRE_WEAR, &mut missing);
                Ok(Vars {
                    $($rf,)* $($of,)* $($raf,)* $($oaf,)*
                    throttle, brake, clutch, tyre_temp, tyre_wear, missing,
                })
            }
        }
    };
}
variables!(define_vars);

impl Vars {
    /// The optional variables that were absent, by SDK name, to log once per attach.
    pub fn missing_optional(&self) -> impl Iterator<Item = &'static str> + '_ {
        self.missing.iter().copied()
    }
}

#[cfg(all(test, feature = "testing"))]
mod tests {
    use super::*;
    use crate::testing::{full_layout, table_of};
    use irsdk::testing::{Layout, LayoutBuilder};

    fn without(names: &[&str]) -> Layout {
        let mut b = LayoutBuilder::new();
        for v in full_layout()
            .vars()
            .iter()
            .filter(|v| !names.contains(&v.name.as_str()))
        {
            b = b.var(&v.name, v.ty, v.count, &v.unit, &v.desc);
        }
        b.build()
    }

    fn retyped(name: &str, ty: VarType) -> Layout {
        let mut b = LayoutBuilder::new();
        for v in full_layout().vars() {
            let t = if v.name == name { ty } else { v.ty };
            b = b.var(&v.name, t, v.count, &v.unit, &v.desc);
        }
        b.build()
    }

    #[test]
    fn full_layout_resolves_with_nothing_missing() {
        let vars = Vars::resolve(&table_of(&full_layout())).expect("resolve");
        assert_eq!(vars.missing_optional().count(), 0);
        assert!(vars.tyre_temp.is_some() && vars.tyre_wear.is_some() && vars.clutch.is_some());
    }

    #[test]
    fn a_missing_required_variable_is_named() {
        let err = Vars::resolve(&table_of(&without(&["LapDistPct"]))).unwrap_err();
        assert_eq!(err, VarsError::Missing("LapDistPct"));
        let err = Vars::resolve(&table_of(&without(&["CarIdxLap"]))).unwrap_err();
        assert_eq!(err, VarsError::Missing("CarIdxLap"));
        assert!(err.to_string().contains("CarIdxLap"));
    }

    #[test]
    fn a_required_variable_of_another_type_is_an_error() {
        let err = Vars::resolve(&table_of(&retyped("Speed", VarType::Int))).unwrap_err();
        assert_eq!(err, VarsError::WrongType("Speed", VarType::Int));
    }

    #[test]
    fn optional_variables_that_are_absent_are_listed() {
        let table = table_of(&without(&[
            "PitsOpen",
            "CarIdxEstTime",
            "LFwearM",
            "ClutchRaw",
            "Clutch",
        ]));
        let vars = Vars::resolve(&table).expect("resolve");
        let missing: Vec<_> = vars.missing_optional().collect();
        for name in ["PitsOpen", "CarIdxEstTime", "LFwearM", "ClutchRaw/Clutch"] {
            assert!(missing.contains(&name), "{name} not listed in {missing:?}");
        }
        assert!(vars.pits_open.is_none() && vars.car_est.is_none() && vars.clutch.is_none());
        assert!(vars.tyre_wear.is_none() && vars.tyre_temp.is_some());
    }

    #[test]
    fn the_raw_pedal_is_preferred_and_the_processed_one_is_the_fallback() {
        let only_processed = table_of(&without(&["ThrottleRaw", "BrakeRaw"]));
        assert!(Vars::resolve(&only_processed).is_ok());
        let none = table_of(&without(&["ThrottleRaw", "Throttle"]));
        assert_eq!(
            Vars::resolve(&none).unwrap_err(),
            VarsError::Missing("Throttle")
        );
    }

    #[test]
    fn int_and_bit_field_variables_read_either_way() {
        let table = table_of(&retyped("SessionFlags", VarType::Int));
        assert!(Vars::resolve(&table).is_ok());
        let table = table_of(&retyped("CarLeftRight", VarType::BitField));
        let vars = Vars::resolve(&table).expect("resolve");
        assert!(vars.car_left_right.is_some());
    }
}
