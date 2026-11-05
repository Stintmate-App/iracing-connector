//! Tests over synthetic maps with unique names.

use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::SystemTime;

use irsdk::defs::VarType;
use irsdk::testing::map::TestMap;
use irsdk::testing::{Layout, LayoutBuilder, RowBuilder};

use crate::probe::{Config, Probe};
use crate::util::{cp1252, iso, under_sim_documents};

static COUNTER: AtomicU32 = AtomicU32::new(0);
const YAML: &[u8] = b"---\nWeekendInfo:\n Encoding: UTF8\n TrackName: caf\xc3\xa9\n";

fn names() -> (String, String) {
    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    let id = format!("{}_{n}", std::process::id());
    (
        format!("Local\\StintmateProbeMap_{id}"),
        format!("Local\\StintmateProbeEvent_{id}"),
    )
}

fn layout(extra: bool) -> Layout {
    let mut b = LayoutBuilder::new()
        .var("Speed", VarType::Float, 1, "m/s", "Speed")
        .var("Lap", VarType::Int, 1, "", "Lap")
        .var("SessionFlags", VarType::BitField, 1, "", "Flags")
        .var("IsOnTrack", VarType::Bool, 1, "", "On track")
        .var("PlayerCarIdx", VarType::Int, 1, "", "Player")
        .var("LFtempCL", VarType::Float, 1, "C", "Tyre")
        .var("CarIdxLapDistPct", VarType::Float, 64, "%", "Lap pct")
        .var("Other", VarType::Float, 4, "", "Not written");
    if extra {
        b = b.var("Extra", VarType::Int, 1, "", "Only in the second layout");
    }
    b.build()
}

fn row(l: &Layout, speed: f32) -> Vec<u8> {
    let mut r = RowBuilder::new(l);
    r.set_f32("Speed", speed)
        .set_i32("Lap", 3)
        .set_u32("SessionFlags", 0x4)
        .set_bool("IsOnTrack", true)
        .set_i32("PlayerCarIdx", 5)
        .set_f32_at("CarIdxLapDistPct", 5, 0.25);
    r.into_bytes()
}

fn start(out: &Path, l: &Layout) -> (TestMap, Probe) {
    let (m, e) = names();
    let map = TestMap::create(&m, &e, l, YAML, 256 * 1024).unwrap();
    let probe = Probe::new(Config {
        out: out.to_path_buf(),
        minutes: None,
        csv_hz: 1000.0,
        names: Some((m, e)),
    })
    .unwrap();
    (map, probe)
}

#[test]
fn writes_the_files_for_a_synthetic_map() {
    let tmp = tempfile::tempdir().unwrap();
    let l = layout(false);
    let (mut map, mut probe) = start(tmp.path(), &l);
    probe.step(); // attaches
    for i in 0..3 {
        map.push_row(&row(&l, i as f32));
        probe.step();
    }
    let d = probe.dir().to_path_buf();
    for f in [
        "README.txt",
        "header.json",
        "vars.json",
        "vars.csv",
        "layout.bin",
        "samples.csv",
        "windows.jsonl",
        "session/001.yaml.bin",
        "session/001.cp1252.yaml",
        "session/001.meta.json",
    ] {
        assert!(d.join(f).exists(), "{f} missing");
    }
    assert_eq!(fs::read_dir(d.join("rows")).unwrap().count(), 1);
    let meta = fs::read_to_string(d.join("session/001.meta.json")).unwrap();
    assert!(meta.contains("\"starts_with_utf8_marker\":true"), "{meta}");
    assert!(meta.contains("\"valid_utf8\":true"));
    assert_eq!(fs::read(d.join("session/001.yaml.bin")).unwrap(), YAML);
    let header = fs::read_to_string(d.join("header.json")).unwrap();
    assert!(header.contains("\"tick_rate\":60") && header.contains("\"num_buf\":3"));
    let vars = fs::read_to_string(d.join("vars.csv")).unwrap();
    assert!(vars.contains("CarIdxLapDistPct,Float,64,"));

    let csv = fs::read_to_string(d.join("samples.csv")).unwrap();
    let mut lines = csv.lines();
    let head: Vec<&str> = lines.next().unwrap().split(',').collect();
    let first: Vec<&str> = lines.next().unwrap().split(',').collect();
    assert_eq!(head.len(), first.len());
    let col = |n: &str| {
        head.iter()
            .position(|h| *h == n)
            .unwrap_or_else(|| panic!("{n}"))
    };
    assert_eq!(first[col("IsOnTrack")], "1");
    assert_eq!(first[col("SessionFlags")], "0x00000004");
    assert_eq!(first[col("CarIdxLapDistPct[player]")], "0.25");
    assert_eq!(first[col("CarIdxLapDistPct[0]")], "0");
    assert!(head.contains(&"CarIdxLapDistPct[7]") && !head.contains(&"CarIdxLapDistPct[8]"));
    assert!(!head.iter().any(|h| h.starts_with("Other")));

    probe.finish();
    let stats = fs::read_to_string(d.join("stats.json")).unwrap();
    assert!(stats.contains("\"rows_seen\":3"), "{stats}");
}

