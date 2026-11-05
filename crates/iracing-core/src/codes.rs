//! Mappings from the SDK's session types, surfaces and pit states to the values the recorder
//! writes.

use irsdk::defs::SurfaceClass;

/// The kind of a session, from the `SessionType` text of the session text.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SessionKind {
    /// A practice session.
    Practice,
    /// An offline test session.
    Test,
    /// A qualifying session.
    Qualifying,
    /// A warm-up session.
    Warmup,
    /// A race (heats included).
    Race,
    /// Anything else.
    Other,
}

impl SessionKind {
    /// The name Pitwall's session types use: tests are practice.
    pub fn as_str(self) -> &'static str {
        match self {
            SessionKind::Practice | SessionKind::Test => "practice",
            SessionKind::Qualifying => "qualifying",
            SessionKind::Warmup => "warmup",
            SessionKind::Race => "race",
            SessionKind::Other => "other",
        }
    }

    /// The code the telemetry stream carries: 1 practice, 2 test, 3 qualifying, 4 warm-up,
    /// 5 race, 6 other.
    pub fn telemetry_code(self) -> u8 {
        match self {
            SessionKind::Practice => 1,
            SessionKind::Test => 2,
            SessionKind::Qualifying => 3,
            SessionKind::Warmup => 4,
            SessionKind::Race => 5,
            SessionKind::Other => 6,
        }
    }
}

fn contains_ci(haystack: &str, needle: &str) -> bool {
    haystack
        .as_bytes()
        .windows(needle.len())
        .any(|w| w.eq_ignore_ascii_case(needle.as_bytes()))
}

/// The kind of the session whose `SessionType` text is `session_type`.
///
/// The first rule that matches wins, ignoring case: `race`, then `qualif`, `warm`, `test`
/// (which also covers `testing`), `practice`; else [`SessionKind::Other`].
///
/// Not observed on the simulator yet: the rules above are the default in use. The table test
/// lists the strings expected ("Practice", "Open Practice", "Lone Practice", "Offline
/// Testing", "Lone Qualify", "Open Qualify", "Race"); add any other string seen.
pub fn session_type(session_type: &str) -> SessionKind {
    const RULES: [(&str, SessionKind); 5] = [
        ("race", SessionKind::Race),
        ("qualif", SessionKind::Qualifying),
        ("warm", SessionKind::Warmup),
        ("test", SessionKind::Test),
        ("practice", SessionKind::Practice),
    ];
    RULES
        .iter()
        .find(|(word, _)| contains_ci(session_type, word))
        .map_or(SessionKind::Other, |&(_, kind)| kind)
}

/// Pitwall's surface code for a surface class: 0 road, 1 kerb, 2 grass, 3 gravel, 4 sand,
/// 5 run-off, 8 other off-track. `None` for a surface that says nothing (not in the world, or
/// a number the SDK does not define): the caller keeps the previous code.
pub fn surface_code(class: SurfaceClass) -> Option<u8> {
    match class {
        SurfaceClass::Asphalt
        | SurfaceClass::Concrete
        | SurfaceClass::Paint
        | SurfaceClass::RacingDirt
        | SurfaceClass::Undefined => Some(0),
        SurfaceClass::Rumble => Some(1),
        SurfaceClass::Grass | SurfaceClass::Astroturf => Some(2),
        SurfaceClass::Gravel => Some(3),
        SurfaceClass::Sand => Some(4),
        SurfaceClass::Grasscrete => Some(5),
        SurfaceClass::Dirt => Some(8),
        SurfaceClass::NotInWorld | SurfaceClass::Unknown => None,
    }
}

/// `PlayerTrackSurface` value of a car in its pit stall.
const TRK_LOC_IN_PIT_STALL: i32 = 1;

/// Pitwall's pit mode: 0 on track, 1 into the pits, 2 in the box, 3 out of the pits or
/// garage, 4 garage.
///
/// `in_pit_stall` is `PlayerCarInPitStall` when the layout has it; otherwise the track
/// location says. `was_in_stall` is whether the car has been in its stall during this visit
/// to the pit road.
pub fn pit_mode(
    in_garage: bool,
    in_pit_stall: Option<bool>,
    track_loc: i32,
    on_pit_road: bool,
    was_in_stall: bool,
) -> u8 {
    if in_garage {
        4
    } else if in_pit_stall.unwrap_or(track_loc == TRK_LOC_IN_PIT_STALL) {
        2
    } else if on_pit_road {
        if was_in_stall { 3 } else { 1 }
    } else {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use irsdk::defs::surface_class;

    #[test]
    fn session_type_table() {
        use SessionKind::*;
        let table = [
            ("Practice", Practice),
            ("Open Practice", Practice),
            ("Lone Practice", Practice),
            ("Offline Testing", Test),
            ("Lone Qualify", Qualifying),
            ("Open Qualify", Qualifying),
            ("Qualifying", Qualifying),
            ("Warmup", Warmup),
            ("Race", Race),
            ("Heat Race", Race),
            ("consolation race", Race),
            ("RACE", Race),
            ("", Other),
            ("Lap Attack", Other),
        ];
        for (text, kind) in table {
            assert_eq!(session_type(text), kind, "{text:?}");
        }
    }

    #[test]
    fn names_and_telemetry_codes() {
        assert_eq!(SessionKind::Test.as_str(), "practice");
        assert_eq!(SessionKind::Practice.as_str(), "practice");
        assert_eq!(SessionKind::Qualifying.as_str(), "qualifying");
        assert_eq!(SessionKind::Warmup.as_str(), "warmup");
        assert_eq!(SessionKind::Race.as_str(), "race");
        assert_eq!(SessionKind::Other.as_str(), "other");
        let codes: Vec<u8> = [
            SessionKind::Practice,
            SessionKind::Test,
            SessionKind::Qualifying,
            SessionKind::Warmup,
            SessionKind::Race,
            SessionKind::Other,
        ]
        .map(SessionKind::telemetry_code)
        .to_vec();
        assert_eq!(codes, [1, 2, 3, 4, 5, 6]);
    }

    #[test]
    fn surface_table() {
        let table = [
            (-1, None),
            (0, Some(0)),
            (1, Some(0)),
            (6, Some(0)),
            (8, Some(0)),
            (10, Some(0)),
            (11, Some(1)),
            (14, Some(1)),
            (15, Some(2)),
            (18, Some(2)),
            (19, Some(8)),
            (22, Some(8)),
            (23, Some(4)),
            (24, Some(3)),
            (26, Some(5)),
            (27, Some(2)),
            (99, None),
        ];
        for (raw, code) in table {
            assert_eq!(surface_code(surface_class(raw)), code, "TrkSurf {raw}");
        }
    }

    #[test]
    fn pit_mode_table() {
        // (in_garage, in_pit_stall, track_loc, on_pit_road, was_in_stall) -> mode
        assert_eq!(pit_mode(true, Some(true), 1, true, true), 4);
        assert_eq!(pit_mode(false, Some(true), 1, true, false), 2);
        assert_eq!(
            pit_mode(false, None, 1, true, false),
            2,
            "location fallback"
        );
        assert_eq!(pit_mode(false, Some(false), 1, true, false), 1, "flag wins");
        assert_eq!(pit_mode(false, None, 2, true, false), 1);
        assert_eq!(pit_mode(false, None, 2, true, true), 3);
        assert_eq!(pit_mode(false, None, 3, false, true), 0);
    }
}
