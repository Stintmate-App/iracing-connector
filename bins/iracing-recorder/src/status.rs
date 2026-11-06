//! The status report: what the recorder tells the control app and Pitwall every half second.

use iracing_core::meta::SessionMeta;
use iracing_core::tracker::{LapView, State};
use stintmate_connector_link::status::StatusInput;

/// The text while the simulator is not running.
pub const WAITING: &str = "Waiting for iRacing";
/// The text while it runs and the player is not in a car (menus, spectating, no description).
pub const NOT_IN_CAR: &str = "iRacing is running: not in a car";
/// The text while laps are recorded.
pub const RECORDING: &str = "Recording";
/// The text while the player is in the garage.
pub const GARAGE: &str = "In the garage";
/// The text while a replay plays.
pub const REPLAY: &str = "Replay: not recording";
/// The error code when the simulator's variables are not the ones the recorder needs.
pub const UNSUPPORTED: &str = "unsupported_sim_version";

/// What the loop knows when it fills a status.
pub struct Facts<'a> {
    /// The simulator process runs.
    pub game_running: bool,
    /// The Pitwall link state as text.
    pub pitwall: &'static str,
    /// Seconds since the recorder started.
    pub uptime_s: f64,
    /// The memory map is open and the simulator is connected.
    pub mapped: bool,
    /// The variables could not be resolved.
    pub unsupported: bool,
    /// A replay is playing.
    pub replay: bool,
    /// The tracker's view of the lap.
    pub view: LapView,
    /// Whether a session is open.
    pub session_open: bool,
    /// The latest session description.
    pub meta: Option<&'a SessionMeta>,
}

/// Fills `input` in place (the strings reuse their buffers).
pub fn fill(input: &mut StatusInput, f: &Facts<'_>) {
    let state = if f.mapped { f.view.state } else { State::Off };
    let p = &mut input.plain;
    p.game_running = f.game_running;
    p.pitwall = f.pitwall;
    p.uptime_s = f.uptime_s;
    p.mapped = f.mapped;
    p.session_open = f.session_open;
    p.live = f.session_open;
    p.in_garage = state == State::Garage;
    p.lap = f.session_open.then_some(f.view.laps_recorded);
    p.error_code = f.unsupported.then_some(UNSUPPORTED);
    p.live_session_type = if f.session_open {
        f.view.session_kind.as_str()
    } else {
        ""
    };
    let (phase, message) = if !f.game_running {
        (None, WAITING)
    } else if f.replay && f.mapped {
        (Some("garage"), REPLAY)
    } else {
        match state {
            State::Off | State::Idle => (Some("menus"), NOT_IN_CAR),
            State::Garage => (Some("garage"), GARAGE),
            State::Pits | State::Track => (Some("on_track"), RECORDING),
        }
    };
    p.phase = phase;
    p.message = message;
    let open = f.meta.filter(|_| f.session_open);
    p.track_length_m = open.and_then(|m| m.track.length_m).unwrap_or(0.0);
    let set = |s: &mut String, v: Option<&str>| {
        s.clear();
        if let Some(v) = v {
            s.push_str(v);
        }
    };
    set(
        &mut input.car,
        open.and_then(|m| m.player.as_ref())
            .map(|d| d.car_id.as_str()),
    );
    set(&mut input.track, open.map(|m| m.track.track_id.as_str()));
    set(&mut input.layout, open.map(|m| m.track.layout_id.as_str()));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts(running: bool, mapped: bool, state: State) -> Facts<'static> {
        let mut view = iracing_core::tracker::Tracker::new(16).view();
        view.state = state;
        Facts {
            game_running: running,
            pitwall: "connected",
            uptime_s: 1.0,
            mapped,
            unsupported: false,
            replay: false,
            view,
            session_open: matches!(state, State::Garage | State::Pits | State::Track),
            meta: None,
        }
    }

    #[test]
    fn the_message_follows_the_state() {
        let mut i = StatusInput::default();
        fill(&mut i, &facts(false, false, State::Off));
        assert_eq!((i.plain.message, i.plain.phase), (WAITING, None));
        fill(&mut i, &facts(true, false, State::Off));
        assert_eq!(
            (i.plain.message, i.plain.phase),
            (NOT_IN_CAR, Some("menus"))
        );
        fill(&mut i, &facts(true, true, State::Garage));
        assert_eq!((i.plain.message, i.plain.phase), (GARAGE, Some("garage")));
        assert!(i.plain.in_garage && i.plain.live);
        fill(&mut i, &facts(true, true, State::Track));
        assert_eq!(
            (i.plain.message, i.plain.phase),
            (RECORDING, Some("on_track"))
        );
        let mut f = facts(true, true, State::Garage);
        f.replay = true;
        fill(&mut i, &f);
        assert_eq!(i.plain.message, REPLAY);
        f.unsupported = true;
        fill(&mut i, &f);
        assert_eq!(i.plain.error_code, Some(UNSUPPORTED));
    }
}
