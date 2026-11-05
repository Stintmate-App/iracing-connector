//! Resuming a session after the recorder restarts while the game keeps running.
//!
//! The recorder stores the game's session clock when it stops with a session open. A recorder
//! started later in the same game process continues that session when its first session has the
//! same identity string and the clock continued ([`IrClock::continues_as`]).

use serde_json::{Value, json};
use stintmate_connector_link::sink::ResumeClock;

use crate::meta::SessionMeta;
use crate::tick::Tick;

/// How long a session of an offline event (`SubSessionID` 0) may wait for a restarted recorder,
/// in seconds.
///
/// Not observed on the simulator yet: see docs/FINDINGS.md, Session identity. Offline, the ids
/// are probably the same in every event, so only the time since the last frame tells a
/// continued session from a new one.
pub const OFFLINE_RESUME_TIMEOUT_S: f64 = 600.0;

/// How far the session time may go back between the stored clock and the new one, in seconds.
const TIME_SLACK_S: f64 = 2.0;

/// The game's session clock at one row.
#[derive(Clone, Debug, PartialEq)]
pub struct IrClock {
    /// `SessionUniqueID`.
    pub unique_id: i32,
    /// `SubSessionID` (0 offline).
    pub sub_session: i64,
    /// `SessionNum`.
    pub session_num: i32,
    /// `SessionTime` in seconds.
    pub session_time: f64,
    /// `LapCompleted` of the player.
    pub lap_completed: i32,
    /// The wall clock of the row in seconds since 1970. Only offline events use it.
    pub wall_s: f64,
}

impl IrClock {
    /// The clock of `tick`, read at wall time `wall_s` (seconds since 1970), in the session
    /// `meta` describes.
    pub fn new(meta: &SessionMeta, tick: &Tick, wall_s: f64) -> IrClock {
        IrClock {
            unique_id: tick.session_unique_id,
            sub_session: meta.sub_session_id,
            session_num: tick.session_num,
            session_time: tick.session_time,
            lap_completed: tick.lap_completed,
            wall_s,
        }
    }
}

impl ResumeClock for IrClock {
    fn to_json(&self) -> Value {
        json!({
            "unique_id": self.unique_id,
            "sub_session": self.sub_session,
            "session_num": self.session_num,
            "session_time": self.session_time,
            "lap_completed": self.lap_completed,
            "wall_s": self.wall_s,
        })
    }

    fn from_json(v: &Value) -> Option<IrClock> {
        let int = |k: &str| v.get(k).and_then(Value::as_i64);
        let time = v.get("session_time").and_then(Value::as_f64)?;
        Some(IrClock {
            unique_id: i32::try_from(int("unique_id")?).ok()?,
            sub_session: int("sub_session")?,
            session_num: i32::try_from(int("session_num")?).ok()?,
            session_time: time,
            lap_completed: i32::try_from(int("lap_completed")?).ok()?,
            wall_s: v.get("wall_s").and_then(Value::as_f64).unwrap_or(0.0),
        })
    }

    /// The same unique id, sub-session and session number, a session time not more than two
    /// seconds earlier, no fewer laps; and, in an offline event, a new row not more than
    /// [`OFFLINE_RESUME_TIMEOUT_S`] after the stored one.
    fn continues_as(&self, now: &IrClock) -> bool {
        let same = self.unique_id == now.unique_id
            && self.sub_session == now.sub_session
            && self.session_num == now.session_num
            && now.session_time >= self.session_time - TIME_SLACK_S
            && now.lap_completed >= self.lap_completed;
        // Not observed across events yet: offline, `SessionUniqueID` is 1 and `SubSessionID` is 0
        // in the one event seen, probably in every event.
        let in_time = self.sub_session != 0
            || (0.0..=OFFLINE_RESUME_TIMEOUT_S).contains(&(now.wall_s - self.wall_s));
        same && in_time
    }
}

/// The text two sessions must share to be the same one:
/// `"{car_id}|{track_id}|{layout_id}|{kind}|{unique_id}|{session_num}"`.
pub fn identity(meta: &SessionMeta, tick: &Tick) -> String {
    format!(
        "{}|{}|{}|{}|{}|{}",
        meta.player.as_ref().map_or("", |p| p.car_id.as_str()),
        meta.track.track_id,
        meta.track.layout_id,
        meta.kind.as_str(),
        tick.session_unique_id,
        tick.session_num,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::meta::build;
    use crate::testing::YamlBuilder;
    use irsdk::worker::SessionDoc;

    fn clock() -> IrClock {
        IrClock {
            unique_id: 7,
            sub_session: 2_000_002,
            session_num: 1,
            session_time: 100.0,
            lap_completed: 3,
            wall_s: 1_000.0,
        }
    }

    #[test]
    fn the_clock_round_trips_through_json() {
        let c = clock();
        assert_eq!(IrClock::from_json(&c.to_json()), Some(c));
        assert_eq!(IrClock::from_json(&json!({"unique_id": 1})), None);
        assert_eq!(IrClock::from_json(&json!("x")), None);
    }

    #[test]
    fn an_old_file_without_wall_time_still_reads() {
        let mut v = clock().to_json();
        v.as_object_mut().unwrap().remove("wall_s");
        assert_eq!(IrClock::from_json(&v).unwrap().wall_s, 0.0);
    }

    #[test]
    fn a_later_row_of_the_same_session_continues() {
        let a = clock();
        let mut b = clock();
        b.session_time = 160.0;
        b.lap_completed = 4;
        b.wall_s = 1_060.0;
        assert!(a.continues_as(&b));
        // Two seconds back is within the slack, three is not.
        b.session_time = 98.0;
        assert!(a.continues_as(&b));
        b.session_time = 97.0;
        assert!(!a.continues_as(&b));
    }

    #[test]
    fn another_session_or_fewer_laps_does_not_continue() {
        let a = clock();
        for edit in [
            (|c: &mut IrClock| c.unique_id = 8) as fn(&mut IrClock),
            |c| c.sub_session = 5,
            |c| c.session_num = 2,
            |c| c.lap_completed = 2,
        ] {
            let mut b = clock();
            edit(&mut b);
            assert!(!a.continues_as(&b));
        }
    }

    #[test]
    fn an_offline_event_resumes_only_within_the_timeout() {
        let mut a = clock();
        a.sub_session = 0;
        let mut b = a.clone();
        b.wall_s = a.wall_s + OFFLINE_RESUME_TIMEOUT_S;
        assert!(a.continues_as(&b));
        b.wall_s += 1.0;
        assert!(!a.continues_as(&b));
        // A clock that went backwards is not a continuation either.
        b.wall_s = a.wall_s - 1.0;
        assert!(!a.continues_as(&b));
        // With a sub-session the wall time does not matter.
        let mut online = clock();
        online.wall_s = 0.0;
        let mut later = clock();
        later.wall_s = 1.0e9;
        assert!(online.continues_as(&later));
    }

    #[test]
    fn the_identity_joins_car_track_layout_kind_and_the_game_ids() {
        let meta = build(
            &SessionDoc::from_raw(1, YamlBuilder::new().build().as_bytes()),
            0,
        );
        let mut tick = Tick::new();
        tick.session_unique_id = 42;
        tick.session_num = 0;
        assert_eq!(
            identity(&meta, &tick),
            "porsche992rgt3|circuit-de-spa-francorchamps|261|practice|42|0"
        );
        let clock = IrClock::new(&meta, &tick, 5.0);
        assert_eq!((clock.unique_id, clock.sub_session), (42, 2_000_002));
    }
}
