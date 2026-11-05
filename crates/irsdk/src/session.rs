//! The partial typed model of the session text.
//!
//! Only the fields the connector reads are modelled; unknown keys are ignored. Every field is an
//! `Option` named by its exact iRacing key. Booleans are written as 0/1 integers and are typed
//! `i64`; numbers with units stay `String` with the helpers of [`crate::yaml::units`].

use serde::Deserialize;

use crate::yaml::units;

/// Declares a struct of optional fields, each renamed to its iRacing key.
macro_rules! model {
    ($(#[$meta:meta])* $name:ident { $($field:ident : $ty:ty = $key:literal),* $(,)? }) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Default, PartialEq, Deserialize)]
        #[serde(default)]
        pub struct $name {
            $(
                #[doc = concat!("The `", $key, "` key.")]
                #[serde(rename = $key)]
                pub $field: Option<$ty>,
            )*
        }
    };
}

/// What was read from the session text: one entry per top-level section that parsed.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SessionInfo {
    /// The `WeekendInfo` section.
    pub weekend: Option<WeekendInfo>,
    /// The `SessionInfo` section (the list of sessions of the event).
    pub session: Option<SessionsInfo>,
    /// The `QualifyResultsInfo` section.
    pub qualify: Option<QualifyResultsInfo>,
    /// The `SplitTimeInfo` section.
    pub split: Option<SplitTimeInfo>,
    /// The `DriverInfo` section.
    pub driver: Option<DriverInfo>,
    /// The `CarSetup` section.
    pub car_setup: Option<CarSetup>,
}

model! {
    /// The `WeekendInfo` section: the track and the event.
    WeekendInfo {
        track_name: String = "TrackName",
        track_id: i64 = "TrackID",
        track_length: String = "TrackLength",
        track_display_name: String = "TrackDisplayName",
        track_config_name: String = "TrackConfigName",
        track_pit_speed_limit: String = "TrackPitSpeedLimit",
        track_num_turns: i64 = "TrackNumTurns",
        series_id: i64 = "SeriesID",
        session_id: i64 = "SessionID",
        sub_session_id: i64 = "SubSessionID",
        event_type: String = "EventType",
        category: String = "Category",
        sim_mode: String = "SimMode",
        official: i64 = "Official",
        team_racing: i64 = "TeamRacing",
        num_car_classes: i64 = "NumCarClasses",
        track_weather_type: String = "TrackWeatherType",
        weekend_options: WeekendOptions = "WeekendOptions",
        telemetry_options: TelemetryOptions = "TelemetryOptions",
    }
}

model! {
    /// The `WeekendOptions` block of `WeekendInfo`.
    WeekendOptions {
        incident_limit: String = "IncidentLimit",
        is_fixed_setup: i64 = "IsFixedSetup",
        standing_start: i64 = "StandingStart",
        fast_repairs_limit: String = "FastRepairsLimit",
    }
}

model! {
    /// The `TelemetryOptions` block of `WeekendInfo`.
    TelemetryOptions {
        telemetry_disk_file: String = "TelemetryDiskFile",
    }
}

impl WeekendInfo {
    /// The track length in metres, from text such as `"5.79 km"` or `"3.60 mi"`.
    pub fn track_length_m(&self) -> Option<f64> {
        units::length_m(self.track_length.as_deref()?)
    }

    /// The pit lane speed limit in km/h, from text such as `"80.00 kph"`.
    pub fn pit_speed_limit_kmh(&self) -> Option<f64> {
        units::speed_kmh(self.track_pit_speed_limit.as_deref()?)
    }

    /// The incident limit; `None` when it is `unlimited` or not written.
    pub fn incident_limit(&self) -> Option<u32> {
        let s = self.weekend_options.as_ref()?.incident_limit.as_deref()?;
        s.trim().parse().ok()
    }
}

model! {
    /// The `SessionInfo` section: the sessions of the event.
    SessionsInfo {
        current_session_num: i64 = "CurrentSessionNum",
        sessions: Vec<Session> = "Sessions",
    }
}

model! {
    /// One session of the event (practice, qualifying, race).
    Session {
        session_num: i64 = "SessionNum",
        session_laps: String = "SessionLaps",
        session_time: String = "SessionTime",
        session_type: String = "SessionType",
        session_name: String = "SessionName",
        results_positions: Vec<ResultPosition> = "ResultsPositions",
    }
}

model! {
    /// One line of a session's results.
    ResultPosition {
        position: i64 = "Position",
        class_position: i64 = "ClassPosition",
        car_idx: i64 = "CarIdx",
        lap: i64 = "Lap",
        time: f64 = "Time",
        fastest_lap: i64 = "FastestLap",
        fastest_time: f64 = "FastestTime",
        last_time: f64 = "LastTime",
        laps_led: i64 = "LapsLed",
        laps_complete: i64 = "LapsComplete",
        incidents: i64 = "Incidents",
        reason_out_id: i64 = "ReasonOutId",
        reason_out_str: String = "ReasonOutStr",
    }
}

