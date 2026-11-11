//! The positions job over disk files built from rows copied from real ones.
//!
//! Every row comes from `positions/rows.rs`, slices of real disk telemetry; the files and the
//! recordings of the laps are built from them. Positions are checked against an independent
//! projection written out in this file.

#[path = "positions/rows.rs"]
mod rows;

use std::cell::Cell;
use std::io::Cursor;
use std::sync::atomic::{AtomicBool, Ordering};

use iracing_core::positions::file::{
    LapRecord, PositionsFile, PositionsHeader, encode_closed, encode_header, encode_lap,
    encode_resolved,
};
use iracing_core::positions::job::{LapPositions, Outcome, Reason, Report, run};
use iracing_core::positions::select::{FileFacts, NotMine, belongs};
use irsdk::defs::VarType;
use irsdk::header::DiskHeader;
use irsdk::scan::Scan;
use irsdk::testing::{LayoutBuilder, RowBuilder, ibt_bytes};
use rows::{FILE_A, FILE_A_END, FILE_B, FILE_B_START, FILE_C_START, Row};

const EARTH_M: f64 = 6_378_137.0;
const YAML: &str = "---\nWeekendInfo:\n TrackName: oulton international\n TrackID: 180\n SubSessionID: 0\nDriverInfo:\n DriverCarIdx: 0\n Drivers:\n - CarIdx: 0\n   CarPath: mx5 mx52016\n...\n";
/// The start dates of the real files (Unix seconds): 2026-11-06 22:42:08, 22:46:12 and
/// 2026-11-09 17:57:35, local time.
const START_A: i64 = 1_794_001_328;
const START_B: i64 = 1_794_001_572;
const START_C: i64 = 1_794_243_455;

/// A disk file image of `rows`, in session `session_num`, with `pad` floats of other variables
/// per row (to make a file span several chunks).
fn image(rows: &[Row], session_num: i32, start_unix: i64, pad: usize) -> Vec<u8> {
    let layout = LayoutBuilder::new()
        .tick_rate(60)
        .var("SessionNum", VarType::Int, 1, "", "")
        .var("SessionTick", VarType::Int, 1, "", "")
        .var("SessionTime", VarType::Double, 1, "s", "")
        .var("Lat", VarType::Double, 1, "deg", "")
        .var("Lon", VarType::Double, 1, "deg", "")
        .var("Alt", VarType::Float, 1, "m", "")
        .var("LapDistPct", VarType::Float, 1, "%", "")
        .var("Pad", VarType::Float, pad.max(1), "", "")
        .build();
    let built: Vec<Vec<u8>> = rows
        .iter()
        .map(|&(tick, time, lat, lon, alt, pct)| {
            let mut b = RowBuilder::new(&layout);
            b.set_i32("SessionNum", session_num)
                .set_i32("SessionTick", tick)
                .set_f64("SessionTime", time)
                .set_f64("Lat", lat)
                .set_f64("Lon", lon)
                .set_f32("Alt", alt)
                .set_f32("LapDistPct", pct);
            b.into_bytes()
        })
        .collect();
    let disk = DiskHeader {
        session_start_date: start_unix,
        session_start_time: rows[0].1,
        session_end_time: rows[rows.len() - 1].1,
        session_lap_count: 1,
        session_record_count: rows.len() as i32,
    };
    ibt_bytes(&layout, YAML, &built, disk)
}

fn scan(rows: &[Row], session_num: i32, start_unix: i64) -> Scan<Cursor<Vec<u8>>> {
    let bytes = image(rows, session_num, start_unix, 1);
    Scan::new(Cursor::new(bytes.clone()), bytes.len() as u64).unwrap()
}

fn header(session_num: i32) -> PositionsHeader {
    PositionsHeader {
        session_key: "session-1".into(),
        session_num,
        session_unique_id: 1,
        sub_session_id: 0,
        track_id: 180,
        car_path: "mx5 mx52016".into(),
        opened_unix: START_A - 5,
    }
}

/// The session time of the crossing between rows `i - 1` and `i`, and where in the step it is,
/// as the recorder finds it from `LapDistPct`.
fn crossing(rows: &[Row], i: usize) -> (f64, f64) {
    let (a, b) = (f64::from(rows[i - 1].5), f64::from(rows[i].5));
    let frac = (1.0 - a) / ((1.0 - a) + b);
    (rows[i - 1].1 + frac * (rows[i].1 - rows[i - 1].1), frac)
}

