//! Every refusal of the layout parsers, and the reasons a variable header is skipped.

use irsdk::defs::VarType;
use irsdk::header::{Header, LayoutError, VAR_HEADER_LEN};
use irsdk::testing::{Layout, LayoutBuilder, RowBuilder, live_image};
use irsdk::vars::VarTable;

fn layout() -> Layout {
    LayoutBuilder::new()
        .var("Speed", VarType::Float, 1, "m/s", "Speed")
        .var("Lap", VarType::Int, 1, "", "Lap")
        .build()
}

fn image() -> Vec<u8> {
    let l = layout();
    let row = RowBuilder::new(&l).into_bytes();
    live_image(&l, "yaml", &[(1, row.clone()), (2, row.clone()), (3, row)])
}

fn set(img: &mut [u8], off: usize, v: i32) {
    img[off..off + 4].copy_from_slice(&v.to_le_bytes());
}

fn refused(off: usize, v: i32) -> LayoutError {
    let mut img = image();
    set(&mut img, off, v);
    Header::parse(&img).unwrap_err()
}

#[test]
fn a_valid_image_parses() {
    assert!(Header::parse(&image()).is_ok());
}

#[test]
fn too_short() {
    let img = image();
    let e = Header::parse(&img[..111]).unwrap_err();
    assert!(matches!(
        e,
        LayoutError::TooShort {
            need: 112,
            have: 111,
            ..
        }
    ));
    assert_eq!(e.field(), "header");
    assert!(Header::parse(&[]).is_err());
}

#[test]
fn version() {
    assert_eq!(refused(0, 1), LayoutError::UnsupportedVersion(1));
    assert_eq!(refused(0, 3), LayoutError::UnsupportedVersion(3));
}

#[test]
fn num_buf_bounds() {
    assert_eq!(refused(32, 0).field(), "num_buf");
    assert_eq!(refused(32, 5).field(), "num_buf");
    assert_eq!(refused(32, -1).field(), "num_buf");
}

#[test]
fn buf_len_bounds() {
    assert_eq!(refused(36, 0).field(), "buf_len");
    assert_eq!(refused(36, -4).field(), "buf_len");
    assert_eq!(refused(36, 4 * 1024 * 1024 + 1).field(), "buf_len");
}

#[test]
fn num_vars_bounds() {
    assert_eq!(refused(24, -1).field(), "num_vars");
    assert_eq!(refused(24, 4097).field(), "num_vars");
    assert_eq!(refused(24, 100_000).field(), "num_vars");
}

#[test]
fn var_headers_must_fit() {
    assert_eq!(refused(28, -112).field(), "var_header_offset");
    assert!(matches!(
        refused(28, i32::MAX),
        LayoutError::PastEnd {
            field: "var_header_offset"
        }
    ));
    // Three headers announced where two fit.
    assert!(matches!(
        refused(24, 3),
        LayoutError::PastEnd {
            field: "var_header_offset"
        }
    ));
}

#[test]
fn session_info_must_fit() {
    assert_eq!(refused(20, -1).field(), "session_info_offset");
    assert_eq!(refused(16, -1).field(), "session_info_len");
    assert!(matches!(
        refused(16, i32::MAX),
        LayoutError::PastEnd {
            field: "session_info_offset"
        }
    ));
    assert!(matches!(
        refused(20, i32::MAX),
        LayoutError::PastEnd {
            field: "session_info_offset"
        }
    ));
}

#[test]
fn live_buffers_must_fit() {
    // Slot 1's offset, then slot 0's.
    assert!(matches!(
        refused(52 + 16, i32::MAX),
        LayoutError::PastEnd {
            field: "buf_offset"
        }
    ));
    assert_eq!(refused(52, -8).field(), "buf_offset");
    let img = image();
    let len = img.len();
    let e = Header::parse(&img[..len - 9]).unwrap_err();
    assert!(matches!(
        e,
        LayoutError::PastEnd {
            field: "buf_offset"
        }
    ));
}