impl SessionsInfo {
    /// The session `CurrentSessionNum` names, else the one whose `SessionNum` equals
    /// `telemetry_session_num` (the telemetry variable of the same name).
    pub fn current(&self, telemetry_session_num: Option<i64>) -> Option<&Session> {
        let by = |n: i64| {
            self.sessions
                .as_deref()?
                .iter()
                .find(|s| s.session_num == Some(n))
        };
        self.current_session_num
            .and_then(by)
            .or_else(|| telemetry_session_num.and_then(by))
    }
}

impl Session {
    /// The lap limit; `None` when the session has no lap limit (`unlimited`) or none is written.
    pub fn laps(&self) -> Option<u32> {
        self.session_laps.as_deref()?.trim().parse().ok()
    }

    /// The time limit in seconds, from text such as `"3600.0000 sec"`; `None` when `unlimited`.
    pub fn time_s(&self) -> Option<f64> {
        units::number_unit(self.session_time.as_deref()?).map(|(n, _)| n)
    }
}

model! {
    /// The `QualifyResultsInfo` section.
    QualifyResultsInfo {
        results: Vec<QualifyResult> = "Results",
    }
}

model! {
    /// One line of the qualifying results.
    QualifyResult {
        position: i64 = "Position",
        class_position: i64 = "ClassPosition",
        car_idx: i64 = "CarIdx",
        fastest_lap: i64 = "FastestLap",
        fastest_time: f64 = "FastestTime",
    }
}

model! {
    /// The `SplitTimeInfo` section.
    SplitTimeInfo {
        sectors: Vec<Sector> = "Sectors",
    }
}

model! {
    /// One sector boundary.
    Sector {
        sector_num: i64 = "SectorNum",
        sector_start_pct: f64 = "SectorStartPct",
    }
}

model! {
    /// The `DriverInfo` section: the player's car and every car on the grid.
    DriverInfo {
        driver_car_idx: i64 = "DriverCarIdx",
        pace_car_idx: i64 = "PaceCarIdx",
        driver_car_fuel_max_ltr: f64 = "DriverCarFuelMaxLtr",
        driver_car_fuel_kg_per_ltr: f64 = "DriverCarFuelKgPerLtr",
        driver_car_max_fuel_pct: f64 = "DriverCarMaxFuelPct",
        driver_car_red_line: f64 = "DriverCarRedLine",
        driver_car_est_lap_time: f64 = "DriverCarEstLapTime",
        driver_setup_name: String = "DriverSetupName",
        driver_setup_is_modified: i64 = "DriverSetupIsModified",
        driver_incident_count: i64 = "DriverIncidentCount",
        drivers: Vec<Driver> = "Drivers",
    }
}

model! {
    /// One car of the grid.
    Driver {
        car_idx: i64 = "CarIdx",
        user_name: String = "UserName",
        abbrev_name: String = "AbbrevName",
        initials: String = "Initials",
        user_id: i64 = "UserID",
        team_id: i64 = "TeamID",
        team_name: String = "TeamName",
        car_number: String = "CarNumber",
        car_number_raw: i64 = "CarNumberRaw",
        car_id: i64 = "CarID",
        car_path: String = "CarPath",
        car_screen_name: String = "CarScreenName",
        car_class_id: i64 = "CarClassID",
        car_class_short_name: String = "CarClassShortName",
        car_class_est_lap_time: f64 = "CarClassEstLapTime",
        car_is_pace_car: i64 = "CarIsPaceCar",
        car_is_ai: i64 = "CarIsAI",
        is_spectator: i64 = "IsSpectator",
        cur_driver_incident_count: i64 = "CurDriverIncidentCount",
        team_incident_count: i64 = "TeamIncidentCount",
    }
}

impl DriverInfo {
    /// The player's entry, by `DriverCarIdx`.
    pub fn player(&self) -> Option<&Driver> {
        let idx = self.driver_car_idx?;
        self.drivers
            .as_deref()?
            .iter()
            .find(|d| d.car_idx == Some(idx))
    }

    /// The fuel tank capacity in litres: `DriverCarFuelMaxLtr` times `DriverCarMaxFuelPct`.
    pub fn fuel_capacity_l(&self) -> Option<f64> {
        Some(self.driver_car_fuel_max_ltr? * self.driver_car_max_fuel_pct?)
    }
}

impl Driver {
    /// True for a real competitor: not the pace car, not a spectator and with a user name.
    pub fn is_real(&self) -> bool {
        self.car_is_pace_car.unwrap_or(0) == 0
            && self.is_spectator.unwrap_or(0) == 0
            && self.user_name.as_deref().is_some_and(|n| !n.is_empty())
    }
}

/// The `CarSetup` section, untyped: its keys depend on the car.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(transparent)]
pub struct CarSetup(pub serde_yaml_ng::Value);

impl CarSetup {
    /// The `UpdateCount` of the setup, which grows when the driver changes it.
    pub fn update_count(&self) -> Option<i64> {
        self.0.get("UpdateCount")?.as_i64()
    }
}