/// The recording of the rows `from..to` as a lap, the way the recorder keeps it.
fn lap(rows: &[Row], from: usize, to: usize, lap_seq: u64, start: Option<f64>) -> LapRecord {
    LapRecord {
        lap_seq,
        start_crossing: start,
        end_crossing: None,
        ticks: rows[from..to].iter().map(|r| r.0).collect(),
        pct: rows[from..to].iter().map(|r| r.5).collect(),
    }
}

/// A positions file of a closed session holding `laps`, written and read back through the
/// format; `resolved` laps are marked.
fn positions_file(h: &PositionsHeader, laps: &[LapRecord], resolved: &[u64]) -> PositionsFile {
    let mut bytes = encode_header(h);
    for l in laps {
        bytes.extend(encode_lap(l));
    }
    bytes.extend(encode_closed(h.opened_unix + 3600));
    for r in resolved {
        bytes.extend(encode_resolved(*r));
    }
    PositionsFile::decode(&bytes).unwrap()
}

/// The exact positions of `rows[from..to]` about the lap crossing between rows `at - 1` and
/// `at`: east, north and up in metres.
fn expected(rows: &[Row], at: usize, from: usize, to: usize) -> Vec<(f64, f64, f64)> {
    let (_, f) = crossing(rows, at);
    let (a, b) = (&rows[at - 1], &rows[at]);
    let (lat0, lon0, alt0) = (
        a.2 + f * (b.2 - a.2),
        a.3 + f * (b.3 - a.3),
        f64::from(a.4) + f * (f64::from(b.4) - f64::from(a.4)),
    );
    rows[from..to]
        .iter()
        .map(|r| {
            (
                (r.3 - lon0).to_radians() * EARTH_M * lat0.to_radians().cos(),
                (r.2 - lat0).to_radians() * EARTH_M,
                f64::from(r.4) - alt0,
            )
        })
        .collect()
}

fn never() -> bool {
    false
}

fn run_all(
    file: &PositionsFile,
    scans: &mut [Scan<Cursor<Vec<u8>>>],
) -> (Report, Vec<LapPositions>) {
    let mut sent = Vec::new();
    let report = run(file, scans, &never, &mut |p| sent.push(p));
    (report, sent)
}

fn assert_close(got: &LapPositions, want: &[(f64, f64, f64)]) {
    assert_eq!(got.x.len(), want.len());
    assert_eq!((got.z.len(), got.y.len()), (want.len(), want.len()));
    for (i, w) in want.iter().enumerate() {
        let (x, z, y) = (
            f64::from(got.x[i]),
            f64::from(got.z[i]),
            f64::from(got.y[i]),
        );
        assert!(
            (x - w.0).abs() < 2e-3 && (z - w.1).abs() < 2e-3 && (y - w.2).abs() < 2e-3,
            "sample {i}: got ({x}, {z}, {y}), want {w:?}"
        );
    }
}

#[test]
fn case_1_a_covered_lap_gets_the_projection_with_its_origin_at_the_crossing() {
    // The lap is the rows after the crossing between indexes 1 and 2 of file A.
    let (tc, _) = crossing(&FILE_A, 2);
    let file = positions_file(&header(0), &[lap(&FILE_A, 2, 302, 7, Some(tc))], &[]);
    let (report, sent) = run_all(&file, &mut [scan(&FILE_A, 0, START_A)]);
    assert_eq!(report.outcomes, [(7, Outcome::Sent)]);
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].lap_seq, 7);
    let want = expected(&FILE_A, 2, 2, 302);
    assert_close(&sent[0], &want);
    // The first sample is a step past the crossing: a metre or so from the origin, not the
    // first row's own position.
    let first = f64::from(sent[0].x[0]).hypot(f64::from(sent[0].z[0]));
    assert!(first > 0.05 && first < 1.5, "{first}");
}

#[test]
fn case_2_a_lap_starting_before_the_file_is_left_as_it_is() {
    // The lap's first samples are the end of file A (a file this job does not have); the file it
    // has begins in the middle of the lap.
    let mut l = lap(&FILE_A_END, 0, 100, 3, None);
    l.ticks.extend(FILE_B_START.iter().map(|r| r.0));
    l.pct.extend(FILE_B_START.iter().map(|r| r.5));
    let file = positions_file(&header(0), &[l], &[]);
    let (report, sent) = run_all(&file, &mut [scan(&FILE_B_START, 0, START_B)]);
    assert!(sent.is_empty());
    assert_eq!(
        report.outcomes,
        [(3, Outcome::Left(Reason::NotCovered { missing: 100 }))]
    );
    // With both files the lap is in two of them: still left as it is.
    let (report, sent) = run_all(
        &file,
        &mut [
            scan(&FILE_A_END, 0, START_A),
            scan(&FILE_B_START, 0, START_B),
        ],
    );
    assert!(sent.is_empty());
    assert_eq!(
        report.outcomes,
        [(3, Outcome::Left(Reason::SplitAcrossFiles))]
    );
}

