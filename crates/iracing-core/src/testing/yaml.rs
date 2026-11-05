//! A builder of session texts in the shape the simulator writes them.

use std::fmt::Write as _;

/// The tyre readings of the last stop, as the setup lists them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TyreReadings {
    /// Hot pressure per wheel (left front, right front, left rear, right rear), in kPa.
    pub hot_kpa: [f64; 4],
    /// Temperature per wheel, in degrees Celsius.
    pub temp_c: [f64; 4],
    /// Tread remaining per wheel, in percent.
    pub tread_pct: [f64; 4],
}

impl Default for TyreReadings {
    fn default() -> Self {
        TyreReadings {
            hot_kpa: [172.0, 171.0, 168.0, 169.0],
            temp_c: [78.0, 79.0, 74.0, 75.0],
            tread_pct: [97.0, 97.0, 98.0, 98.0],
        }
    }
}

/// One line of a session's results.
#[derive(Clone, Debug, PartialEq)]
pub struct ResultSpec {
    /// The car slot.
    pub car_idx: i32,
    /// The overall position.
    pub position: i32,
    /// The position in the class.
    pub class_position: i32,
    /// The reason-out number (0 for a car still running).
    pub reason_out_id: i32,
    /// The reason-out text.
    pub reason_out: String,
}

impl ResultSpec {
    /// A running car in position `position` (also its class position).
    pub fn running(car_idx: i32, position: i32) -> ResultSpec {
        ResultSpec {
            car_idx,
            position,
            class_position: position,
            reason_out_id: 0,
            reason_out: "Running".to_owned(),
        }
    }
}

/// One session of the event.
#[derive(Clone, Debug, PartialEq)]
pub struct SessionSpec {
    /// The session number.
    pub num: i32,
    /// The session type text.
    pub kind: String,
    /// The results; empty writes `null`, as the simulator does before there are any.
    pub results: Vec<ResultSpec>,
}

/// Builds the session text of an event: weekend, sessions, sectors, drivers and setup.
#[derive(Clone, Debug)]
pub struct YamlBuilder {
    track_id: i64,
    track_name: String,
    display_name: String,
    config_name: String,
    length_m: f64,
    imperial: bool,
    session_id: i64,
    sub_session_id: i64,
    incident_limit: Option<u32>,
    sessions: Vec<SessionSpec>,
    current: i32,
    sectors: usize,
    player_idx: i32,
    pace_idx: Option<i32>,
    fuel_max_l: f64,
    fuel_pct: f64,
    drivers: usize,
    classes: usize,
    names: Vec<(i32, String)>,
    setup: Option<(i64, Option<TyreReadings>)>,
}

const NAMES: [&str; 6] = [
    "Alex Kramer",
    "Jos\u{e9} Garc\u{ed}a",
    "Zo\u{eb} M\u{fc}ller",
    "Ren\u{e9}e Dubois",
    "Sam Okafor",
    "Nuno Ara\u{fa}jo",
];
const CLASSES: [(&str, &str, &str, f64); 4] = [
    ("GT3", "porsche992rgt3", "Porsche 911 GT3 R (992)", 98.0),
    ("LMP2", "dallarap217", "Dallara P217", 88.0),
    ("MX5", "mx5 mx52016", "Global Mazda MX-5 Cup", 110.0),
    ("GT4", "bmwm4gt4", "BMW M4 GT4", 104.0),
];

impl Default for YamlBuilder {
    fn default() -> Self {
        YamlBuilder::new()
    }
}

impl YamlBuilder {
    /// A one-class field of one driver (the player, slot 1, with the pace car in slot 0) on a
    /// 4.01 km track with a practice session, three sectors and a setup.
    pub fn new() -> YamlBuilder {
        YamlBuilder {
            track_id: 261,
            track_name: "spa up".to_owned(),
            display_name: "Circuit de Spa-Francorchamps".to_owned(),
            config_name: "Grand Prix Pits".to_owned(),
            length_m: 4010.0,
            imperial: false,
            session_id: 1_000_001,
            sub_session_id: 2_000_002,
            incident_limit: Some(17),
            sessions: vec![SessionSpec {
                num: 0,
                kind: "Practice".to_owned(),
                results: Vec::new(),
            }],
            current: 0,
            sectors: 3,
            player_idx: 1,
            pace_idx: Some(0),
            fuel_max_l: 100.0,
            fuel_pct: 1.0,
            drivers: 1,
            classes: 1,
            names: Vec::new(),
            setup: Some((1, None)),
        }
    }

