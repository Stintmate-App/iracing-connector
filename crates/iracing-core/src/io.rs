//! The interfaces between the recorder parts: where rows come from ([`Source`]), where the
//! tracker's records go ([`Sink`]) and where wall time comes from ([`Clock`]), with the records
//! themselves.

use std::sync::Arc;

use crate::lap::LapBuffer;
use crate::meta::SessionMeta;
use crate::sectors::SectorTimes;
use crate::tick::Tick;
use crate::validity::InvalidCause;

/// The wall clock. The core reads no clock of its own: time inside a session is the row's
/// `SessionTime`, wall time comes from here.
pub trait Clock: Send {
    /// The current time as an ISO 8601 text in UTC, for timestamps.
    fn iso(&self) -> String;
    /// Seconds on a monotonic clock that keeps running while the simulator is gone (the
    /// session time does not). Only differences are used.
    fn mono_s(&self) -> f64;
}

/// What a [`Source`] found when asked for the next row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourceEvent {
    /// The tick was filled with a new row.
    Row,
    /// There is no new row yet.
    NoRow,
    /// The row layout or the event restarted: the next rows belong to a new run.
    Reset,
    /// The simulator is gone.
    Disconnected,
    /// The source has no more rows (a file ends).
    End,
}

/// The recorder frame source, live or replayed.
pub trait Source {
    /// Fills `tick` with the next row, setting `connected` and `fresh`, when there is one.
    fn next(&mut self, tick: &mut Tick) -> SourceEvent;

    /// The session description that goes with the rows, when the source holds one itself.
    fn meta(&self) -> Option<&Arc<SessionMeta>> {
        None
    }
}

/// Why a setup is sent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SetupReason {
    /// The session opened.
    SessionStart,
    /// The car left the pit road.
    PitExit,
}

impl SetupReason {
    /// The reason as the write API names it.
    pub fn as_str(self) -> &'static str {
        match self {
            SetupReason::SessionStart => "session_start",
            SetupReason::PitExit => "pit_exit",
        }
    }
}

/// What tells one session from another, compared on every row with integers only.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SessionKey {
    /// The simulator's `SessionUniqueID`.
    pub unique_id: i32,
    /// The index of the session inside the event.
    pub session_num: i32,
    /// Hash of the car, track, layout and session type (`SessionMeta::identity_hash`).
    pub meta_identity: u64,
}

/// The weather and track state at the end of a lap or the start of a session.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Conditions {
    /// Air temperature in degrees Celsius.
    pub ambient_c: Option<f32>,
    /// Track temperature in degrees Celsius.
    pub track_c: Option<f32>,
    /// Precipitation as a fraction, 0 to 1.
    pub rain: Option<f32>,
    /// Wind speed in metres per second.
    pub wind_speed_ms: Option<f32>,
}

impl Conditions {
    /// The conditions the row shows.
    pub fn of(t: &Tick) -> Conditions {
        Conditions {
            ambient_c: t.air_c,
            track_c: t.track_c,
            rain: t.precip,
            wind_speed_ms: t.wind_ms,
        }
    }
}

/// What a session opening carries.
#[derive(Clone, Debug)]
pub struct SessionOpenInfo {
    /// The identity of the session.
    pub key: SessionKey,
    /// The session description (the sink builds the catalog from it).
    pub meta: Arc<SessionMeta>,
    /// The conditions on the opening row.
    pub conditions: Conditions,
    /// Wall time of the opening, ISO 8601.
    pub started_at: String,
}

/// The kind of a lap.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LapType {
    /// A formation or parade lap.
    Formation,
    /// A lap that started in the pits, the stall or the garage.
    Out,
    /// A lap that ended in the pits or by leaving the car.
    In,
    /// Any other lap.
    Flying,
}

impl LapType {
    /// The name the write API uses.
    pub fn as_str(self) -> &'static str {
        match self {
            LapType::Formation => "formation",
            LapType::Out => "out",
            LapType::In => "in",
            LapType::Flying => "flying",
        }
    }
}

/// Where a lap time comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimeSource {
    /// The simulator's own time (`LapLastLapTime`).
    Official,
    /// Crossing to crossing, interpolated by the recorder.
    Measured,
}

impl TimeSource {
    /// The name the write API uses.
    pub fn as_str(self) -> &'static str {
        match self {
            TimeSource::Official => "official",
            TimeSource::Measured => "measured",
        }
    }
}

