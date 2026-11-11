//! Tests that read recordings made on a machine with the simulator.
//!
//! The recordings are not in the repository yet, so every test is ignored by default and prints
//! a `SKIP` line naming what it needs when the file is absent. Run them with
//! `cargo test -p irsdk -- --ignored --nocapture`. They look in `tests/fixtures/real/`:
//!
//! - `*.ibt`: telemetry files;
//! - `var_table.golden.txt`: one line per variable of the first `.ibt`, tab-separated
//!   `name`, `type` (as in `VarType`'s `Debug`), `count`, `unit`;
//! - `live_header.bin`: the first bytes of the live map (the header and the variable headers);
//! - `session/*.yaml`: raw session texts as the map holds them (NUL padding allowed);
//! - `tick_rate.txt`: the tick rate the live header should say, as a single integer.

use std::path::PathBuf;

use irsdk::header::Header;
use irsdk::ibt::Ibt;
use irsdk::vars::VarTable;
use irsdk::yaml::{self, decode};

fn dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/real")
}

/// Files of `sub` (relative to the fixtures folder) with the extension `ext`, sorted.
fn files(sub: &str, ext: &str) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir().join(sub))
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == ext))
        .collect();
    v.sort();
    v
}

/// Prints the SKIP line of a test and returns `true` when `found` is empty.
fn skip(test: &str, needs: &str, found: bool) -> bool {
    if !found {
        println!("SKIP {test}: needs {needs} in tests/fixtures/real/");
    }
    !found
}

fn ibts() -> Vec<Ibt> {
    files("", "ibt")
        .iter()
        .map(|p| Ibt::open(p).unwrap_or_else(|e| panic!("{}: {e}", p.display())))
        .collect()
}

#[test]
#[ignore = "needs recordings from the simulator"]
fn real_ibt_reads_every_row() {
    let ibts = ibts();
    if skip(
        "real_ibt_reads_every_row",
        "at least one *.ibt",
        !ibts.is_empty(),
    ) {
        return;
    }
    for ibt in &ibts {
        let declared = ibt.disk().session_record_count;
        if declared > 0 {
            assert_eq!(ibt.len(), declared as usize, "row count");
        }
        let tick = ibt.vars().find::<i32>("SessionTick");
        let speed = ibt.vars().find::<f32>("Speed").expect("Speed");
        let pct = ibt.vars().find::<f32>("LapDistPct").expect("LapDistPct");
        let mut last = i32::MIN;
        for (i, bytes) in ibt.rows().enumerate() {
            let row = irsdk::row::Row::from_slice(bytes);
            if let Ok(t) = &tick {
                let now = t.read(&row);
                assert!(now >= last, "SessionTick fell at row {i}");
                last = now;
            }
            let s = speed.read(&row);
            assert!((0.0..120.0).contains(&s), "Speed {s} at row {i}");
            let p = pct.read(&row);
            assert!((-0.001..=1.001).contains(&p), "LapDistPct {p} at row {i}");
        }
    }
}

#[test]
#[ignore = "needs recordings from the simulator"]
fn real_var_table_golden() {
    let golden = std::fs::read_to_string(dir().join("var_table.golden.txt")).ok();
    let ibts = ibts();
    if skip(
        "real_var_table_golden",
        "var_table.golden.txt and a *.ibt",
        golden.is_some() && !ibts.is_empty(),
    ) {
        return;
    }
    let want: Vec<String> = golden
        .unwrap()
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(str::to_owned)
        .collect();
    let got: Vec<String> = ibts[0]
        .vars()
        .iter()
        .map(|v| format!("{}\t{:?}\t{}\t{}", v.name, v.ty, v.count, v.unit))
        .collect();
    assert_eq!(got, want);
}

#[test]
#[ignore = "needs recordings from the simulator"]
fn real_live_header_parses() {
    let bytes = std::fs::read(dir().join("live_header.bin")).ok();
    if skip(
        "real_live_header_parses",
        "live_header.bin",
        bytes.is_some(),
    ) {
        return;
    }
    let bytes = bytes.unwrap();
    // The dump holds only the start of the map: the offsets point past it.
    let h = Header::parse_in(&bytes, 1 << 30, false).expect("header");
    assert_eq!(h.ver, 2);
    let table = VarTable::parse(&bytes, &h).expect("variable headers");
    assert_eq!(table.len(), h.num_vars as usize - table.skipped().len());
    assert!(table.get("SessionTime").is_some());
    println!(
        "live header: tick_rate {}, {} variables, row {} bytes, {} skipped",
        h.tick_rate,
        table.len(),
        h.buf_len,
        table.skipped().len()
    );
}

