//! A session text of about 1 MiB: the 64-car grid and a very large `CarSetup`, generated from
//! `tests/corpus/yaml/grid_64.yaml` so the repository does not carry a megabyte of text.

use std::time::Instant;

use irsdk::worker::SessionDoc;
use irsdk::yaml::{self, decode, fallback};

/// The grid file with its `CarSetup` replaced by one of at least `target` bytes.
fn large_text(target: usize) -> String {
    let path = format!(
        "{}/tests/corpus/yaml/grid_64.yaml",
        env!("CARGO_MANIFEST_DIR")
    );
    let grid = decode(&std::fs::read(path).unwrap()).text;
    let head = &grid[..grid.find("\nCarSetup:").expect("CarSetup section") + 1];
    let mut text = String::with_capacity(target + 4096);
    text.push_str(head);
    text.push_str("CarSetup:\n UpdateCount: 9\n");
    let mut i = 0;
    while text.len() < target {
        text.push_str(&format!(" Group{i}:\n  Sub:\n"));
        for k in 0..20 {
            text.push_str(&format!("   Setting{k}: {} kPa\n", 100 + (i * 7 + k) % 90));
        }
        i += 1;
    }
    text
}

#[test]
fn a_one_mebibyte_text_parses_whole_and_in_time() {
    let text = large_text(1 << 20);
    assert!(text.len() >= 1 << 20);

    let t0 = Instant::now();
    let p = yaml::parse(&text);
    let parse = t0.elapsed();
    let t0 = Instant::now();
    let f = fallback::extract(&text);
    let scan = t0.elapsed();
    println!(
        "1 MiB text: {} bytes, parse {parse:?}, fallback {scan:?}",
        text.len()
    );

    assert!(p.errors.is_empty(), "{:?}", p.errors);
    let d = p.info.driver.expect("driver section");
    assert_eq!(d.drivers.expect("drivers").len(), 64);
    assert_eq!(p.info.car_setup.unwrap().update_count(), Some(9));
    assert_eq!(f.car_setup_update_count, Some(9));
    // Generous: the real bound is a few milliseconds in a release build.
    assert!(parse.as_secs() < 10, "parse took {parse:?}");
}

#[test]
fn a_session_document_of_the_large_text_has_every_part() {
    let text = large_text(1 << 20);
    let doc = SessionDoc::from_raw(3, text.as_bytes());
    assert!(doc.errors.is_empty(), "{:?}", doc.errors);
    assert_eq!(doc.update, 3);
    assert_ne!(doc.sections.car_setup, 0);
    assert_ne!(doc.sections.driver, 0);
}
