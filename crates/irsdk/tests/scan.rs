//! `Scan` against `Ibt` on built files.

use std::io::Cursor;

use irsdk::defs::VarType;
use irsdk::header::DiskHeader;
use irsdk::ibt::{Ibt, IbtError};
use irsdk::row::Row;
use irsdk::scan::{CHUNK_BYTES, Scan};
use irsdk::testing::{LayoutBuilder, RowBuilder, ibt_bytes};
use irsdk::vars::{Var, VarError};

const YAML: &str = "---\nWeekendInfo:\n TrackName: scan\n...\n";

fn layout() -> irsdk::testing::Layout {
    LayoutBuilder::new()
        .tick_rate(60)
        .var("Speed", VarType::Float, 1, "m/s", "")
        .var("Lat", VarType::Double, 1, "deg", "")
        .var("Lap", VarType::Int, 1, "", "")
        .var("Pad", VarType::Float, 40, "", "")
        .build()
}

fn rows(l: &irsdk::testing::Layout, n: i32) -> Vec<Vec<u8>> {
    (0..n)
        .map(|i| {
            let mut b = RowBuilder::new(l);
            b.set_f32("Speed", i as f32 * 0.5)
                .set_f64("Lat", 53.0 + f64::from(i) * 1e-6)
                .set_i32("Lap", i / 7);
            b.into_bytes()
        })
        .collect()
}

fn disk(count: i32) -> DiskHeader {
    DiskHeader {
        session_start_date: 1_700_000_000,
        session_start_time: 0.0,
        session_end_time: 10.0,
        session_lap_count: 1,
        session_record_count: count,
    }
}

fn scan(bytes: &[u8]) -> Scan<Cursor<Vec<u8>>> {
    Scan::new(Cursor::new(bytes.to_vec()), bytes.len() as u64).unwrap()
}

#[test]
fn scan_yields_the_same_values_as_ibt_row_by_row() {
    let l = layout();
    // More rows than one chunk holds: the chunk boundary is crossed.
    let per_chunk = CHUNK_BYTES / l.buf_len();
    let n = (per_chunk * 2 + 17) as i32;
    let bytes = ibt_bytes(&l, YAML, &rows(&l, n), disk(n));
    let ibt = Ibt::from_bytes(bytes.clone()).unwrap();
    let speed: Var<f32> = ibt.vars().find("Speed").unwrap();
    let lat: Var<f64> = ibt.vars().find("Lat").unwrap();
    let lap: Var<i32> = ibt.vars().find("Lap").unwrap();

    let mut s = scan(&bytes);
    assert!(s.is_complete());
    assert_eq!(s.rows(), n as u64);
    assert_eq!(s.row_len(), ibt.row_len());
    assert_eq!(s.disk(), ibt.disk());
    assert_eq!(s.header(), ibt.header());
    assert!(s.session_text().text.contains("TrackName: scan"));
    let (fs, fl, fp) = (
        s.field::<f32>("Speed").unwrap(),
        s.field::<f64>("Lat").unwrap(),
        s.field::<i32>("Lap").unwrap(),
    );
    let mut seen = 0usize;
    let mut chunks = 0;
    while let Some(c) = s.next_chunk().unwrap() {
        assert_eq!(c.first_row(), seen as u64);
        assert_eq!(c.rows().count(), c.len());
        for r in c.rows() {
            let row = Row::from_slice(ibt.row(seen).unwrap());
            assert_eq!(fs.get(r), speed.read(&row));
            assert_eq!(fl.get(r), lat.read(&row));
            assert_eq!(fp.get(r), lap.read(&row));
            seen += 1;
        }
        chunks += 1;
    }
    assert_eq!(seen, n as usize);
    assert_eq!(chunks, 3);
}

#[test]
fn a_file_whose_size_does_not_match_its_count_is_incomplete() {
    let l = layout();
    let ok = ibt_bytes(&l, YAML, &rows(&l, 10), disk(10));
    assert!(scan(&ok).is_complete());
    // Rows missing (the count promises more than the file holds).
    let short = ibt_bytes(&l, YAML, &rows(&l, 8), disk(10));
    let s = scan(&short);
    assert!(!s.is_complete());
    assert_eq!(s.rows(), 8);
    // Trailing bytes after the last row.
    let mut long = ok.clone();
    long.extend_from_slice(&[0; 5]);
    assert!(!scan(&long).is_complete());
    // A record count of zero: rows that fit are read, but the file is not complete.
    let zero = ibt_bytes(&l, YAML, &rows(&l, 5), disk(0));
    let s = scan(&zero);
    assert!(!s.is_complete());
    assert_eq!(s.rows(), 5);
}

#[test]
fn errors_are_reported_not_panics() {
    let l = layout();
    let none = ibt_bytes(&l, YAML, &[], disk(0));
    assert!(matches!(
        Scan::new(Cursor::new(none.clone()), none.len() as u64),
        Err(IbtError::NoRows)
    ));
    let ok = ibt_bytes(&l, YAML, &rows(&l, 3), disk(3));
    for cut in [0usize, 50, 143, 200] {
        let r = Scan::new(Cursor::new(ok[..cut].to_vec()), cut as u64);
        assert!(r.is_err(), "cut at {cut}");
    }
    // The file ends in the middle of a chunk: the read fails.
    let mut s = Scan::new(Cursor::new(ok[..ok.len() - 3].to_vec()), ok.len() as u64).unwrap();
    assert!(s.next_chunk().is_err());
}

#[test]
fn fields_check_name_type_and_count() {
    let l = layout();
    let bytes = ibt_bytes(&l, YAML, &rows(&l, 2), disk(2));
    let s = scan(&bytes);
    assert!(matches!(s.field::<f32>("Nope"), Err(VarError::Missing)));
    assert!(matches!(
        s.field::<i32>("Speed"),
        Err(VarError::WrongType { .. })
    ));
    assert!(matches!(
        s.field::<f32>("Pad"),
        Err(VarError::NotScalar { count: 40 })
    ));
}

#[test]
fn open_reads_a_file_from_disk() {
    let l = layout();
    let dir = std::env::temp_dir().join(format!("irsdk_scan_test_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("a.ibt");
    std::fs::write(&path, ibt_bytes(&l, YAML, &rows(&l, 4), disk(4))).unwrap();
    let mut s = Scan::open(&path).unwrap();
    assert!(s.is_complete());
    assert_eq!(s.next_chunk().unwrap().unwrap().len(), 4);
    assert!(s.next_chunk().unwrap().is_none());
    assert!(matches!(
        Scan::open(&dir.join("missing.ibt")),
        Err(IbtError::Io(_))
    ));
    std::fs::remove_dir_all(&dir).unwrap();
}
