//! Decoding of the raw session text: code page 1252, the UTF-8 marker, NULs and line ends.

use irsdk::yaml::decode;

fn corpus(name: &str) -> Vec<u8> {
    let path = format!("{}/tests/corpus/yaml/{name}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read(path).expect("corpus file")
}

#[test]
fn cp1252_maps_all_256_bytes() {
    const HIGH: [(u8, char); 27] = [
        (0x80, '\u{20AC}'),
        (0x82, '\u{201A}'),
        (0x83, '\u{0192}'),
        (0x84, '\u{201E}'),
        (0x85, '\u{2026}'),
        (0x86, '\u{2020}'),
        (0x87, '\u{2021}'),
        (0x88, '\u{02C6}'),
        (0x89, '\u{2030}'),
        (0x8A, '\u{0160}'),
        (0x8B, '\u{2039}'),
        (0x8C, '\u{0152}'),
        (0x8E, '\u{017D}'),
        (0x91, '\u{2018}'),
        (0x92, '\u{2019}'),
        (0x93, '\u{201C}'),
        (0x94, '\u{201D}'),
        (0x95, '\u{2022}'),
        (0x96, '\u{2013}'),
        (0x97, '\u{2014}'),
        (0x98, '\u{02DC}'),
        (0x99, '\u{2122}'),
        (0x9A, '\u{0161}'),
        (0x9B, '\u{203A}'),
        (0x9C, '\u{0153}'),
        (0x9E, '\u{017E}'),
        (0x9F, '\u{0178}'),
    ];
    for b in 1u8..=255 {
        let d = decode(&[b'a', b, b'z']);
        assert!(!d.utf8);
        assert_eq!(d.replaced, 0);
        let expected: String = match b {
            0x81 | 0x8D | 0x8F | 0x90 | 0x9D => "az".into(),
            0x80..=0x9F => {
                let c = HIGH.iter().find(|(x, _)| *x == b).expect("table entry").1;
                format!("a{c}z")
            }
            0x01..=0x08 | 0x0B..=0x1F | 0x7F => "az".into(),
            _ => format!("a{}z", char::from(b)),
        };
        assert_eq!(d.text, expected, "byte {b:#04x}");
    }
}

#[test]
fn undefined_bytes_are_dropped() {
    let d = decode(b"a\x81b\x8Dc\x8Fd\x90e\x9Df");
    assert_eq!(d.text, "abcdef");
}

#[test]
fn latin1_range_keeps_its_code_points() {
    let d = decode(b"caf\xE9 M\xFCller \xA0\xFF");
    assert_eq!(d.text, "caf\u{E9} M\u{FC}ller \u{A0}\u{FF}");
}

#[test]
fn text_ends_at_the_first_nul() {
    let d = decode(b"abc\n\0def\0\0");
    assert_eq!(d.text, "abc\n");
    assert_eq!(decode(b"\0abc").text, "");
    assert_eq!(decode(b"").text, "");
}

#[test]
fn crlf_becomes_lf_and_controls_go() {
    let d = decode(b"a\r\nb\tc\x07d\x1bx\r\n");
    assert_eq!(d.text, "a\nb\tcdx\n");
}

#[test]
fn utf8_marker_selects_utf8() {
    let raw = b"---\nWeekendInfo:\n Encoding: UTF8\n Name: caf\xC3\xA9 \xE2\x82\xAC\n\0\0";
    let d = decode(raw);
    assert!(d.utf8);
    assert_eq!(d.replaced, 0);
    assert!(d.text.contains("caf\u{E9} \u{20AC}"));
}

#[test]
fn utf8_marker_with_crlf() {
    let raw = b"---\r\nWeekendInfo:\r\n Encoding: UTF8\r\n Name: caf\xC3\xA9\r\n";
    let d = decode(raw);
    assert!(d.utf8);
    assert_eq!(
        d.text,
        "---\nWeekendInfo:\n Encoding: UTF8\n Name: caf\u{E9}\n"
    );
}

#[test]
fn utf8_invalid_sequences_are_counted() {
    let raw = b"---\nWeekendInfo:\n Encoding: UTF8\n A: \xFF\n B: \xC3\n";
    let d = decode(raw);
    assert!(d.utf8);
    assert_eq!(d.replaced, 2);
    assert_eq!(d.text.matches('\u{FFFD}').count(), 2);
}

#[test]
fn without_the_marker_utf8_bytes_are_mojibake() {
    let d = decode(b"---\nWeekendInfo:\n Name: caf\xC3\xA9\n");
    assert!(!d.utf8);
    assert!(d.text.contains("caf\u{C3}\u{A9}"));
}

#[test]
fn corpus_cp1252_names() {
    let d = decode(&corpus("cp1252_names.yaml"));
    assert!(!d.utf8);
    assert!(!d.text.contains('\r'));
    assert!(!d.text.contains('\0'));
    assert!(d.text.contains("TrackName: caf\u{E9} ring"));
    assert!(d.text.contains("UserName: Jos\u{E9} M\u{FC}ller"));
    assert!(
        d.text
            .contains("TeamName: Caf\u{E9} \u{201C}Racing\u{201D} \u{20AC} \n")
    );
}

#[test]
fn corpus_utf8_marker() {
    let d = decode(&corpus("utf8_marker.yaml"));
    assert!(d.utf8);
    assert_eq!(d.replaced, 0);
    assert!(d.text.contains("UserName: Jos\u{E9} M\u{FC}ller"));
    assert!(d.text.contains("TeamName: Equipo \u{20AC}"));
    assert!(!d.text.contains('\0'));
}