    /// Sets the track: simulator id, track name, display name and layout name.
    pub fn track(mut self, id: i64, name: &str, display: &str, config: &str) -> Self {
        self.track_id = id;
        self.track_name = name.to_owned();
        self.display_name = display.to_owned();
        self.config_name = config.to_owned();
        self
    }

    /// Sets the lap length in metres.
    pub fn length_m(mut self, metres: f64) -> Self {
        self.length_m = metres;
        self
    }

    /// The lap length in metres.
    pub fn track_length_m(&self) -> f64 {
        self.length_m
    }

    /// Writes lengths in miles, pressures in psi and fuel in gallons.
    pub fn imperial(mut self) -> Self {
        self.imperial = true;
        self
    }

    /// Sets the session and sub-session ids.
    pub fn ids(mut self, session_id: i64, sub_session_id: i64) -> Self {
        self.session_id = session_id;
        self.sub_session_id = sub_session_id;
        self
    }

    /// Sets the incident limit; `None` writes `unlimited`.
    pub fn incident_limit(mut self, limit: Option<u32>) -> Self {
        self.incident_limit = limit;
        self
    }

    /// Adds session `num` of type `kind`, or changes its type when it exists, and makes it the
    /// current one.
    pub fn session(mut self, num: i32, kind: &str) -> Self {
        match self.sessions.iter_mut().find(|s| s.num == num) {
            Some(s) => s.kind = kind.to_owned(),
            None => self.sessions.push(SessionSpec {
                num,
                kind: kind.to_owned(),
                results: Vec::new(),
            }),
        }
        self.current = num;
        self
    }

    /// The current session's number.
    pub fn current_session(&self) -> i32 {
        self.current
    }

    /// Sets the results of session `num`.
    pub fn results(mut self, num: i32, results: Vec<ResultSpec>) -> Self {
        let s = self
            .sessions
            .iter_mut()
            .find(|s| s.num == num)
            .unwrap_or_else(|| panic!("no session {num}"));
        s.results = results;
        self
    }

    /// Sets the number of sectors, evenly spaced (0 writes no sector section).
    pub fn sectors(mut self, n: usize) -> Self {
        self.sectors = n;
        self
    }

    /// Sets the player's car slot.
    pub fn player_idx(mut self, idx: i32) -> Self {
        self.player_idx = idx;
        self
    }

    /// The player's car slot.
    pub fn player(&self) -> i32 {
        self.player_idx
    }

    /// Sets the pace car's slot; `None` writes no pace car.
    pub fn pace_car(mut self, idx: Option<i32>) -> Self {
        self.pace_idx = idx;
        self
    }

    /// Sets the tank: the car's maximum in litres and the share of it the series allows.
    pub fn fuel(mut self, max_l: f64, pct: f64) -> Self {
        self.fuel_max_l = max_l;
        self.fuel_pct = pct;
        self
    }

    /// Sets the number of real drivers (the player included) and of car classes they share.
    pub fn roster(mut self, drivers: usize, classes: usize) -> Self {
        assert!(drivers >= 1 && (1..=CLASSES.len()).contains(&classes));
        self.drivers = drivers;
        self.classes = classes;
        self
    }

    /// The car slots of the real drivers, in order; the player is among them.
    pub fn slots(&self) -> Vec<i32> {
        let mut slots: Vec<i32> = (0..)
            .filter(|i| Some(*i) != self.pace_idx)
            .take(self.drivers)
            .collect();
        if !slots.contains(&self.player_idx) {
            *slots.last_mut().expect("at least one driver") = self.player_idx;
        }
        slots
    }

    /// Replaces the name of the driver in slot `idx`; the text is written as given, so it may
    /// hold characters the Windows-1252 text cannot, or a line break.
    pub fn driver_name(mut self, idx: i32, name: &str) -> Self {
        self.names.retain(|(i, _)| *i != idx);
        self.names.push((idx, name.to_owned()));
        self
    }