#[test]
fn case_3_a_lap_with_an_out_of_world_row_is_left_as_it_is() {
    assert_eq!((FILE_C_START[0].2, FILE_C_START[0].3), (0.0, 0.0));
    let file = positions_file(&header(1), &[lap(&FILE_C_START, 0, 60, 1, None)], &[]);
    let (report, sent) = run_all(&file, &mut [scan(&FILE_C_START, 1, START_C)]);
    assert!(sent.is_empty());
    assert_eq!(report.outcomes, [(1, Outcome::Left(Reason::OutOfWorld))]);
}

#[test]
fn case_4_a_lap_distance_mismatch_is_left_as_it_is() {
    // The ticks of file A with the lap distances of file B: another session's recording.
    let (tc, _) = crossing(&FILE_A, 2);
    let mut l = lap(&FILE_A, 2, 302, 4, Some(tc));
    l.pct = FILE_B[2..302].iter().map(|r| r.5).collect();
    let file = positions_file(&header(0), &[l], &[]);
    let (report, sent) = run_all(&file, &mut [scan(&FILE_A, 0, START_A)]);
    assert!(sent.is_empty());
    assert_eq!(report.outcomes, [(4, Outcome::Left(Reason::Mismatch))]);
}

#[test]
fn case_5_two_files_of_one_session_each_lap_from_its_file() {
    let (ta, _) = crossing(&FILE_A, 2);
    let (tb, _) = crossing(&FILE_B, 2);
    let file = positions_file(
        &header(0),
        &[
            lap(&FILE_A, 2, 302, 1, Some(ta)),
            lap(&FILE_B, 2, 302, 2, Some(tb)),
        ],
        &[],
    );
    let (report, sent) = run_all(
        &file,
        &mut [scan(&FILE_A, 0, START_A), scan(&FILE_B, 0, START_B)],
    );
    assert_eq!(
        report.outcomes,
        [(1, Outcome::Sent), (2, Outcome::Sent)],
        "{report:?}"
    );
    assert_close(&sent[0], &expected(&FILE_A, 2, 2, 302));
    assert_close(&sent[1], &expected(&FILE_B, 2, 2, 302));
    // The files are given in the other order: the same result.
    let (_, swapped) = run_all(
        &file,
        &mut [scan(&FILE_B, 0, START_B), scan(&FILE_A, 0, START_A)],
    );
    assert_eq!(swapped, sent);
}

#[test]
fn case_6_a_file_outside_the_sessions_wall_clock_span_is_not_used() {
    let h = header(0);
    let closed = Some(h.opened_unix + 3600);
    let facts = |start: i64| {
        let s = scan(&FILE_A, 0, start);
        FileFacts::of(&s, START_A + 120)
    };
    // The real file's headers: track, car and sub-session read from its session text.
    let mine = facts(START_A);
    assert_eq!(
        (mine.track_id, mine.car_path.as_deref(), mine.sub_session_id),
        (Some(180), Some("mx5 mx52016"), Some(0))
    );
    assert!(mine.complete);
    assert_eq!(belongs(&h, closed, &mine), Ok(()));
    // The ids repeat offline: a file of the same track and car and the same ticks, written a
    // day later, is another session's. The file name is never looked at.
    let later = facts(START_A + 86_400);
    assert_eq!(belongs(&h, closed, &later), Err(NotMine::OutsideSpan));
    let earlier = facts(START_A - 86_400);
    assert_eq!(belongs(&h, closed, &earlier), Err(NotMine::OutsideSpan));
    // Selected like the thread does, only the file of the session is read.
    let candidates = [
        (later, scan(&FILE_A, 0, START_A + 86_400)),
        (mine, scan(&FILE_A, 0, START_A)),
        (earlier, scan(&FILE_A, 0, START_A - 86_400)),
    ];
    let mut chosen: Vec<_> = candidates
        .into_iter()
        .filter(|(f, _)| belongs(&h, closed, f).is_ok())
        .map(|(_, s)| s)
        .collect();
    assert_eq!(chosen.len(), 1);
    let (tc, _) = crossing(&FILE_A, 2);
    let file = positions_file(&h, &[lap(&FILE_A, 2, 302, 1, Some(tc))], &[]);
    let (report, _) = run_all(&file, &mut chosen);
    assert_eq!(report.outcomes, [(1, Outcome::Sent)]);
}

