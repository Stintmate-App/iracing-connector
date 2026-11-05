//! What the recorder needs from the session text: the types of [`SessionMeta`].
//!
//! The session text is read on its own thread; the recorder takes the finished value as an
//! `Arc<SessionMeta>`. Only the data types are here.

use std::sync::Arc;

use crate::codes::SessionKind;
use crate::vars::CARS;

/// The identifiers and names of the track and its layout.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TrackIds {
    /// The track id: a slug of the display name (of the track name when there is none).
    pub track_id: String,
    /// The layout id: the simulator's track id as a decimal number.
    pub layout_id: String,
    /// The track's display name.
    pub track_name: String,
    /// The layout's name (`Default` when the session text gives none).
    pub layout_name: String,
    /// The lap length in metres.
    pub length_m: Option<f64>,
}

/// One car of the grid.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DriverEntry {
    /// The driver's name, on one line.
    pub name: String,
    /// The simulator's user id.
    pub user_id: i64,
    /// The car id: the car's path as written, trimmed.
    pub car_id: String,
    /// The car's display name.
    pub car_name: String,
    /// The class id: the class number as a decimal.
    pub class_id: String,
    /// The class's short name, else the car's display name.
    pub class_name: String,
    /// The class's estimated lap time in seconds.
    pub class_est_lap_s: Option<f64>,
    /// Whether this is a real competitor: not the pace car, not a spectator, with a name.
    pub is_real: bool,
}

/// One line of the current session's results.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ResultEntry {
    /// The overall position, 0 when unknown.
    pub position: i32,
    /// The position in the car's class, 0 when unknown.
    pub class_position: i32,
    /// The simulator's reason-out number (0 when the car is still running).
    pub reason_out_id: i32,
    /// The simulator's reason-out text.
    pub reason_out: String,
}

/// The car setup flattened to path and text pairs, in document order.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CarSetupSnapshot {
    /// The setup's `UpdateCount`, which grows when the driver changes it.
    pub update_count: Option<i64>,
    /// Every leaf of the setup: the dotted path of its keys and its text.
    pub entries: Vec<(String, String)>,
}

/// Everything the recorder reads from the session text for one session of the event.
#[derive(Clone, Debug, PartialEq)]
pub struct SessionMeta {
    /// The session text's update counter this was built from.
    pub update: i32,
    /// Counts the metas built, so the recorder can tell a new one with one atomic load.
    pub generation: u64,
    /// The session number this meta describes.
    pub session_num: i32,
    /// The kind of the session.
    pub kind: SessionKind,
    /// The session type as the session text writes it.
    pub kind_label: String,
    /// The track and layout.
    pub track: TrackIds,
    /// The player's entry, when the roster has it.
    pub player: Option<DriverEntry>,
    /// The player's car slot (`DriverCarIdx`), -1 when unknown.
    pub player_idx: i32,
    /// The pace car's slot, -1 when there is none.
    pub pace_car_idx: i32,
    /// The player's tank capacity in litres.
    pub fuel_capacity_l: Option<f64>,
    /// The incident limit; `None` when unlimited or not written.
    pub incident_limit: Option<u32>,
    /// Whether the event has more than one car class.
    pub multiclass: bool,
    /// The start of each sector as a fraction of the lap, in order.
    pub sector_starts: Vec<f64>,
    /// The roster by car slot; an entry that is not a real competitor is never shown or sent.
    pub roster: [Option<DriverEntry>; CARS],
    /// The current session's results by car slot.
    pub results: [ResultEntry; CARS],
    /// The player's setup, when the session text has one.
    pub setup: Option<Arc<CarSetupSnapshot>>,
    /// The `UpdateCount` of `setup`.
    pub setup_update: Option<i64>,
    /// A hash of what identifies the session: car, track, layout and session type.
    pub identity_hash: u64,
    /// A hash of the set of cars and classes of the roster and the track.
    pub catalog_hash: u64,
    /// A hash of the names by car slot.
    pub roster_hash: u64,
    /// The sections of the session text that did not parse: `(section, message)`.
    pub parse_errors: Vec<(String, String)>,
}

impl SessionMeta {
    /// A meta with nothing read yet, for session `session_num`.
    pub fn empty(session_num: i32) -> SessionMeta {
        SessionMeta {
            update: 0,
            generation: 0,
            session_num,
            kind: SessionKind::Other,
            kind_label: String::new(),
            track: TrackIds::default(),
            player: None,
            player_idx: -1,
            pace_car_idx: -1,
            fuel_capacity_l: None,
            incident_limit: None,
            multiclass: false,
            sector_starts: Vec::new(),
            roster: std::array::from_fn(|_| None),
            results: std::array::from_fn(|_| ResultEntry::default()),
            setup: None,
            setup_update: None,
            identity_hash: 0,
            catalog_hash: 0,
            roster_hash: 0,
            parse_errors: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_meta_has_a_slot_for_every_car_and_no_player() {
        let meta = SessionMeta::empty(2);
        assert_eq!(meta.session_num, 2);
        assert_eq!(meta.roster.len(), CARS);
        assert_eq!(meta.results.len(), CARS);
        assert!(meta.roster.iter().all(Option::is_none));
        assert_eq!((meta.player_idx, meta.pace_car_idx), (-1, -1));
        assert_eq!(meta, meta.clone());
    }
}
