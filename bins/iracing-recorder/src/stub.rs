//! The recorder loop: once a second, whether iRacing runs and whether Pitwall is linked.

use iracing_core::SPEC;
use std::time::Duration;
use stintmate_connector_link::boot::Booted;
use stintmate_connector_link::link::LinkState;
use stintmate_connector_link::status::{StatusInput, StatusWorker};
use stintmate_connector_platform::game::GameWatch;

/// The status text while the simulator is not running.
pub const WAITING: &str = "Waiting for iRacing";
/// The status text while it runs: nothing is read from it yet.
pub const RUNNING: &str = "iRacing is running; laps are not recorded by this version";

/// Fills `input` for the given facts.
pub fn fill(input: &mut StatusInput, game_running: bool, pitwall: &'static str, uptime_s: f64) {
    let p = &mut input.plain;
    p.game_running = game_running;
    p.phase = game_running.then_some("menus");
    p.message = if game_running { RUNNING } else { WAITING };
    p.pitwall = pitwall;
    p.uptime_s = uptime_s;
}

/// Runs until the stop event; the exit code of the process.
pub fn run(booted: Booted, hz_given: bool) -> i32 {
    tracing::info!(
        "Recorder {} started; rate {} Hz{}; no simulator data is read",
        SPEC.version,
        booted.hz,
        if hz_given { " (from --hz)" } else { "" }
    );
    booted.pitwall.start();
    if let Some(control) = &booted.control {
        control.start();
    }
    let worker = StatusWorker::new(booted.status);
    let mut watch = GameWatch::new(&SPEC.game);
    let mut input = StatusInput::default();
    let mut first = true;
    loop {
        let running = watch.running(false);
        let pitwall = match booted.pitwall.state().0 {
            LinkState::Connected => "connected",
            LinkState::Waiting => "waiting",
            LinkState::Refused => "refused",
        };
        fill(
            &mut input,
            running,
            pitwall,
            booted.started_at.elapsed().as_secs_f64().floor(),
        );
        worker.submit(&input, first);
        first = false;
        if booted.stop.wait(Duration::from_secs(1)) {
            break;
        }
    }
    tracing::info!("Stop requested; closing");
    worker.close();
    booted.pitwall.close();
    if let Some(control) = &booted.control {
        control.close();
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_status_follows_the_game() {
        let mut input = StatusInput::default();
        fill(&mut input, false, "waiting", 0.0);
        assert!(!input.plain.game_running && input.plain.phase.is_none());
        assert_eq!(input.plain.message, WAITING);
        fill(&mut input, true, "connected", 5.0);
        assert!(input.plain.game_running);
        assert_eq!(input.plain.phase, Some("menus"));
        assert_eq!(input.plain.message, RUNNING);
    }
}
