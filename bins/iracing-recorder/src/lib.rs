//! The iRacing recorder: reads the simulator's shared memory and records laps through Pitwall.
//!
//! [`cli`] parses the command line, [`recorder`] holds the loop, [`sink`] adapts the core's
//! records to the connector runtime's write sink and [`status`] fills the status report.

pub mod cli;
pub mod recorder;
pub mod sink;
pub mod status;
