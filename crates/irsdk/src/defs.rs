//! The constants and enumerations of the iRacing SDK.
//!
//! The values follow the SDK header `irsdk_defines.h` (see `THIRD-PARTY-NOTICES-irsdk.txt`).
//! The broadcast message name is not defined here: this crate never sends a message to the game.

/// Name of the memory mapping the simulator publishes its data in.
pub const MEM_MAP_NAME: &str = r"Local\IRSDKMemMapFileName";
/// Name of the event the simulator signals after each new row.
pub const DATA_VALID_EVENT_NAME: &str = r"Local\IRSDKDataValidEvent";

/// The layout version this crate reads.
pub const VER: i32 = 2;
/// The most row buffers the header describes.
pub const MAX_BUFS: usize = 4;
/// Size of the name and unit fields of a variable header, in bytes.
pub const MAX_STRING: usize = 32;
/// Size of the description field of a variable header, in bytes.
pub const MAX_DESC: usize = 64;
/// The lap count of a session with no lap limit.
pub const UNLIMITED_LAPS: i32 = 32767;
/// The session time of a session with no time limit, in seconds.
pub const UNLIMITED_TIME: f32 = 604_800.0;
/// Bit of the header `status` that tells the simulator is connected.
pub const STATUS_CONNECTED: i32 = 1;

/// The type of a telemetry variable.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(i32)]
pub enum VarType {
    /// One byte, a character.
    Char = 0,
    /// One byte, a boolean.
    Bool = 1,
    /// A signed 32-bit integer.
    Int = 2,
    /// A 32-bit set of flags.
    BitField = 3,
    /// A 32-bit float.
    Float = 4,
    /// A 64-bit float.
    Double = 5,
}

impl VarType {
    /// The type for the raw value of a variable header, if it is a known one.
    pub fn from_i32(raw: i32) -> Option<VarType> {
        Some(match raw {
            0 => VarType::Char,
            1 => VarType::Bool,
            2 => VarType::Int,
            3 => VarType::BitField,
            4 => VarType::Float,
            5 => VarType::Double,
            _ => return None,
        })
    }

    /// Size of one value of this type, in bytes.
    pub fn size(self) -> usize {
        match self {
            VarType::Char | VarType::Bool => 1,
            VarType::Int | VarType::BitField | VarType::Float => 4,
            VarType::Double => 8,
        }
    }
}

/// Bits of the `SessionFlags` variable.
pub mod session_flags {
    /// The checkered flag.
    pub const CHECKERED: u32 = 0x1;
    /// The white flag.
    pub const WHITE: u32 = 0x2;
    /// The green flag.
    pub const GREEN: u32 = 0x4;
    /// The yellow flag.
    pub const YELLOW: u32 = 0x8;
    /// The red flag.
    pub const RED: u32 = 0x10;
    /// The blue flag.
    pub const BLUE: u32 = 0x20;
    /// Debris on the track.
    pub const DEBRIS: u32 = 0x40;
    /// The crossed flags.
    pub const CROSSED: u32 = 0x80;
    /// The yellow flag, waving.
    pub const YELLOW_WAVING: u32 = 0x100;
    /// One lap to green.
    pub const ONE_LAP_TO_GREEN: u32 = 0x200;
    /// The green flag is held.
    pub const GREEN_HELD: u32 = 0x400;
    /// Ten laps to go.
    pub const TEN_TO_GO: u32 = 0x800;
    /// Five laps to go.
    pub const FIVE_TO_GO: u32 = 0x1000;
    /// A random flag, waving.
    pub const RANDOM_WAVING: u32 = 0x2000;
    /// A caution.
    pub const CAUTION: u32 = 0x4000;
    /// A caution, waving.
    pub const CAUTION_WAVING: u32 = 0x8000;
    /// The black flag.
    pub const BLACK: u32 = 0x1_0000;
    /// The car is disqualified.
    pub const DISQUALIFY: u32 = 0x2_0000;
    /// The car is serviceable.
    pub const SERVICIBLE: u32 = 0x4_0000;
    /// The furled black flag.
    pub const FURLED: u32 = 0x8_0000;
    /// The car must repair.
    pub const REPAIR: u32 = 0x10_0000;
    /// Scoring of the disqualified car is invalid.
    pub const DQ_SCORING_INVALID: u32 = 0x20_0000;
    /// Start lights hidden.
    pub const START_HIDDEN: u32 = 0x1000_0000;
    /// Start lights: ready.
    pub const START_READY: u32 = 0x2000_0000;
    /// Start lights: set.
    pub const START_SET: u32 = 0x4000_0000;
    /// Start lights: go.
    pub const START_GO: u32 = 0x8000_0000;
}

