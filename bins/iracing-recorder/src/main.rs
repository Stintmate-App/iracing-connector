//! `iracing-recorder.exe`: the iRacing connector's recorder, started hidden by the control app.
//!
//! It connects to Pitwall and reports whether iRacing runs. It reads no simulator data and
//! writes no laps yet.

mod stub;

use iracing_core::SPEC;
use stintmate_connector_link::boot::{RecorderArgs, boot};

fn main() {
    let args = match RecorderArgs::parse(std::env::args().skip(1)) {
        Ok(args) => args,
        Err(line) => {
            eprintln!("iracing-recorder: {line}");
            std::process::exit(2);
        }
    };
    if let Some(unknown) = args.rest.first() {
        // The recorder is started only by the control app, which passes nothing else.
        eprintln!("iracing-recorder: unknown option {unknown}");
        std::process::exit(2);
    }
    let booted = match boot(&SPEC, &args) {
        Ok(booted) => booted,
        Err(code) => std::process::exit(code),
    };
    std::process::exit(stub::run(booted, args.hz.is_some()));
}