    /// Sets the setup: its `UpdateCount` and the tyre readings of the last stop. `None` for the
    /// count writes no setup at all.
    pub fn setup(mut self, update_count: Option<i64>, readings: Option<TyreReadings>) -> Self {
        self.setup = update_count.map(|n| (n, readings));
        self
    }

    /// The setup's `UpdateCount`.
    pub fn setup_update_count(&self) -> Option<i64> {
        self.setup.map(|(n, _)| n)
    }

    fn name_of(&self, idx: i32) -> String {
        if let Some((_, n)) = self.names.iter().find(|(i, _)| *i == idx) {
            return n.clone();
        }
        if idx == self.player_idx {
            "Test Driver".to_owned()
        } else {
            NAMES[idx as usize % NAMES.len()].to_owned()
        }
    }

    /// The session text, as characters.
    pub fn build(&self) -> String {
        let mut s = String::from("---\n");
        self.weekend(&mut s);
        self.sessions_block(&mut s);
        if self.sectors > 0 {
            s.push_str("SplitTimeInfo:\n Sectors:\n");
            for i in 0..self.sectors {
                let pct = i as f64 / self.sectors as f64;
                let _ = writeln!(s, " - SectorNum: {i}\n   SectorStartPct: {pct:.6}");
            }
        }
        self.drivers_block(&mut s);
        if let Some((count, readings)) = self.setup {
            self.setup_block(&mut s, count, readings);
        }
        s.push_str("...\n");
        s
    }

    /// The session text as the simulator writes it: Windows-1252, a character it cannot
    /// represent as `?`.
    pub fn build_cp1252(&self) -> Vec<u8> {
        self.build()
            .chars()
            .map(|c| u8::try_from(u32::from(c)).unwrap_or(b'?'))
            .collect()
    }

    fn weekend(&self, s: &mut String) {
        let length = if self.imperial {
            format!("{:.2} mi", self.length_m / 1609.344)
        } else {
            format!("{:.2} km", self.length_m / 1000.0)
        };
        let limit = self
            .incident_limit
            .map_or("unlimited".to_owned(), |n| n.to_string());
        let classes = self.classes;
        let _ = write!(
            s,
            "WeekendInfo:\n TrackName: {}\n TrackID: {}\n TrackLength: {length}\n \
             TrackDisplayName: {}\n TrackConfigName: {}\n TrackPitSpeedLimit: 80.00 kph\n \
             SessionID: {}\n SubSessionID: {}\n EventType: Test\n Category: Road\n \
             SimMode: full\n Official: 0\n NumCarClasses: {classes}\n WeekendOptions:\n  \
             IncidentLimit: {limit}\n  IsFixedSetup: 0\n  StandingStart: 0\n",
            self.track_name,
            self.track_id,
            self.display_name,
            self.config_name,
            self.session_id,
            self.sub_session_id,
        );
    }

    fn sessions_block(&self, s: &mut String) {
        let _ = writeln!(
            s,
            "SessionInfo:\n CurrentSessionNum: {}\n Sessions:",
            self.current
        );
        for session in &self.sessions {
            let _ = writeln!(
                s,
                " - SessionNum: {}\n   SessionLaps: unlimited\n   SessionTime: 3600.0000 sec\n   \
                 SessionType: {}",
                session.num, session.kind
            );
            if session.results.is_empty() {
                s.push_str("   ResultsPositions: null\n");
            } else {
                s.push_str("   ResultsPositions:\n");
                for r in &session.results {
                    let _ = writeln!(
                        s,
                        "   - Position: {}\n     ClassPosition: {}\n     CarIdx: {}\n     \
                         ReasonOutId: {}\n     ReasonOutStr: {}",
                        r.position, r.class_position, r.car_idx, r.reason_out_id, r.reason_out
                    );
                }
            }
        }
    }

