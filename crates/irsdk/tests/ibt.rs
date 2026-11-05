//! Round trips of synthetic `.ibt` images through `Ibt`.

use irsdk::defs::VarType;
use irsdk::header::{DISK_HEADER_END, DiskHeader};
use irsdk::ibt::{Ibt, IbtError};
use irsdk::row::Row;
use irsdk::testing::{LayoutBuilder, RowBuilder, ibt_bytes, synthetic_ibt};
use irsdk::vars::Var;

fn layout() -> irsdk::testing::Layout {
    LayoutBuilder::new()
        .tick_rate(60)
        .var("Speed", VarType::Float, 1, "m/s", "")
        .var("Lap", VarType::Int, 1, "", "")
        .build()
}

fn rows(l: &irsdk::testing::Layout, n: i32) -> Vec<Vec<u8>> {
    (0..n)
        .map(|i| {
            let mut b = RowBuilder::new(l);
            b.set_f32("Speed", i as f32 * 2.0).set_i32("Lap", i);
            b.into_bytes()
        })
        .collect()
}

fn disk(count: i32) -> DiskHeader {
    DiskHeader {
        session_start_date: 1_700_000_123,
        session_start_time: 12.5,
        session_end_time: 99.25,
        session_lap_count: 4,
        session_record_count: count,
    }
}

const YAML: &str = "---\nWeekendInfo:\n TrackName: roundtrip\n TrackLength: 3.00 km\n...\n";

#[test]
fn round_trip_header_disk_rows_and_yaml() {
    let l = layout();
    let r = rows(&l, 10);
    let ibt = Ibt::from_bytes(ibt_bytes(&l, YAML, &r, disk(10))).unwrap();
    assert_eq!(ibt.header().tick_rate, 60);
    assert_eq!(ibt.header().num_vars, 2);
    assert_eq!(ibt.header().num_buf, 1);
    assert_eq!(ibt.header().buf_len as usize, l.buf_len());
    assert_eq!(*ibt.disk(), disk(10));
    assert_eq!(ibt.len(), 10);
    assert_eq!(ibt.row_len(), l.buf_len());
    assert_eq!(ibt.yaml_raw(), YAML.as_bytes());
    assert!(ibt.session_text().text.contains("roundtrip"));
    let w = ibt.session().info.weekend.expect("weekend");
    assert_eq!(w.track_name.as_deref(), Some("roundtrip"));
    assert_eq!(w.track_length_m(), Some(3000.0));
    for (i, row) in ibt.rows().enumerate() {
        assert_eq!(row, r[i].as_slice());
        assert_eq!(ibt.row(i), Some(row));
    }
    assert_eq!(ibt.rows().count(), 10);
    assert!(ibt.row(10).is_none());
}

#[test]
fn variables_read_through_the_table() {
    let l = layout();
    let ibt = Ibt::from_bytes(ibt_bytes(&l, YAML, &rows(&l, 5), disk(5))).unwrap();
    let speed: Var<f32> = ibt.vars().find("Speed").unwrap();
    let lap: Var<i32> = ibt.vars().find("Lap").unwrap();
    let row = Row::from_slice(ibt.row(3).unwrap());
    assert_eq!(speed.read(&row), 6.0);
    assert_eq!(lap.read(&row), 3);
    assert_eq!(ibt.vars().get("Speed").unwrap().unit, "m/s");
}

#[test]
fn rows_start_after_the_headers() {
    let l = layout();
    let bytes = ibt_bytes(&l, YAML, &rows(&l, 2), disk(2));
    let ibt = Ibt::from_bytes(bytes.clone()).unwrap();
    let at = ibt.header().var_buf[0].buf_offset as usize;
    assert!(at >= DISK_HEADER_END + 2 * 144 + YAML.len());
    assert_eq!(&bytes[at..at + ibt.row_len()], ibt.row(0).unwrap());
}

#[test]
fn record_count_is_clamped_to_the_rows_in_the_file() {
    let l = layout();
    let ibt = Ibt::from_bytes(ibt_bytes(&l, YAML, &rows(&l, 6), disk(1000))).unwrap();
    assert_eq!(ibt.len(), 6);
    let ibt = Ibt::from_bytes(ibt_bytes(&l, YAML, &rows(&l, 6), disk(4))).unwrap();
    assert_eq!(ibt.len(), 4);
    assert_eq!(ibt.rows().count(), 4);
}

#[test]
fn a_record_count_of_zero_uses_what_fits() {
    let l = layout();
    let ibt = Ibt::from_bytes(ibt_bytes(&l, YAML, &rows(&l, 7), disk(0))).unwrap();
    assert_eq!(ibt.len(), 7);
}

#[test]
fn a_truncated_file_keeps_its_whole_rows() {
    let l = layout();
    let mut bytes = ibt_bytes(&l, YAML, &rows(&l, 8), disk(8));
    bytes.truncate(bytes.len() - l.buf_len() - 3);
    let ibt = Ibt::from_bytes(bytes).unwrap();
    assert_eq!(ibt.len(), 6);
}

#[test]
fn a_file_without_rows_is_refused() {
    let l = layout();
    let r = Ibt::from_bytes(ibt_bytes(&l, YAML, &[], disk(0)));
    assert!(matches!(r, Err(IbtError::NoRows)));
}

#[test]
fn cut_headers_are_layout_errors() {
    let l = layout();
    let bytes = ibt_bytes(&l, YAML, &rows(&l, 3), disk(3));
    for cut in [0, 50, 111, 112, 143, 144, 200] {
        let r = Ibt::from_bytes(bytes[..cut].to_vec());
        assert!(matches!(r, Err(IbtError::Layout(_))), "cut at {cut}: {r:?}");
    }
}

#[test]
fn a_wrong_version_is_refused() {
    let l = layout();
    let mut bytes = ibt_bytes(&l, YAML, &rows(&l, 3), disk(3));
    bytes[0] = 9;
    assert!(matches!(Ibt::from_bytes(bytes), Err(IbtError::Layout(_))));
}

#[test]
fn open_reads_a_file() {
    let l = layout();
    let dir = std::env::temp_dir().join(format!("irsdk_ibt_test_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("a.ibt");
    std::fs::write(&path, ibt_bytes(&l, YAML, &rows(&l, 4), disk(4))).unwrap();
    let ibt = Ibt::open(&path).unwrap();
    assert_eq!(ibt.len(), 4);
    assert!(matches!(
        Ibt::open(&dir.join("missing.ibt")),
        Err(IbtError::Io(_))
    ));
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn the_synthetic_file_has_laps_and_rising_distance() {
    let ibt = synthetic_ibt(3, 50);
    assert_eq!(ibt.len(), 150);
    assert_eq!(ibt.disk().session_lap_count, 3);
    let pct: Var<f32> = ibt.vars().find("LapDistPct").unwrap();
    let lap: Var<i32> = ibt.vars().find("Lap").unwrap();
    let first = Row::from_slice(ibt.row(0).unwrap());
    let last = Row::from_slice(ibt.row(149).unwrap());
    assert_eq!(pct.read(&first), 0.0);
    assert_eq!(lap.read(&first), 1);
    assert!(pct.read(&last) > 0.9);
    assert_eq!(lap.read(&last), 3);
}