/// A finished lap. It owns the channel columns: they are moved, never copied.
#[derive(Clone, Debug)]
pub struct FinishedLap {
    /// The simulator's lap counter when the lap started.
    pub sim_lap: i32,
    /// The kind of lap.
    pub lap_type: LapType,
    /// Whether the lap ended before the line (a reset, a tow, leaving the car, the session
    /// ending). A partial lap has no time.
    pub partial: bool,
    /// The lap time in seconds, when the lap is timed.
    pub time_s: Option<f32>,
    /// Where `time_s` comes from.
    pub time_source: Option<TimeSource>,
    /// Crossing to crossing, when the lap started at the line.
    pub measured_time_s: Option<f32>,
    /// Whether the lap is valid.
    pub valid: bool,
    /// Why it is not valid, when known.
    pub cause: Option<InvalidCause>,
    /// Whether `cause` is certain.
    pub cause_certain: bool,
    /// Distance along the lap in metres where it became invalid.
    pub invalid_at_m: Option<f32>,
    /// Lap clock in seconds where it became invalid.
    pub invalid_at_s: Option<f32>,
    /// Whether any sample was taken on the track.
    pub on_track: bool,
    /// Whether the lap touched the pit road.
    pub touched_pit: bool,
    /// Session time of the lap start.
    pub started_at_s: f64,
    /// Session time of the lap end.
    pub ended_at_s: f64,
    /// Fuel in litres at the start.
    pub fuel_start_l: Option<f32>,
    /// Litres burnt, not counting time in the stall.
    pub fuel_used_l: f32,
    /// Top speed in km/h.
    pub max_speed_kmh: f32,
    /// The sector times.
    pub sectors: SectorTimes,
    /// The conditions at the end of the lap.
    pub conditions: Conditions,
    /// The channel columns.
    pub buffer: LapBuffer,
}

/// One lap of one car of the field, for the rival records.
#[derive(Clone, Debug, PartialEq)]
pub struct ParticipantLap {
    /// The car's slot.
    pub participant: u8,
    /// The car's completed laps after the crossing.
    pub lap: i32,
    /// The driver's name.
    pub name: String,
    /// The car id.
    pub car_id: String,
    /// The class id.
    pub class_id: String,
    /// Whether this is the player.
    pub is_player: bool,
    /// The lap time in seconds, when known.
    pub time_s: Option<f32>,
    /// Where `time_s` comes from.
    pub time_source: Option<TimeSource>,
    /// The sector times in seconds, as many as the track has (empty when it has none); `None`
    /// for a sector that was not timed.
    pub sectors: Vec<Option<f32>>,
    /// Whether the lap counts as valid.
    pub valid: bool,
    /// Whether the car touched the pit road during the lap.
    pub pit: bool,
    /// Pit stops since the session started.
    pub pit_stops: u32,
    /// Whether the crossing that started the lap was seen.
    pub from_line: bool,
    /// The position after the lap.
    pub position: Option<i32>,
    /// Session time of the crossing.
    pub crossed_at_s: Option<f64>,
    /// The simulator's best lap of the car in seconds.
    pub sim_best_lap_s: Option<f32>,
    /// Why the car is out, when it is.
    pub state: Option<String>,
}

/// Why the car was in the pits when the simulator gave new tyre readings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StallOccasion {
    /// The car drove into its pit box.
    PitStop,
    /// The simulator put the car back in the pits.
    Reset,
    /// The driver got into the car in the pits.
    CarEntry,
}

impl StallOccasion {
    /// The write API's `occasion` value.
    pub fn as_str(self) -> &'static str {
        match self {
            StallOccasion::PitStop => "pit_stop",
            StallOccasion::Reset => "reset",
            StallOccasion::CarEntry => "car_entry",
        }
    }
}

/// A new set of tyre readings: temperatures and wear per wheel (left front, right front, left
/// rear, right rear) and position (left, middle, right).
///
/// The simulator updates its live readings only in the pits (a stop, a reset, getting into the
/// car), so these are the readings of that moment. They are shown in the live frame and sent as
/// a `tyre_readings` write.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StallTyres {
    /// Session time of the row that carried them.
    pub at_s: f64,
    /// Why the car was in the pits.
    pub occasion: StallOccasion,
    /// Temperatures in degrees Celsius.
    pub temp_c: [[f32; 3]; 4],
    /// Wear as the simulator reports it (1 new).
    pub wear: [[f32; 3]; 4],
}

/// Where the tracker's records go.
pub trait Sink {
    /// A session opened (the sink sends the catalog first). Returns the session's local id.
    fn start_session(&mut self, open: &SessionOpenInfo) -> u64;
    /// The session ended.
    fn end_session(&mut self, local: u64, ended_at: &str);
    /// A lap finished.
    fn lap(&mut self, local: u64, lap: FinishedLap);
    /// A setup was taken. `fuel_l` is 0 when the tank level is not known.
    fn setup(
        &mut self,
        local: u64,
        reason: SetupReason,
        at: &str,
        meta: Arc<SessionMeta>,
        fuel_l: f32,
    );
    /// The car left the pit road.
    fn pit_exit(&mut self, local: u64, at: &str, fuel_l: Option<f64>);
    /// Laps of the field.
    fn participants(&mut self, local: u64, rows: Vec<ParticipantLap>);
    /// Cars that joined after the session opened.
    fn catalog(&mut self, meta: Arc<SessionMeta>);
    /// The simulator gave a new set of tyre readings in the pits; `at` is the wall-clock time of
    /// the row that carried them. Optional.
    fn stall_tyres(&mut self, _local: u64, _at: &str, _tyres: &StallTyres) {}
    /// A fresh, empty lap buffer for the tracker's pool, made on the sink's own thread so the
    /// tick thread does not allocate it. Called after each lap is handed over. `None` (the
    /// default) makes the tracker allocate one when the next lap ends.
    fn spare_buffer(&mut self, _rows: usize) -> Option<LapBuffer> {
        None
    }
}
