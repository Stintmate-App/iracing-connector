//! `iracing-connector-ui.exe`: the iRacing connector's control app.
#![windows_subsystem = "windows"]

use stintmate_connector_control::{Hooks, run};

fn main() {
    let argv = std::env::args().skip(1).collect();
    std::process::exit(run(&iracing_core::SPEC, Hooks::default(), argv));
}
