//! `iracing-connector-window.exe`: the iRacing connector's window, one process at most.
#![windows_subsystem = "windows"]

use iracing_core::SPEC;

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    if argv.first().map(String::as_str) == Some("--version") {
        stintmate_connector_platform::console::attach_parent_output();
        println!("iracing-connector-window {}", SPEC.version);
        return;
    }
    std::process::exit(stintmate_connector_window::run(&SPEC, argv));
}