#[test]
fn case_7_driving_starts_mid_job_the_job_stops_and_the_next_run_sends() {
    let (tc, _) = crossing(&FILE_A, 2);
    let file = positions_file(&header(0), &[lap(&FILE_A, 2, 302, 1, Some(tc))], &[]);
    // Rows of 8 KB: the 302 rows span several chunks.
    let big = || {
        let bytes = image(&FILE_A, 0, START_A, 2000);
        Scan::new(Cursor::new(bytes.clone()), bytes.len() as u64).unwrap()
    };
    let polls = Cell::new(0);
    let driving = AtomicBool::new(false);
    let stop = || {
        polls.set(polls.get() + 1);
        // Driving starts after the first chunk was read.
        if polls.get() == 2 {
            driving.store(true, Ordering::SeqCst);
        }
        driving.load(Ordering::SeqCst)
    };
    let mut sent = Vec::new();
    let mut scans = [big()];
    let report = run(&file, &mut scans, &stop, &mut |p| sent.push(p));
    assert!(report.stopped);
    assert!(report.outcomes.is_empty(), "nothing is resolved");
    assert!(sent.is_empty(), "nothing half sent");
    assert_eq!(polls.get(), 2, "asked before each chunk, then it stopped");
    // Driving is over: the next run reads the file again and sends the lap.
    let mut sent = Vec::new();
    let mut scans = [big()];
    let report = run(&file, &mut scans, &never, &mut |p| sent.push(p));
    assert!(!report.stopped);
    assert_eq!(report.outcomes, [(1, Outcome::Sent)]);
    assert_close(&sent[0], &expected(&FILE_A, 2, 2, 302));
}

#[test]
fn a_lap_with_no_crossing_of_its_own_takes_the_origin_of_another() {
    let (tc, _) = crossing(&FILE_A, 2);
    // Lap 1 (resolved before) began at the line; lap 2 is rows 52 to 152 with no crossing.
    let file = positions_file(
        &header(0),
        &[
            lap(&FILE_A, 2, 52, 1, Some(tc)),
            lap(&FILE_A, 52, 152, 2, None),
        ],
        &[1],
    );
    let (report, sent) = run_all(&file, &mut [scan(&FILE_A, 0, START_A)]);
    assert_eq!(
        report.outcomes,
        [(2, Outcome::Sent)],
        "lap 1 is not pending"
    );
    assert_close(&sent[0], &expected(&FILE_A, 2, 52, 152));
    // With no crossing anywhere there is no origin.
    let file = positions_file(&header(0), &[lap(&FILE_A, 52, 152, 2, None)], &[]);
    let (report, sent) = run_all(&file, &mut [scan(&FILE_A, 0, START_A)]);
    assert!(sent.is_empty());
    assert_eq!(report.outcomes, [(2, Outcome::Left(Reason::NoOrigin))]);
}

#[test]
fn nothing_pending_reads_nothing() {
    let (tc, _) = crossing(&FILE_A, 2);
    let file = positions_file(&header(0), &[lap(&FILE_A, 2, 52, 1, Some(tc))], &[1]);
    let polls = Cell::new(0);
    let stop = || {
        polls.set(polls.get() + 1);
        false
    };
    let report = run(&file, &mut [scan(&FILE_A, 0, START_A)], &stop, &mut |_| {
        panic!("nothing to send")
    });
    assert_eq!(report, Report::default());
    assert_eq!(polls.get(), 0);
}

#[test]
fn a_file_of_another_session_number_gives_nothing() {
    let (tc, _) = crossing(&FILE_A, 2);
    let file = positions_file(&header(2), &[lap(&FILE_A, 2, 302, 1, Some(tc))], &[]);
    let (report, sent) = run_all(&file, &mut [scan(&FILE_A, 0, START_A)]);
    assert!(sent.is_empty());
    assert_eq!(
        report.outcomes,
        [(1, Outcome::Left(Reason::NotCovered { missing: 300 }))]
    );
}
