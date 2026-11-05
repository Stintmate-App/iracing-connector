//! `iracing-probe`: a read-only diagnostic that records what the simulator exposes (its shared
//! memory and its windows) into a folder, to confirm facts about the simulator.

#![deny(unsafe_code)]

mod probe;
mod util;
mod win;

use std::path::PathBuf;
use std::process::ExitCode;

use probe::{Config, Probe};

const USAGE: &str = "usage: iracing-probe --out <folder> [--minutes N] [--csv-hz 1]";

fn parse(args: &[String]) -> Result<Config, String> {
    let (mut out, mut minutes, mut csv_hz) = (None, None, 1.0);
    let (mut map, mut event) = (None, None);
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut value = || it.next().cloned().ok_or(format!("{a} needs a value"));
        match a.as_str() {
            "--out" => out = Some(PathBuf::from(value()?)),
            "--minutes" => minutes = Some(value()?.parse::<f64>().map_err(|e| e.to_string())?),
            "--csv-hz" => csv_hz = value()?.parse::<f64>().map_err(|e| e.to_string())?,
            "--map-name" => map = Some(value()?),
            "--event-name" => event = Some(value()?),
            other => return Err(format!("unknown argument {other}")),
        }
    }
    let names = match (map, event) {
        (Some(m), Some(e)) => Some((m, e)),
        (None, None) => None,
        _ => return Err("--map-name and --event-name go together".to_owned()),
    };
    if !csv_hz.is_finite() || csv_hz <= 0.0 || csv_hz > 1000.0 {
        return Err("--csv-hz must be above 0 and at most 1000".to_owned());
    }
    if minutes.is_some_and(|m| !m.is_finite() || m <= 0.0) {
        return Err("--minutes must be above 0".to_owned());
    }
    Ok(Config {
        out: out.ok_or("--out is required")?,
        minutes,
        csv_hz,
        names,
    })
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cfg = match parse(&args) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{e}\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    let mut probe = match Probe::new(cfg) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::from(2);
        }
    };
    let stop = win::install_ctrl_c();
    probe.run(stop);
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests;
