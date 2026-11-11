//! Exact positions: the `lap_positions` message the recorder builds passes the runtime's embedded
//! schema, the positions file gets one record per lap while recording, and the background thread
//! does nothing while a session is open or when there is no disk telemetry folder.

mod common;
use common::*;
use iracing_core::positions::file::{
    LapRecord, PositionsFile, PositionsHeader, encode_closed, encode_header, encode_lap,
    encode_resolved,
};
use iracing_core::positions::job::LapPositions;
use iracing_recorder::positions::{Worker, unix_now};
use iracing_recorder::sink::positions_message;
use irsdk::testing::map::TestMap;
use irsdk::testing::replay::{layout_of, replay};
use irsdk::testing::synthetic_ibt;
use serde_json::{Value, json};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::channel;
use stintmate_connector_wire::schema_check::{PROTOCOL_SCHEMA, WRITE_API_SCHEMA, definition};

fn base64(bytes: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let n = chunk.iter().fold(0u32, |a, b| (a << 8) | u32::from(*b)) << (8 * (3 - chunk.len()));
        for i in 0..=chunk.len() {
            out.push(T[((n >> (18 - 6 * i)) & 63) as usize] as char);
        }
        out.extend(std::iter::repeat_n('=', 3 - chunk.len()));
    }
    out
}

#[test]
fn the_lap_positions_message_validates_against_the_write_api_schema() {
    let msg = positions_message(LapPositions {
        lap_seq: 3,
        x: vec![0.0, 1.5, 3.0],
        z: vec![0.0, 0.5, 1.0],
        y: vec![0.0, 0.1, 0.2],
    });
    // The wire form: the params plus `type`, the session's id and the lap; a channel's bytes are
    // an array of numbers in JSON and base64 text in the schema.
    let mut v = serde_json::to_value(&msg).unwrap();
    let o = v.as_object_mut().unwrap();
    o.insert("type".into(), json!("lap_positions"));
    o.insert("session_id".into(), json!(1));
    o.insert("lap".into(), json!(3));
    for ch in o
        .get_mut("channels")
        .unwrap()
        .as_object_mut()
        .unwrap()
        .values_mut()
    {
        let bytes: Vec<u8> = ch["data"]
            .as_array()
            .unwrap()
            .iter()
            .map(|b| u8::try_from(b.as_u64().unwrap()).unwrap())
            .collect();
        ch["data"] = json!(base64(&bytes));
    }
    let docs: Vec<Value> = vec![
        serde_json::from_str(WRITE_API_SCHEMA).unwrap(),
        serde_json::from_str(PROTOCOL_SCHEMA).unwrap(),
    ];
    definition(&docs, "lap_positions", &v).unwrap_or_else(|e| panic!("{e}\n{v}"));
    let names: Vec<&str> = v["channels"]
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(names, ["x", "y", "z"]);
}

#[test]
fn the_positions_file_gets_one_record_per_lap() {
    let rig = rig();
    let ibt = synthetic_ibt(4, 240);
    let layout = layout_of(&ibt);
    let size = TestMap::required_size(&layout, ibt.yaml_raw().len());
    let mut map =
        TestMap::create(&rig.map, &rig.event, &layout, ibt.yaml_raw(), size).expect("test mapping");
    let stop = AtomicBool::new(false);
    let mut child = rig.spawn();
    let player = std::thread::spawn(move || replay(&ibt, &mut map, 60.0, &stop));
    assert!(player.join().unwrap() > 0);
    assert!(
        rig.fake
            .wait_for(|c| c.iter().any(|x| x.method.ends_with(".lap")), 20),
        "no lap was written"
    );
    assert_eq!(rig.stop(&mut child), Some(0));
    rig.fake.stop();
    let laps = rig.count_suffix(".lap");
    let dir = rig.data_dir().join("positions");
    let files: Vec<_> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("no positions folder {}: {e}", dir.display()))
        .filter_map(Result::ok)
        .collect();
    assert_eq!(files.len(), 1, "one file for the one session");
    let file = PositionsFile::decode(&std::fs::read(files[0].path()).unwrap()).unwrap();
    assert!(!file.truncated);
    assert_eq!(file.laps.len(), laps, "one record per lap written");
    for (i, l) in file.laps.iter().enumerate() {
        assert_eq!(l.lap_seq, i as u64 + 1, "the lap's own number");
        assert!(!l.ticks.is_empty() && l.ticks.len() == l.pct.len());
    }
    // The recorder stopped with the session open (kept for the next recorder): not closed.
    assert_eq!(file.closed_unix, None);
    assert!(file.resolved.is_empty());
}

fn header(opened: i64) -> PositionsHeader {
    PositionsHeader {
        session_key: "session-a".into(),
        session_num: 1,
        session_unique_id: 2,
        sub_session_id: 0,
        track_id: 289,
        car_path: "mx5 mx52016".into(),
        opened_unix: opened,
    }
}

fn write(path: &Path, closed: Option<i64>, resolved: &[u64]) {
    let mut bytes = encode_header(&header(unix_now() - 7200));
    for seq in 1..=2u64 {
        bytes.extend(encode_lap(&LapRecord {
            lap_seq: seq,
            start_crossing: Some(10.0),
            end_crossing: Some(95.0),
            ticks: vec![100, 101, 102],
            pct: vec![0.001, 0.002, 0.003],
        }));
    }
    if let Some(c) = closed {
        bytes.extend(encode_closed(c));
    }
    for r in resolved {
        bytes.extend(encode_resolved(*r));
    }
    std::fs::write(path, bytes).unwrap();
}

#[test]
fn the_thread_does_nothing_while_driving_and_with_no_telemetry_folder() {
    let dir = tempfile::tempdir().unwrap();
    let pos = dir.path().join("positions");
    std::fs::create_dir_all(&pos).unwrap();
    let finished = pos.join("finished.bin");
    let pending = pos.join("pending.bin");
    write(&finished, Some(unix_now() - 3600), &[1, 2]);
    write(&pending, Some(unix_now() - 3600), &[]);
    let bytes_before = std::fs::read(&pending).unwrap();

    let driving = Arc::new(AtomicBool::new(true));
    let stop = Arc::new(AtomicBool::new(false));
    let (tx, rx) = channel();
    // A telemetry folder with a file in it, which must not be opened while driving.
    let tele = dir.path().join("telemetry");
    std::fs::create_dir_all(&tele).unwrap();
    std::fs::write(tele.join("not-a-file.ibt"), b"x").unwrap();
    let t = tele.clone();
    let mut worker = Worker::new(
        pos.clone(),
        Box::new(move || Some(t.clone())),
        driving.clone(),
        stop,
        tx,
    );
    worker.pass();
    assert!(
        finished.exists(),
        "nothing is deleted while a session is open"
    );
    assert_eq!(std::fs::read(&pending).unwrap(), bytes_before);
    assert!(rx.try_recv().is_err());

    // Not driving, and no telemetry folder: the finished file goes, the pending one stays as it
    // is and nothing is handed over.
    driving.store(false, Ordering::Relaxed);
    let mut worker = Worker::new(
        pos.clone(),
        Box::new(|| None),
        driving,
        Arc::new(AtomicBool::new(false)),
        channel().0,
    );
    worker.pass();
    assert!(!finished.exists());
    assert_eq!(std::fs::read(&pending).unwrap(), bytes_before);
    assert!(
        std::fs::read_dir(&tele).unwrap().count() == 1,
        "the telemetry folder is untouched"
    );
}
