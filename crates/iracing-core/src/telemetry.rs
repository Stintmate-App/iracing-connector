//! The `connector.telemetry` v1 sample: the neutral 32-byte record the brake cues run on.
//!
//! [`sample`] fills one from the row and the lap view. It is plain arithmetic on values already
//! in memory, so the recorder calls it for every row without allocating. The runtime's
//! [`Sample`] struct is used as it is.

use stintmate_connector_wire::telemetry::{
    FLYING, FRESH, IN_GARAGE, IN_PIT_LANE, LAP_INVALID, ON_TRACK, PLAYER_VIEW, Sample, TIMED,
};

use crate::io::LapType;
use crate::tick::Tick;
use crate::tracker::{LapView, State};

/// `game_state`: the simulator is not running.
pub const GAME_NOT_RUNNING: u8 = 0;
/// `game_state`: menus, or a session without a driving car (idle, garage).
pub const GAME_MENUS: u8 = 1;
/// `game_state`: driving, on the pit road or the track.
pub const GAME_PLAYING: u8 = 3;
/// `game_state`: no new row for a while (the simulator is paused).
pub const GAME_FROZEN: u8 = 4;
/// `game_state`: a replay is playing.
pub const GAME_REPLAY: u8 = 5;

/// The sample of one row.
///
/// `length_m` is the layout length (0 when not known) and `frame` the sample counter. The lap
/// distance is `-1` unless the car is driving, the lap time `-1` unless the lap is timed, and
/// the longitudinal acceleration is the simulator's own value (braking is negative).
pub fn sample(t: &Tick, v: &LapView, length_m: f32, frame: u32) -> Sample {
    let driving = matches!(v.state, State::Pits | State::Track);
    let mut flags = 0u16;
    if v.state == State::Track {
        flags |= ON_TRACK;
    }
    if t.on_pit_road {
        flags |= IN_PIT_LANE;
    }
    if t.is_in_garage {
        flags |= IN_GARAGE;
    }
    if v.timed {
        flags |= TIMED;
        if v.lap_type == Some(LapType::Flying) {
            flags |= FLYING;
        }
    }
    if !v.valid {
        flags |= LAP_INVALID;
    }
    if !t.is_replay {
        flags |= PLAYER_VIEW;
    }
    if t.fresh {
        flags |= FRESH;
    }
    let game_state = if t.is_replay {
        GAME_REPLAY
    } else {
        match v.state {
            State::Off => GAME_NOT_RUNNING,
            State::Idle | State::Garage => GAME_MENUS,
            State::Pits | State::Track => GAME_PLAYING,
        }
    };
    Sample {
        lap_distance_m: if driving { t.lap_dist } else { -1.0 },
        speed_ms: t.speed,
        accel_long_ms2: t.long_accel,
        track_length_m: length_m,
        lap_time_s: if v.timed { v.lap_clock_s } else { -1.0 },
        frame,
        sim_lap: t.lap.clamp(0, i32::from(u16::MAX)) as u16,
        session_type: v.session_kind.telemetry_code(),
        game_state,
        flags,
        reserved: 0,
    }
}

/// The keep-alive form of a sample: the same values, marked as a repeat (no `FRESH` flag) and,
/// when `frozen`, with the game state "paused".
pub fn repeat(mut s: Sample, frozen: bool) -> Sample {
    s.flags &= !FRESH;
    if frozen && s.game_state == GAME_PLAYING {
        s.game_state = GAME_FROZEN;
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codes::SessionKind;

    fn view(state: State) -> LapView {
        let mut v = crate::tracker::Tracker::new(0).view();
        v.state = state;
        v.session_kind = SessionKind::Race;
        v
    }

    #[test]
    fn driving_sample_carries_the_row() {
        let mut t = Tick::new();
        t.lap_dist = 1234.5;
        t.speed = 50.0;
        t.long_accel = -9.0;
        t.lap = 7;
        t.tick = 99;
        t.fresh = true;
        t.on_pit_road = false;
        let mut v = view(State::Track);
        v.timed = true;
        v.lap_clock_s = 42.5;
        v.lap_type = Some(LapType::Flying);
        let s = sample(&t, &v, 4000.0, 5);
        assert_eq!(s.lap_distance_m, 1234.5);
        assert_eq!(s.accel_long_ms2, -9.0);
        assert_eq!((s.lap_time_s, s.track_length_m, s.frame), (42.5, 4000.0, 5));
        assert_eq!((s.sim_lap, s.session_type, s.game_state), (7, 5, 3));
        let want = ON_TRACK | TIMED | FLYING | PLAYER_VIEW | FRESH;
        assert_eq!(s.flags, want);
    }

    #[test]
    fn not_driving_has_no_distance_or_time() {
        let mut t = Tick::new();
        t.lap_dist = 80.0;
        t.is_in_garage = true;
        let mut v = view(State::Garage);
        v.valid = false;
        let s = sample(&t, &v, 0.0, 1);
        assert_eq!(
            (s.lap_distance_m, s.lap_time_s, s.game_state),
            (-1.0, -1.0, 1)
        );
        assert_eq!(s.flags, IN_GARAGE | LAP_INVALID | PLAYER_VIEW);
    }

    #[test]
    fn states_map_to_game_states() {
        let t = Tick::new();
        let code = |st| sample(&t, &view(st), 0.0, 0).game_state;
        assert_eq!(code(State::Off), 0);
        assert_eq!(code(State::Idle), 1);
        assert_eq!(code(State::Pits), 3);
        let mut r = Tick::new();
        r.is_replay = true;
        let s = sample(&r, &view(State::Track), 0.0, 0);
        assert_eq!((s.game_state, s.flags & PLAYER_VIEW), (5, 0));
    }

    #[test]
    fn repeat_clears_fresh_and_freezes() {
        let mut t = Tick::new();
        t.fresh = true;
        let s = sample(&t, &view(State::Track), 0.0, 0);
        let r = repeat(s, true);
        assert_eq!((r.flags & FRESH, r.game_state), (0, GAME_FROZEN));
        assert_eq!(repeat(s, false).game_state, GAME_PLAYING);
    }
}
