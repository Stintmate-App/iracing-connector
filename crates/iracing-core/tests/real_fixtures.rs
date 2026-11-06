//! Tests that read recordings made on a machine with the simulator.
//!
//! Every test is ignored by default. Set `IRSDK_FIXTURES` to the folder `iracing-probe` wrote
//! and run `cargo test -p iracing-core --test real_fixtures -- --ignored --nocapture`. A test
//! prints a `SKIP` line naming what it needs when the variable is unset or the file is absent.
//! The folder holds:
//!
//! - `header.json`: the header fields of the live map (`buf_len` is read from it);
//! - `layout.bin`: the bytes of the live map from 0 to the end of the variable headers;
//! - `rows/<tick>.row.bin`: raw rows (a name with `-g` belongs to a later layout and is skipped);
//! - `session/NNN.yaml.bin`: the session text as the map held it, one file per change;
//! - `*.ibt` (optional): telemetry files, for the lap test.
//!
//! The recordings are real user data and are never committed.

use std::path::{Path, PathBuf};

use iracing_core::meta::{self, SessionMeta};
use iracing_core::tick::Tick;
use iracing_core::vars::Vars;
use irsdk::header::Header;
use irsdk::row::Row;
use irsdk::vars::VarTable;
use irsdk::worker::SessionDoc;

fn root() -> Option<PathBuf> {
    std::env::var_os("IRSDK_FIXTURES")
        .map(PathBuf::from)
        .filter(|p| p.is_dir())
}

/// Prints the SKIP line of a test and returns `true` when `ok` is false.
fn skip(test: &str, needs: &str, ok: bool) -> bool {
    if !ok {
        println!("SKIP {test}: needs {needs} (set IRSDK_FIXTURES to the probe's output folder)");
    }
    !ok
}

/// Files of `sub` ending in `suffix`, sorted by name.
fn files(root: &Path, sub: &str, suffix: &str) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(root.join(sub))
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.ends_with(suffix) && !n.contains("-g"))
        })
        .collect();
    v.sort();
    v
}

/// The variable table of the first layout and the length of one row.
fn layout(root: &Path) -> Option<(VarTable, usize)> {
    let bytes = std::fs::read(root.join("layout.bin")).ok()?;
    let json = std::fs::read(root.join("header.json")).ok()?;
    let json: serde_json::Value = serde_json::from_slice(&json).ok()?;
    let buf_len = usize::try_from(json.get("buf_len")?.as_i64()?).ok()?;
    // The dump holds only the start of the map: the offsets of the row buffers point past it.
    let header = Header::parse_in(&bytes, 1 << 30, false).expect("header");
    let table = VarTable::parse(&bytes, &header).expect("variable headers");
    Some((table, buf_len))
}

/// The ticks of the recorded rows, in file order.
fn ticks(root: &Path) -> Option<Vec<Tick>> {
    let (table, buf_len) = layout(root)?;
    let vars = Vars::resolve(&table).expect("the recorder's variables are in the layout");
    let rows: Vec<Tick> = files(root, "rows", ".row.bin")
        .iter()
        .map(|p| {
            let bytes = std::fs::read(p).unwrap();
            assert_eq!(bytes.len(), buf_len, "{}: row length", p.display());
            let mut t = Tick::new();
            t.fill(&vars, &Row::from_slice(&bytes));
            t.connected = true;
            *t
        })
        .collect();
    (!rows.is_empty()).then_some(rows)
}

/// The description built from each recorded session text.
fn metas(root: &Path) -> Vec<(PathBuf, SessionMeta)> {
    files(root, "session", ".yaml.bin")
        .into_iter()
        .enumerate()
        .map(|(i, p)| {
            let raw = std::fs::read(&p).unwrap();
            let doc = SessionDoc::from_raw(i as i32, &raw);
            let m = meta::build(&doc, 0);
            (p, m)
        })
        .collect()
}

#[test]
#[ignore = "needs the probe's recordings"]
fn real_rows_fill_ticks() {
    let rows = root().and_then(|r| ticks(&r));
    if skip(
        "real_rows_fill_ticks",
        "layout.bin, header.json and rows/*.row.bin",
        rows.is_some(),
    ) {
        return;
    }
    let rows = rows.unwrap();
    let mut by_time = rows.iter().collect::<Vec<_>>();
    by_time.sort_by_key(|t| t.tick);
    // Offline, every session is numbered 0 and the session time restarts with it, so a fall
    // with an unchanged `SessionNum` is counted, not refused.
    let falls = by_time
        .windows(2)
        .filter(|p| p[1].session_time < p[0].session_time)
        .count();
    println!("SessionTime fell {falls} times between consecutive rows");
    for t in by_time {
        assert!((0.0..120.0).contains(&t.speed), "Speed {}", t.speed);
        assert!(
            (-1.001..=1.001).contains(&t.lap_dist_pct),
            "LapDistPct {}",
            t.lap_dist_pct
        );
    }
    println!("{} rows read", rows.len());
}

