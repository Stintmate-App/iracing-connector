//! Runs every hand-made layout image of `tests/corpus/layout`.
//!
//! The first part of a file name says what the parsers must do:
//! `ok_` parses fully with no skipped variable; `header_err_` is refused by `Header::parse`;
//! `table_skips_` has a good header and a table that skips at least one variable; `any_` only
//! must not panic. No input may panic whatever its name says.

use std::path::PathBuf;

use irsdk::header::Header;
use irsdk::ibt::Ibt;
use irsdk::sampler::Sampler;
use irsdk::vars::VarTable;
use irsdk::view::SliceView;

fn files() -> Vec<(String, Vec<u8>)> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/corpus/layout");
    let mut out: Vec<(String, Vec<u8>)> = std::fs::read_dir(dir)
        .expect("corpus folder")
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|x| x == "bin"))
        .map(|p| {
            let name = p.file_stem().unwrap().to_string_lossy().into_owned();
            (name, std::fs::read(&p).unwrap())
        })
        .collect();
    out.sort();
    out
}

#[test]
fn every_corpus_file_is_handled_as_its_name_says() {
    let files = files();
    assert!(files.len() >= 50, "only {} corpus files", files.len());
    for (name, bytes) in &files {
        let header = Header::parse(bytes);
        // Whatever the verdict, the other readers must not panic either.
        let _ = Ibt::from_bytes(bytes.clone());
        if let Ok(mut s) = Sampler::attach(SliceView::new(bytes)) {
            for _ in 0..3 {
                let _ = s.poll();
            }
        }
        if name.starts_with("ok_") {
            let h = header.unwrap_or_else(|e| panic!("{name}: {e}"));
            let t = VarTable::parse(bytes, &h).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert!(t.skipped().is_empty(), "{name}: {:?}", t.skipped());
        } else if name.starts_with("header_err_") {
            assert!(header.is_err(), "{name} must be refused");
        } else if name.starts_with("table_skips_") {
            let h = header.unwrap_or_else(|e| panic!("{name}: {e}"));
            let t = VarTable::parse(bytes, &h).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert!(!t.skipped().is_empty(), "{name} must skip a variable");
            assert!(
                t.get("Lap").is_some() || name.ends_with("duplicate_name"),
                "{name}"
            );
        } else {
            assert!(name.starts_with("any_"), "{name}: unknown prefix");
            if let Ok(h) = header {
                let _ = VarTable::parse(bytes, &h);
            }
        }
    }
}

#[test]
fn name_bytes_are_read_as_cp1252_and_need_no_terminator() {
    let files = files();
    let get = |n: &str| files.iter().find(|(f, _)| f == n).unwrap().1.clone();

    let img = get("ok_name_cp1252");
    let h = Header::parse(&img).unwrap();
    let t = VarTable::parse(&img, &h).unwrap();
    // 0xE9 is an e acute; 0x80 is the euro sign.
    assert!(t.get("Vitesse_\u{e9}\u{20ac}").is_some());

    let img = get("ok_name_without_nul");
    let h = Header::parse(&img).unwrap();
    let t = VarTable::parse(&img, &h).unwrap();
    assert!(t.get(&"A".repeat(32)).is_some());
}
