//! Exact lap positions from the simulator's disk telemetry, found after the session.
//!
//! While driving, the recorder keeps the address of every sample of a lap (its `SessionTick`
//! and `LapDistPct`) in a small file per session ([`file`]). After the session, when the user
//! has disk telemetry on, the simulator has written the same rows, with their latitude and
//! longitude, into `.ibt` files. This module finds the files that belong to a session
//! ([`select`]), matches the recorded samples to file rows and projects them to metres
//! ([`job`], [`project`]).
//!
//! Everything here is pure: no thread, no clock, no folder lookup. The caller supplies the
//! files, the time and a flag to stop.

pub mod file;
pub mod job;
pub mod project;
pub mod select;