#[test]
#[ignore = "needs recordings from the simulator"]
fn real_yaml_parses_typed() {
    let texts = files("session", "yaml");
    if skip(
        "real_yaml_parses_typed",
        "session/*.yaml",
        !texts.is_empty(),
    ) {
        return;
    }
    for path in texts {
        let raw = std::fs::read(&path).unwrap();
        let d = decode(&raw);
        let p = yaml::parse(&d.text);
        let name = path.display();
        assert!(p.errors.is_empty(), "{name}: {:?}", p.errors);
        let w = p.info.weekend.as_ref().expect("WeekendInfo");
        assert!(
            w.track_length_m().is_some_and(|m| m > 0.0),
            "{name}: track length"
        );
        let driver = p.info.driver.as_ref().expect("DriverInfo");
        assert!(driver.player().is_some(), "{name}: player");
        assert!(driver.fuel_capacity_l().is_some(), "{name}: fuel capacity");
        println!(
            "{name}: utf8 {}, {} replaced, track {:?}",
            d.utf8, d.replaced, w.track_name
        );
    }
}

#[test]
#[ignore = "needs recordings from the simulator"]
fn real_ibt_replay_matches() {
    let ibts = ibts();
    if skip(
        "real_ibt_replay_matches",
        "at least one *.ibt",
        !ibts.is_empty(),
    ) {
        return;
    }
    replay_matches(&ibts[0]);
}

#[cfg(windows)]
fn replay_matches(ibt: &Ibt) {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    use irsdk::live::Live;
    use irsdk::sampler::Poll;
    use irsdk::testing::map::TestMap;
    use irsdk::testing::replay::{layout_of, replay};

    let id = std::process::id();
    let (m, e) = (
        format!(r"Local\StintmateRealReplayMap_{id}"),
        format!(r"Local\StintmateRealReplayEvent_{id}"),
    );
    let layout = layout_of(ibt);
    let size = TestMap::required_size(&layout, ibt.yaml_raw().len());
    let mut map = TestMap::create(&m, &e, &layout, ibt.yaml_raw(), size).unwrap();
    let mut live = Live::open_named(&m, &e).unwrap();
    let stop = AtomicBool::new(false);
    // The first rows are enough: every row that arrives must be a row of the file, in order.
    let wanted = ibt.len().min(600);
    let (mut seen, mut next) = (0, 0);
    std::thread::scope(|s| {
        let h = s.spawn(|| replay(ibt, &mut map, 300.0, &stop));
        while seen < wanted {
            match live.wait(Duration::from_millis(500)) {
                Poll::Row => {
                    seen += 1;
                    let got = live.sampler().row().bytes();
                    let at = (next..ibt.len())
                        .find(|&i| ibt.row(i).unwrap() == got)
                        .expect("a row that is not in the file, or out of order");
                    next = at + 1;
                }
                Poll::NoNewRow if h.is_finished() => break,
                _ => {}
            }
        }
        stop.store(true, Ordering::SeqCst);
        h.join().unwrap();
    });
    assert!(seen > 0, "no row arrived");
}

#[cfg(not(windows))]
fn replay_matches(_ibt: &Ibt) {
    println!("SKIP real_ibt_replay_matches: needs Windows (the test mapping)");
}

#[test]
#[ignore = "needs recordings from the simulator"]
fn real_tick_rate() {
    let bytes = std::fs::read(dir().join("live_header.bin")).ok();
    let want = std::fs::read_to_string(dir().join("tick_rate.txt")).ok();
    if skip(
        "real_tick_rate",
        "live_header.bin and tick_rate.txt",
        bytes.is_some() && want.is_some(),
    ) {
        return;
    }
    let h = Header::parse_in(&bytes.unwrap(), 1 << 30, false).expect("header");
    let want: i32 = want
        .unwrap()
        .trim()
        .parse()
        .expect("an integer in tick_rate.txt");
    assert_eq!(h.tick_rate, want);
    let every = (h.tick_rate / 60).max(1);
    println!("tick_rate {}: one row in {every} is kept", h.tick_rate);
    assert!(every >= 1);
}

#[test]
#[ignore = "needs recordings from the simulator"]
fn real_scan_matches_ibt_on_every_file() {
    let paths = files("", "ibt");
    if skip(
        "real_scan_matches_ibt_on_every_file",
        "at least one *.ibt",
        !paths.is_empty(),
    ) {
        return;
    }
    for p in &paths {
        let ibt = Ibt::open(p).unwrap();
        let mut scan = irsdk::scan::Scan::open(p).unwrap();
        assert_eq!(scan.rows(), ibt.len() as u64, "{}", p.display());
        assert_eq!(scan.disk(), ibt.disk());
        let pct = scan.field::<f32>("LapDistPct").unwrap();
        let pct_ibt: irsdk::vars::Var<f32> = ibt.vars().find("LapDistPct").unwrap();
        let mut i = 0;
        while let Some(c) = scan.next_chunk().unwrap() {
            for r in c.rows() {
                let row = irsdk::row::Row::from_slice(ibt.row(i).unwrap());
                assert_eq!(pct.get(r), pct_ibt.read(&row));
                i += 1;
            }
        }
        assert_eq!(i, ibt.len());
    }
}