#[test]
fn a_session_change_writes_a_new_yaml_file() {
    let tmp = tempfile::tempdir().unwrap();
    let l = layout(false);
    let (mut map, mut probe) = start(tmp.path(), &l);
    probe.step();
    map.push_row(&row(&l, 1.0));
    probe.step();
    let second = b"---\nWeekendInfo:\n Encoding: UTF8\n TrackName: other\n";
    map.set_yaml(second);
    map.push_row(&row(&l, 2.0));
    probe.step();
    let d = probe.dir();
    assert_eq!(fs::read(d.join("session/002.yaml.bin")).unwrap(), second);
    assert!(!d.join("session/003.yaml.bin").exists());
    // A session change dumps a row too.
    assert_eq!(fs::read_dir(d.join("rows")).unwrap().count(), 2);
}

#[test]
fn a_reset_starts_a_new_samples_file() {
    let tmp = tempfile::tempdir().unwrap();
    let l = layout(false);
    let (mut map, mut probe) = start(tmp.path(), &l);
    probe.step();
    map.push_row(&row(&l, 1.0));
    probe.step();
    let l2 = layout(true);
    map.restart(&l2, YAML);
    probe.step(); // the reset
    map.push_row(&row(&l2, 2.0));
    probe.step();
    let d = probe.dir();
    assert!(d.join("samples.csv").exists());
    let second = fs::read_to_string(d.join("samples-2.csv")).unwrap();
    assert!(second.lines().next().unwrap().contains(",Extra"));
    assert!(d.join("vars-2.json").exists() && d.join("layout-2.bin").exists());
    let first = fs::read_to_string(d.join("samples.csv")).unwrap();
    assert!(!first.lines().next().unwrap().contains("Extra"));
}

#[test]
fn refuses_the_simulator_documents_folder() {
    let bad = Path::new(r"C:\Users\someone\Documents\iRacing\telemetry");
    assert!(under_sim_documents(bad));
    assert!(under_sim_documents(Path::new(r"d:\x\documents\IRACING")));
    assert!(!under_sim_documents(Path::new(
        r"C:\Users\someone\Documents\Probe"
    )));
    let cfg = Config {
        out: bad.to_path_buf(),
        minutes: None,
        csv_hz: 1.0,
        names: None,
    };
    assert!(Probe::new(cfg).is_err());
}

#[test]
fn text_helpers() {
    assert_eq!(cp1252(b"a\x80\xe9"), "a\u{20AC}\u{e9}");
    assert_eq!(iso(SystemTime::UNIX_EPOCH), "1970-01-01T00:00:00.000Z");
}