/// Generates an enum of raw `i32` values with a `from_i32` that rejects unknown values.
macro_rules! raw_enum {
    ($(#[$meta:meta])* $name:ident { $( $(#[$vmeta:meta])* $variant:ident = $val:expr ),+ $(,)? }) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        pub enum $name {
            $( $(#[$vmeta])* $variant ),+
        }

        impl $name {
            /// The value for a raw number of the SDK, if it is a known one.
            pub fn from_i32(raw: i32) -> Option<$name> {
                match raw {
                    $( $val => Some($name::$variant), )+
                    _ => None,
                }
            }
        }
    };
}

raw_enum! {
    /// The `SessionState` variable.
    SessionState {
        /// Not a valid state.
        Invalid = 0,
        /// The driver is getting in the car.
        GetInCar = 1,
        /// Warm-up.
        Warmup = 2,
        /// Parade laps.
        ParadeLaps = 3,
        /// Racing.
        Racing = 4,
        /// The checkered flag has fallen.
        Checkered = 5,
        /// Cool down.
        CoolDown = 6,
    }
}

raw_enum! {
    /// The `PlayerTrackSurface` location values (`TrkLoc`).
    TrkLoc {
        /// Not in the world.
        NotInWorld = -1,
        /// Off the track.
        OffTrack = 0,
        /// In the pit stall.
        InPitStall = 1,
        /// Approaching the pits.
        ApproachingPits = 2,
        /// On the track.
        OnTrack = 3,
    }
}

raw_enum! {
    /// The `CarLeftRight` variable.
    CarLeftRight {
        /// The spotter is off.
        Off = 0,
        /// No car alongside.
        Clear = 1,
        /// A car on the left.
        CarLeft = 2,
        /// A car on the right.
        CarRight = 3,
        /// Cars on both sides.
        CarLeftRight = 4,
        /// Two cars on the left.
        TwoCarsLeft = 5,
        /// Two cars on the right.
        TwoCarsRight = 6,
    }
}

raw_enum! {
    /// Pace mode values (`PaceMode`). The values are marked "to verify": the SDK research names
    /// them without their numbers; a probe on the real simulator must confirm them before the
    /// recorder relies on them.
    PaceMode {
        /// Single-file start.
        SingleFileStart = 0,
        /// Double-file start.
        DoubleFileStart = 1,
        /// Single-file restart.
        SingleFileRestart = 2,
        /// Double-file restart.
        DoubleFileRestart = 3,
        /// Not pacing.
        NotPacing = 4,
    }
}

/// Bits of the pace flags. Marked "to verify" like [`PaceMode`].
pub mod pace_flags {
    /// The car is at the end of the line.
    pub const END_OF_LINE: u32 = 0x1;
    /// The car gets a free pass.
    pub const FREE_PASS: u32 = 0x2;
    /// The car is waved around.
    pub const WAVED_AROUND: u32 = 0x4;
}

/// The `TrkSurf` values, one number per surface.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SurfaceClass {
    /// Not in the world (-1).
    NotInWorld,
    /// Undefined (0).
    Undefined,
    /// Asphalt (1 to 4).
    Asphalt,
    /// Concrete (5 and 6).
    Concrete,
    /// Racing dirt (7 and 8).
    RacingDirt,
    /// Paint (9 and 10).
    Paint,
    /// Rumble strips (11 to 14).
    Rumble,
    /// Grass (15 to 18).
    Grass,
    /// Dirt (19 to 22).
    Dirt,
    /// Sand (23).
    Sand,
    /// Gravel (24 and 25).
    Gravel,
    /// Grasscrete (26).
    Grasscrete,
    /// Astroturf (27).
    Astroturf,
    /// A number the SDK does not define.
    Unknown,
}

/// The class of a `TrkSurf` number.
pub fn surface_class(raw: i32) -> SurfaceClass {
    match raw {
        -1 => SurfaceClass::NotInWorld,
        0 => SurfaceClass::Undefined,
        1..=4 => SurfaceClass::Asphalt,
        5..=6 => SurfaceClass::Concrete,
        7..=8 => SurfaceClass::RacingDirt,
        9..=10 => SurfaceClass::Paint,
        11..=14 => SurfaceClass::Rumble,
        15..=18 => SurfaceClass::Grass,
        19..=22 => SurfaceClass::Dirt,
        23 => SurfaceClass::Sand,
        24..=25 => SurfaceClass::Gravel,
        26 => SurfaceClass::Grasscrete,
        27 => SurfaceClass::Astroturf,
        _ => SurfaceClass::Unknown,
    }
}

/// The `PitSvStatus` values.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PitSvStatus(pub i32);

impl PitSvStatus {
    /// No service.
    pub const NONE: PitSvStatus = PitSvStatus(0);
    /// Service in progress.
    pub const IN_PROGRESS: PitSvStatus = PitSvStatus(1);
    /// Service complete.
    pub const COMPLETE: PitSvStatus = PitSvStatus(2);

    /// Whether the value is one of the error codes (100 to 105).
    pub fn is_error(self) -> bool {
        (100..=105).contains(&self.0)
    }
}

/// The `PlayerCarMyIncidentCount`-style incident word: report in the low byte, penalty in the
/// high byte.
pub mod incident_flags {
    /// Loss of control.
    pub const REPORT_LOSS_OF_CONTROL: u32 = 1;
    /// Off track.
    pub const REPORT_OFF_TRACK: u32 = 2;
    /// Contact with the world.
    pub const REPORT_CONTACT_WITH_WORLD: u32 = 4;
    /// Collision with the world.
    pub const REPORT_COLLISION_WITH_WORLD: u32 = 5;
    /// Contact with another car.
    pub const REPORT_CAR_CONTACT: u32 = 7;
    /// Collision with another car.
    pub const REPORT_CAR_COLLISION: u32 = 8;
    /// Penalty: no points.
    pub const PENALTY_0X: u32 = 0x100;
    /// Penalty: one point.
    pub const PENALTY_1X: u32 = 0x200;
    /// Penalty: two points.
    pub const PENALTY_2X: u32 = 0x300;
    /// Penalty: four points.
    pub const PENALTY_4X: u32 = 0x400;

    /// The report: the low byte.
    pub fn report(word: u32) -> u32 {
        word & 0xff
    }

    /// The incident points of the penalty in the high byte, if it is a known one.
    pub fn penalty_points(word: u32) -> Option<u8> {
        match word & 0xff00 {
            PENALTY_0X => Some(0),
            PENALTY_1X => Some(1),
            PENALTY_2X => Some(2),
            PENALTY_4X => Some(4),
            _ => None,
        }
    }
}