#[test]
fn only_the_used_slots_are_checked() {
    let mut img = image();
    set(&mut img, 32, 2);
    set(&mut img, 52 + 32, i32::MAX); // slot 2 is unused now
    assert!(Header::parse(&img).is_ok());
}

#[test]
fn file_headers_may_have_no_rows() {
    let mut img = image();
    let cut = img.len() - 9;
    img.truncate(cut);
    assert!(Header::parse(&img).is_err());
    assert!(Header::parse_in(&img, img.len(), false).is_ok());
}

#[test]
fn truncation_at_every_length_never_panics() {
    let img = image();
    for n in 0..img.len() {
        let _ = Header::parse(&img[..n]);
        if let Ok(h) = Header::parse_in(&img[..n], n, false) {
            let _ = VarTable::parse(&img[..n], &h);
        }
    }
}

/// An image with one variable header rewritten.
fn with_var(index: usize, f: impl Fn(&mut [u8])) -> (Vec<u8>, Header) {
    let mut img = image();
    let h = Header::parse(&img).unwrap();
    let at = h.var_header_offset as usize + index * VAR_HEADER_LEN;
    f(&mut img[at..at + VAR_HEADER_LEN]);
    (img, h)
}

fn skipped_reason(f: impl Fn(&mut [u8])) -> &'static str {
    let (img, h) = with_var(0, f);
    let t = VarTable::parse(&img, &h).unwrap();
    assert_eq!(t.len(), 1, "the other variable stays");
    assert!(t.get("Lap").is_some());
    assert_eq!(t.skipped().len(), 1);
    t.skipped()[0].1
}

#[test]
fn skipped_unknown_type() {
    assert_eq!(
        skipped_reason(|v| v[0..4].copy_from_slice(&9i32.to_le_bytes())),
        "unknown type"
    );
    assert_eq!(
        skipped_reason(|v| v[0..4].copy_from_slice(&(-1i32).to_le_bytes())),
        "unknown type"
    );
}

#[test]
fn skipped_count_below_one() {
    assert_eq!(
        skipped_reason(|v| v[8..12].copy_from_slice(&0i32.to_le_bytes())),
        "count below one"
    );
}

#[test]
fn skipped_negative_numbers() {
    assert_eq!(
        skipped_reason(|v| v[4..8].copy_from_slice(&(-4i32).to_le_bytes())),
        "negative offset or count"
    );
    assert_eq!(
        skipped_reason(|v| v[8..12].copy_from_slice(&(-1i32).to_le_bytes())),
        "negative offset or count"
    );
}

#[test]
fn skipped_empty_name() {
    assert_eq!(skipped_reason(|v| v[16] = 0), "empty name");
}

#[test]
fn skipped_past_the_end_of_the_row() {
    let past = layout().buf_len() as i32;
    assert_eq!(
        skipped_reason(|v| v[4..8].copy_from_slice(&past.to_le_bytes())),
        "past the end of the row"
    );
    assert_eq!(
        skipped_reason(|v| v[8..12].copy_from_slice(&i32::MAX.to_le_bytes())),
        "past the end of the row"
    );
}

#[test]
fn skipped_duplicate_name() {
    let (img, h) = with_var(1, |v| {
        v[16..48].fill(0);
        v[16..21].copy_from_slice(b"Speed");
    });
    let t = VarTable::parse(&img, &h).unwrap();
    assert_eq!(t.len(), 1);
    assert_eq!(t.skipped(), &[("Speed".to_owned(), "duplicate name")]);
}

#[test]
fn var_table_refuses_a_buffer_without_the_headers() {
    let img = image();
    let h = Header::parse(&img).unwrap();
    assert!(matches!(
        VarTable::parse(&img[..120], &h),
        Err(LayoutError::PastEnd { .. })
    ));
}
