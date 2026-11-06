//! The iRacing recorder: reads the simulator's shared memory and records laps through Pitwall.
//!
//! [`cli`] parses the command line, [`recorder`] holds the loop, [`sink`] adapts the core's
//! records to the connector runtime's write sink, [`status`] fills the status report, [`live`]
//! serves the live frame, [`telemetry`] the sample stream and [`serve`] the capture target and the
//! actions.

pub mod cli;
pub mod live;
pub mod recorder;
pub mod serve;
pub mod sink;
pub mod status;
pub mod summary;
pub mod telemetry;
