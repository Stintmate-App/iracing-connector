//! `iracing-connector-ui.exe`: the iRacing connector's control app.
#![windows_subsystem = "windows"]

mod probe;

use std::sync::Arc;
use stintmate_connector_control::{Hooks, run};

fn main() {
    let argv = std::env::args().skip(1).collect();
    let hooks = Hooks {
        resume: Arc::new(probe::SdkResumeProbe::default()),
    };
    std::process::exit(run(&iracing_core::SPEC, hooks, argv));
}