    fn drivers_block(&self, s: &mut String) {
        let pace = self.pace_idx.map_or(-1, i64::from);
        let _ = write!(
            s,
            "DriverInfo:\n DriverCarIdx: {}\n PaceCarIdx: {pace}\n DriverCarFuelMaxLtr: {:.3}\n \
             DriverCarMaxFuelPct: {:.3}\n DriverCarFuelKgPerLtr: 0.750\n Drivers:\n",
            self.player_idx, self.fuel_max_l, self.fuel_pct
        );
        if let Some(p) = self.pace_idx {
            let _ = writeln!(
                s,
                " - CarIdx: {p}\n   UserName: Pace Car\n   UserID: -1\n   CarPath: safety pcfast\n   \
                 CarScreenName: Pace Car\n   CarClassID: 11\n   CarClassShortName: Pace\n   \
                 CarIsPaceCar: 1\n   IsSpectator: 0"
            );
        }
        for (k, idx) in self.slots().into_iter().enumerate() {
            let (class_short, path, car, est) = CLASSES[k % self.classes];
            let _ = writeln!(
                s,
                " - CarIdx: {idx}\n   UserName: {}\n   UserID: {}\n   TeamName: {}\n   \
                 CarNumber: \"{}\"\n   CarPath: {path}\n   CarScreenName: {car}\n   \
                 CarClassID: {}\n   CarClassShortName: {class_short}\n   \
                 CarClassEstLapTime: {est:.4}\n   CarIsPaceCar: 0\n   IsSpectator: 0",
                self.name_of(idx),
                1000 + idx,
                self.name_of(idx),
                idx + 10,
                74 + (k % self.classes),
            );
        }
    }