#[test]
#[ignore = "needs the probe's recordings"]
fn real_meta() {
    let metas = root().map(|r| metas(&r)).filter(|m| !m.is_empty());
    if skip("real_meta", "session/*.yaml.bin", metas.is_some()) {
        return;
    }
    for (path, m) in metas.unwrap() {
        let name = path.display();
        assert!(m.parse_errors.is_empty(), "{name}: {:?}", m.parse_errors);
        assert!(!m.track.track_id.is_empty(), "{name}: track id");
        assert!(!m.track.layout_id.is_empty(), "{name}: layout id");
        assert!(m.track.length_m.is_some_and(|l| l > 0.0), "{name}: length");
        let player = m.player.as_ref().expect("the player's car");
        assert!(!player.car_id.is_empty(), "{name}: car id");
        assert!(!player.car_name.is_empty(), "{name}: car name");
        assert!(
            m.fuel_capacity_l.is_some_and(|f| f > 0.0),
            "{name}: fuel capacity"
        );
        assert!(!m.sector_starts.is_empty(), "{name}: sectors");
    }
}

#[test]
#[ignore = "needs the probe's recordings"]
fn real_setup() {
    let last = root()
        .map(|r| metas(&r))
        .and_then(|m| m.into_iter().next_back());
    if skip("real_setup", "session/*.yaml.bin", last.is_some()) {
        return;
    }
    let (path, m) = last.unwrap();
    let snap = m
        .setup
        .as_ref()
        .unwrap_or_else(|| panic!("{}: no CarSetup", path.display()));
    let setup = snap.to_setup("session_start", "2026-01-01T00:00:00+00:00", None);
    // The pressures and the fuel are typed fields; a car without brake bias leaves it out.
    for (name, p) in [
        ("FL", setup.pressure_fl_kpa),
        ("FR", setup.pressure_fr_kpa),
        ("RL", setup.pressure_rl_kpa),
        ("RR", setup.pressure_rr_kpa),
    ] {
        assert!(p.is_some_and(|v| v > 50.0), "cold pressure {name}: {p:?}");
    }
    assert!(setup.fuel_l.is_some_and(|f| f > 0.0), "fuel");
    println!(
        "setup: brake bias {:?}, compounds {:?}/{:?}, fuel {:?}",
        setup.brake_bias_front_pct, setup.compound_front, setup.compound_rear, setup.fuel_l
    );
}

#[test]
#[ignore = "needs the probe's recordings"]
fn real_signs() {
    let rows = root().and_then(|r| ticks(&r));
    if skip("real_signs", "rows/*.row.bin", rows.is_some()) {
        return;
    }
    let rows = rows.unwrap();
    let corners: Vec<_> = rows
        .iter()
        .filter(|t| t.speed > 15.0 && t.lat_accel.abs() > 6.0 && t.steer_rad.abs() > 0.2)
        .collect();
    if corners.is_empty() {
        println!(
            "SKIP real_signs: no cornering row among the {} rows",
            rows.len()
        );
        return;
    }
    // Observed: `LatAccel` and `SteeringWheelAngle` are both above 0 to the left.
    let agree = corners
        .iter()
        .filter(|t| t.lat_accel * t.steer_rad > 0.0)
        .count();
    println!(
        "cornering rows: {}, steer agrees with lateral acceleration in {agree}",
        corners.len()
    );
    assert!(
        agree * 10 >= corners.len() * 8,
        "steer and LatAccel disagree"
    );
    // The channel is positive on a right-hand corner: g_lat = -LatAccel / g.
    assert!(
        corners
            .iter()
            .filter(|t| t.lat_accel < 0.0)
            .all(|t| -t.lat_accel > 0.0)
    );
    let braking: Vec<_> = rows
        .iter()
        .filter(|t| t.brake > 0.5 && t.speed > 15.0)
        .collect();
    if braking.is_empty() {
        println!(
            "SKIP real_signs (braking): no braking row among the {} rows",
            rows.len()
        );
        return;
    }
    let slowing = braking.iter().filter(|t| t.long_accel < 0.0).count();
    println!(
        "braking rows: {}, LongAccel below 0 in {slowing}",
        braking.len()
    );
    assert!(
        slowing * 10 >= braking.len() * 8,
        "braking should give LongAccel < 0"
    );
}

#[test]
#[ignore = "needs the probe's recordings"]
fn real_ibt_laps() {
    let ibts = root().map(|r| files(&r, "", ".ibt")).unwrap_or_default();
    if skip("real_ibt_laps", "at least one *.ibt", !ibts.is_empty()) {
        return;
    }
    for path in ibts {
        let ibt =
            irsdk::ibt::Ibt::open(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        let vars = Vars::resolve(ibt.vars()).expect("the recorder's variables are in the file");
        let mut t = Tick::new();
        let mut crossings = 0;
        let mut last = f32::NAN;
        for bytes in ibt.rows() {
            t.fill(&vars, &Row::from_slice(bytes));
            if last > 0.9 && t.lap_dist_pct < 0.1 {
                crossings += 1;
            }
            last = t.lap_dist_pct;
        }
        let declared = ibt.disk().session_lap_count;
        println!(
            "{}: {crossings} crossings, {declared} laps declared",
            path.display()
        );
        assert!(
            (i64::from(declared) - i64::from(crossings)).abs() <= 1,
            "crossings and declared laps differ by more than one"
        );
    }
}
