//! `iracing-recorder.exe`: the iRacing connector's recorder, started hidden by the control app.

use iracing_core::SPEC;
use iracing_recorder::{cli, recorder};
use stintmate_connector_link::boot::boot;

fn main() {
    let opts = match cli::parse(std::env::args().skip(1).collect()) {
        Ok(opts) => opts,
        Err(line) => {
            eprintln!("iracing-recorder: {line}");
            std::process::exit(2);
        }
    };
    let booted = match boot(&SPEC, &opts.args) {
        Ok(booted) => booted,
        Err(code) => std::process::exit(code),
    };
    std::process::exit(recorder::run(booted, &opts));
}