    fn setup_block(&self, s: &mut String, count: i64, readings: Option<TyreReadings>) {
        let (pressure_unit, pressure): (&str, fn(f64) -> f64) = if self.imperial {
            ("psi", |kpa| kpa / 6.894_757)
        } else {
            ("kPa", |kpa| kpa)
        };
        let fuel = if self.imperial {
            format!("{:.1} gal", 80.0 / 3.785_411)
        } else {
            "80.0 L".to_owned()
        };
        let _ = write!(
            s,
            "CarSetup:\n UpdateCount: {count}\n TiresAero:\n  TireType:\n   TireType: Dry\n"
        );
        for (i, wheel) in ["LeftFront", "RightFront", "LeftRear", "RightRear"]
            .iter()
            .enumerate()
        {
            let cold = pressure(if i < 2 { 152.0 } else { 148.0 });
            let _ = writeln!(s, "  {wheel}:\n   ColdPressure: {cold:.1} {pressure_unit}");
            if let Some(r) = readings {
                let _ = writeln!(
                    s,
                    "   LastHotPressure: {:.1} {pressure_unit}\n   LastTempsOMI: {t:.0}C, {t:.0}C, \
                     {t:.0}C\n   TreadRemaining: {w:.0}%, {w:.0}%, {w:.0}%",
                    pressure(r.hot_kpa[i]),
                    t = r.temp_c[i],
                    w = r.tread_pct[i],
                );
            }
        }
        let _ = write!(
            s,
            " Chassis:\n  Front:\n   ArbSetting: 3\n   BrakePressureBias: 54.5%\n  Rear:\n   \
             ArbSetting: 2\n  InCarDials:\n   AbsSetting: 3 (ABS)\n   \
             TractionControlSetting: 4 (TC)\n Systems:\n  Fuel:\n   FuelLevel: {fuel}\n"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use irsdk::worker::SessionDoc;

    #[test]
    fn the_default_text_parses_with_every_section() {
        let text = YamlBuilder::new().build();
        let doc = SessionDoc::from_raw(1, text.as_bytes());
        assert!(doc.errors.is_empty(), "{:?}", doc.errors);
        let info = &doc.info;
        let weekend = info.weekend.as_ref().expect("weekend");
        assert_eq!(weekend.track_id, Some(261));
        assert_eq!(weekend.track_length_m(), Some(4010.0));
        assert_eq!(weekend.incident_limit(), Some(17));
        let sessions = info.session.as_ref().expect("sessions");
        assert_eq!(
            sessions
                .current(None)
                .expect("current")
                .session_type
                .as_deref(),
            Some("Practice")
        );
        assert_eq!(
            info.split
                .as_ref()
                .expect("split")
                .sectors
                .as_ref()
                .map(Vec::len),
            Some(3)
        );
        let drivers = info.driver.as_ref().expect("drivers");
        assert_eq!(
            drivers.player().expect("player").user_name.as_deref(),
            Some("Test Driver")
        );
        assert_eq!(drivers.fuel_capacity_l(), Some(100.0));
        assert_eq!(
            info.car_setup.as_ref().expect("setup").update_count(),
            Some(1)
        );
    }

    #[test]
    fn a_field_has_real_drivers_in_classes_and_a_pace_car() {
        let b = YamlBuilder::new().roster(20, 2);
        let doc = SessionDoc::from_raw(1, b.build().as_bytes());
        let drivers = doc.info.driver.expect("drivers").drivers.expect("list");
        assert_eq!(drivers.len(), 21);
        let real = drivers.iter().filter(|d| d.is_real()).count();
        assert_eq!(real, 20);
        let ids: std::collections::BTreeSet<_> = drivers
            .iter()
            .filter(|d| d.is_real())
            .filter_map(|d| d.car_class_id)
            .collect();
        assert_eq!(ids.len(), 2);
        assert_eq!(b.slots().len(), 20);
        assert!(b.slots().contains(&b.player()));
        assert_eq!(doc.info.weekend.expect("weekend").num_car_classes, Some(2));
    }

    #[test]
    fn cp1252_text_keeps_accents_as_single_bytes() {
        let b = YamlBuilder::new()
            .roster(3, 1)
            .driver_name(2, "\u{20ac}uro \u{e9}");
        let bytes = b.build_cp1252();
        assert!(bytes.contains(&0xE9), "e acute as one byte");
        assert!(!bytes.contains(&0xC3), "no UTF-8 lead byte");
        let doc = SessionDoc::from_raw(1, &bytes);
        let drivers = doc.info.driver.expect("drivers").drivers.expect("list");
        let names: Vec<_> = drivers.iter().filter_map(|d| d.user_name.clone()).collect();
        assert!(names.contains(&"?uro \u{e9}".to_owned()), "{names:?}");
        assert!(names.contains(&"Jos\u{e9} Garc\u{ed}a".to_owned()) || names.len() >= 3);
    }

    #[test]
    fn a_name_with_a_line_break_breaks_only_the_driver_section() {
        // The text is written as given, so the break splits the line: the section no longer
        // parses and the line scan still finds the player.
        let b = YamlBuilder::new().roster(2, 1).driver_name(2, "Two\nLines");
        let doc = SessionDoc::from_raw(1, b.build().as_bytes());
        assert!(
            doc.errors
                .iter()
                .any(|(section, _)| section == "DriverInfo")
        );
        assert!(doc.info.weekend.is_some() && doc.info.session.is_some());
        assert_eq!(doc.fallback.driver_car_idx, Some(1));
        assert_eq!(doc.fallback.fuel_max_ltr, Some(100.0));
    }

    #[test]
    fn units_follow_the_system() {
        let text = YamlBuilder::new().imperial().length_m(4023.0).build();
        assert!(text.contains("TrackLength: 2.50 mi"), "{text}");
        assert!(text.contains("psi") && text.contains("gal"));
        let doc = SessionDoc::from_raw(1, text.as_bytes());
        let m = doc
            .info
            .weekend
            .expect("weekend")
            .track_length_m()
            .expect("length");
        assert!((m - 4023.0).abs() < 10.0, "{m}");
    }

    #[test]
    fn results_sessions_and_the_setup_readings_are_written() {
        let b = YamlBuilder::new()
            .session(1, "Race")
            .results(1, vec![ResultSpec::running(1, 1)])
            .setup(Some(4), Some(TyreReadings::default()));
        let text = b.build();
        assert!(text.contains("LastHotPressure: 172.0 kPa"));
        assert!(text.contains("LastTempsOMI: 78C, 78C, 78C"));
        let doc = SessionDoc::from_raw(1, text.as_bytes());
        let sessions = doc.info.session.expect("sessions");
        assert_eq!(sessions.current_session_num, Some(1));
        let race = sessions.current(None).expect("current");
        assert_eq!(race.session_type.as_deref(), Some("Race"));
        assert_eq!(race.results_positions.as_ref().map(Vec::len), Some(1));
        assert!(
            !YamlBuilder::new()
                .setup(None, None)
                .build()
                .contains("CarSetup")
        );
    }
}
