//! The control app's look at the simulator for a session kept open by a stopped recorder.

use std::time::Duration;

use iracing_core::resume::IrClock;
use irsdk::live::{Live, OpenError};
use irsdk::sampler::Poll;
use serde_json::Value;
use stintmate_connector_control::{ResumeLook, ResumeProbe};
use stintmate_connector_link::sink::ResumeClock;

/// How many times one look waits for a row, and for how long each time.
const TRIES: u32 = 3;
const TRY_WAIT: Duration = Duration::from_millis(25);

/// Compares the simulator's session with the one a recorder left open, by reading one row of
/// its shared memory. The view is closed before each answer.
#[derive(Clone, Debug, Default)]
pub struct SdkResumeProbe {
    /// A test mapping and event to read instead of the simulator's.
    names: Option<(String, String)>,
}

impl SdkResumeProbe {
    /// A probe that reads the named test mapping and event.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn with_names(map: &str, event: &str) -> SdkResumeProbe {
        SdkResumeProbe {
            names: Some((map.to_owned(), event.to_owned())),
        }
    }

    fn open(&self) -> Result<Live, OpenError> {
        match &self.names {
            Some((map, event)) => Live::open_named(map, event),
            None => Live::open(),
        }
    }
}

impl ResumeProbe for SdkResumeProbe {
    fn look(&self, stored_clock: &Value, game_running: bool) -> ResumeLook {
        if !game_running {
            return ResumeLook::Gone;
        }
        let Some(stored) = IrClock::from_json(stored_clock) else {
            return ResumeLook::Drop;
        };
        let mut live = match self.open() {
            Ok(live) => live,
            // The process runs but exposes no data: the simulator went back to its menus.
            Err(OpenError::NotRunning) => return ResumeLook::Drop,
            Err(_) => return ResumeLook::NoReading,
        };
        if !live.sampler().connected() {
            return ResumeLook::Drop;
        }
        for _ in 0..TRIES {
            match live.wait(TRY_WAIT) {
                Poll::Row => {
                    let vars = live.sampler().vars();
                    let (Ok(unique), Ok(num)) = (
                        vars.find::<i32>("SessionUniqueID"),
                        vars.find::<i32>("SessionNum"),
                    ) else {
                        return ResumeLook::NoReading;
                    };
                    let row = live.sampler().row();
                    let same =
                        unique.read(row) == stored.unique_id && num.read(row) == stored.session_num;
                    return if same {
                        ResumeLook::Keep
                    } else {
                        ResumeLook::Drop
                    };
                }
                Poll::Disconnected => return ResumeLook::Drop,
                _ => {}
            }
        }
        ResumeLook::NoReading
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iracing_core::testing::{LapProfile, Scenario, YamlBuilder};
    use irsdk::testing::map::TestMap;
    use std::sync::atomic::AtomicBool;

    fn stored(unique_id: i32, session_num: i32) -> Value {
        IrClock {
            unique_id,
            sub_session: 0,
            session_num,
            session_time: 10.0,
            lap_completed: 0,
            wall_s: 0.0,
        }
        .to_json()
    }

    #[test]
    fn the_probe_compares_the_session_it_reads() {
        let mut s = Scenario::new(YamlBuilder::new());
        s.garage(0.5).to_stall().drive_out();
        s.lap(LapProfile::new(10.0));
        let tag = std::process::id();
        let (map_name, event) = (
            format!("probe_test_{tag}_map"),
            format!("probe_test_{tag}_ev"),
        );
        let size = TestMap::required_size(s.layout(), s.initial_yaml().len());
        let mut map = TestMap::create(&map_name, &event, s.layout(), s.initial_yaml(), size)
            .expect("test map");
        let probe = SdkResumeProbe::with_names(&map_name, &event);

        assert_eq!(probe.look(&stored(1, 0), false), ResumeLook::Gone);
        assert_eq!(probe.look(&Value::Null, true), ResumeLook::Drop);
        // No row yet: nothing to compare.
        assert_eq!(probe.look(&stored(1, 0), true), ResumeLook::NoReading);

        let stop = AtomicBool::new(false);
        std::thread::scope(|scope| {
            scope.spawn(|| s.replay_into(&mut map, 60.0, &stop));
            std::thread::sleep(Duration::from_millis(300));
            // The scenario's session numbers and unique id are read back from its first row.
            let first = s.ibt();
            let vars = first.vars();
            let unique = vars.find::<i32>("SessionUniqueID").unwrap();
            let num = vars.find::<i32>("SessionNum").unwrap();
            let row = irsdk::row::Row::from_slice(first.row(0).unwrap());
            let (u, n) = (unique.read(&row), num.read(&row));
            assert_eq!(probe.look(&stored(u, n), true), ResumeLook::Keep);
            assert_eq!(probe.look(&stored(u + 1, n), true), ResumeLook::Drop);
            assert_eq!(probe.look(&stored(u, n + 1), true), ResumeLook::Drop);
            stop.store(true, std::sync::atomic::Ordering::Relaxed);
        });
        // A mapping that does not exist: the simulator is up but exposes nothing.
        let gone = SdkResumeProbe::with_names("probe_test_missing_map", "probe_test_missing_ev");
        assert_eq!(gone.look(&stored(1, 0), true), ResumeLook::Drop);
    }
}
