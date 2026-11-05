//! `iracing-replay`: writes the rows of an `.ibt` file, or of a synthetic session, into a test
//! memory mapping with a name of your choice, for running a reader without the simulator.
//!
//! The simulator's own names are refused: this tool never writes where the game would.

#![deny(unsafe_code)]

#[cfg(windows)]
fn main() -> std::process::ExitCode {
    app::run()
}

#[cfg(not(windows))]
fn main() -> std::process::ExitCode {
    eprintln!("iracing-replay writes a Windows memory mapping and runs on Windows only");
    std::process::ExitCode::from(2)
}

#[cfg(windows)]
mod app {
    use std::path::PathBuf;
    use std::process::ExitCode;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::{Duration, Instant};

    use irsdk::ibt::Ibt;
    use irsdk::testing::map::TestMap;
    use irsdk::testing::replay::{layout_of, replay};
    use irsdk::testing::synthetic_ibt;

    const USAGE: &str = "usage: iracing-replay --map-name <name> [--event-name <name>] \
(--ibt <file> | --synthetic <laps>) [--hz 60] [--lap-seconds 10] [--loop] [--stop-after <seconds>]";

    enum Source {
        Ibt(PathBuf),
        Synthetic(u32),
    }

    struct Config {
        map: String,
        event: String,
        source: Source,
        hz: f64,
        lap_seconds: f64,
        looping: bool,
        stop_after: Option<f64>,
    }

    fn number(a: &str, v: Option<&String>) -> Result<f64, String> {
        let v = v.ok_or(format!("{a} needs a value"))?;
        v.parse::<f64>()
            .ok()
            .filter(|n| n.is_finite() && *n > 0.0)
            .ok_or(format!("{a} must be a number above 0"))
    }

    fn parse(args: &[String]) -> Result<Config, String> {
        let (mut map, mut event, mut source) = (None, None, None);
        let (mut hz, mut lap_seconds, mut looping, mut stop_after) = (60.0, 10.0, false, None);
        let mut it = args.iter();
        while let Some(a) = it.next() {
            match a.as_str() {
                "--map-name" => map = Some(it.next().ok_or("--map-name needs a value")?.clone()),
                "--event-name" => {
                    event = Some(it.next().ok_or("--event-name needs a value")?.clone());
                }
                "--ibt" => {
                    let p = it.next().ok_or("--ibt needs a value")?;
                    source = Some(Source::Ibt(PathBuf::from(p)));
                }
                "--synthetic" => {
                    let laps = number(a, it.next())?;
                    source = Some(Source::Synthetic(laps.round().max(1.0) as u32));
                }
                "--hz" => hz = number(a, it.next())?,
                "--lap-seconds" => lap_seconds = number(a, it.next())?,
                "--stop-after" => stop_after = Some(number(a, it.next())?),
                "--loop" => looping = true,
                other => return Err(format!("unknown argument {other}")),
            }
        }
        let map = map.ok_or("--map-name is required")?;
        let event = event.unwrap_or_else(|| format!("{map}_event"));
        let source = source.ok_or("give --ibt <file> or --synthetic <laps>")?;
        Ok(Config {
            map,
            event,
            source,
            hz,
            lap_seconds,
            looping,
            stop_after,
        })
    }

    pub fn run() -> ExitCode {
        let args: Vec<String> = std::env::args().skip(1).collect();
        if args.iter().any(|a| a == "--help" || a == "-h") {
            println!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        let cfg = match parse(&args) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("{e}\n{USAGE}");
                return ExitCode::from(2);
            }
        };
        match play(&cfg) {
            Ok(rows) => {
                println!("replayed {rows} rows");
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("{e}");
                ExitCode::FAILURE
            }
        }
    }

    fn play(cfg: &Config) -> Result<usize, String> {
        let ibt = match &cfg.source {
            Source::Ibt(path) => Ibt::open(path).map_err(|e| format!("{}: {e}", path.display()))?,
            Source::Synthetic(laps) => {
                let per_lap = (cfg.lap_seconds * cfg.hz).round().max(2.0) as u32;
                synthetic_ibt(*laps, per_lap)
            }
        };
        let layout = layout_of(&ibt);
        let size = TestMap::required_size(&layout, ibt.yaml_raw().len());
        let mut map = TestMap::create(&cfg.map, &cfg.event, &layout, ibt.yaml_raw(), size)
            .map_err(|e| format!("cannot create the test mapping: {e}"))?;
        eprintln!(
            "mapping {} ready: {} rows at {} Hz{}",
            cfg.map,
            ibt.len(),
            cfg.hz,
            if cfg.looping { ", looping" } else { "" }
        );
        let stop = AtomicBool::new(false);
        let mut total = 0;
        std::thread::scope(|s| {
            if let Some(secs) = cfg.stop_after {
                let stop = &stop;
                s.spawn(move || {
                    let end = Instant::now() + Duration::from_secs_f64(secs);
                    while Instant::now() < end && !stop.load(Ordering::Relaxed) {
                        std::thread::sleep(Duration::from_millis(20));
                    }
                    stop.store(true, Ordering::Relaxed);
                });
            }
            loop {
                total += replay(&ibt, &mut map, cfg.hz, &stop);
                if !cfg.looping || stop.load(Ordering::Relaxed) {
                    break;
                }
            }
            // Release the timer thread when the replay ended first.
            stop.store(true, Ordering::Relaxed);
        });
        Ok(total)
    }
}
