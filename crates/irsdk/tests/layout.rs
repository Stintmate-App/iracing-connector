//! Header, disk header and variable header offsets, against images the builder wrote.

use irsdk::defs::VarType;
use irsdk::header::{DISK_HEADER_END, DiskHeader, HEADER_LEN, Header, VAR_HEADER_LEN};
use irsdk::row::Row;
use irsdk::testing::{LayoutBuilder, RowBuilder, live_image};
use irsdk::vars::VarTable;

fn layout() -> irsdk::testing::Layout {
    LayoutBuilder::new()
        .tick_rate(60)
        .num_buf(3)
        .var(
            "SessionTime",
            VarType::Double,
            1,
            "s",
            "Seconds since session start",
        )
        .var("Speed", VarType::Float, 1, "m/s", "GPS vehicle speed")
        .var("Lap", VarType::Int, 1, "", "Laps started")
        .var("IsOnTrack", VarType::Bool, 1, "", "Car on track")
        .var("CarIdxLap", VarType::Int, 64, "", "Lap of every car")
        .var("TempUnit", VarType::Float, 1, "\u{b0}C", "Degrees")
        .build()
}

#[test]
fn sizes_are_the_sdk_sizes() {
    assert_eq!(HEADER_LEN, 112);
    assert_eq!(DISK_HEADER_END, 144);
    assert_eq!(VAR_HEADER_LEN, 144);
}

#[test]
fn header_fields_equal_what_was_written() {
    let l = layout();
    let r0 = RowBuilder::new(&l).into_bytes();
    let r1 = RowBuilder::new(&l).into_bytes();
    let image = live_image(&l, "---\nWeekendInfo:\n", &[(100, r0), (101, r1)]);
    let h = Header::parse(&image).unwrap();
    assert_eq!(h.ver, 2);
    assert!(h.connected());
    assert_eq!(h.tick_rate, 60);
    assert_eq!(h.session_info_update, 1);
    assert_eq!(h.session_info_len, 17);
    assert_eq!(h.num_vars, 6);
    assert_eq!(h.var_header_offset, 112);
    assert_eq!(h.session_info_offset, 112 + 6 * 144);
    assert_eq!(h.num_buf, 3);
    assert_eq!(h.buf_len as usize, l.buf_len());
    assert_eq!(h.var_buf[0].tick_count, 100);
    assert_eq!(h.var_buf[1].tick_count, 101);
    assert_eq!(h.var_buf[2].tick_count, 0);
    // The YAML sits where the header says.
    let at = h.session_info_offset as usize;
    assert_eq!(&image[at..at + 17], b"---\nWeekendInfo:\n");
    // The buffers are distinct and inside the image.
    let offs: Vec<i32> = h.var_buf[..3].iter().map(|b| b.buf_offset).collect();
    assert!(offs[0] < offs[1] && offs[1] < offs[2]);
    assert!(offs[2] as usize + l.buf_len() <= image.len());
}

#[test]
fn var_headers_equal_what_was_written() {
    let l = layout();
    let image = live_image(&l, "", &[]);
    let h = Header::parse(&image).unwrap();
    let t = VarTable::parse(&image, &h).unwrap();
    assert!(t.skipped().is_empty());
    assert_eq!(t.len(), 6);
    for v in l.vars() {
        let got = t.get(&v.name).unwrap();
        assert_eq!(got.ty, v.ty);
        assert_eq!(got.offset, v.offset);
        assert_eq!(got.count, v.count);
        assert_eq!(got.unit, v.unit);
        assert_eq!(got.desc, v.desc);
    }
    // The double sits at an 8-aligned offset, the rest packed in order.
    assert_eq!(t.get("SessionTime").unwrap().offset, 0);
    assert_eq!(t.get("Speed").unwrap().offset, 8);
    assert_eq!(t.get("Lap").unwrap().offset, 12);
    assert_eq!(t.get("IsOnTrack").unwrap().offset, 16);
    assert_eq!(t.get("CarIdxLap").unwrap().offset, 20);
    // The unit is decoded from cp1252.
    assert_eq!(t.get("TempUnit").unwrap().unit, "\u{b0}C");
}

#[test]
fn disk_header_fields() {
    let mut b = vec![0u8; DISK_HEADER_END];
    b[112..120].copy_from_slice(&1_700_000_000i64.to_le_bytes());
    b[120..128].copy_from_slice(&12.5f64.to_le_bytes());
    b[128..136].copy_from_slice(&99.25f64.to_le_bytes());
    b[136..140].copy_from_slice(&7i32.to_le_bytes());
    b[140..144].copy_from_slice(&5321i32.to_le_bytes());
    let d = DiskHeader::parse(&b).unwrap();
    assert_eq!(d.session_start_date, 1_700_000_000);
    assert_eq!(d.session_start_time, 12.5);
    assert_eq!(d.session_end_time, 99.25);
    assert_eq!(d.session_lap_count, 7);
    assert_eq!(d.session_record_count, 5321);
    assert!(DiskHeader::parse(&b[..DISK_HEADER_END - 1]).is_err());
}

#[test]
fn rows_are_read_back_through_the_image() {
    let l = layout();
    let mut rb = RowBuilder::new(&l);
    rb.set_f32("Speed", 41.5).set_i32("Lap", 3);
    let image = live_image(&l, "", &[(7, rb.into_bytes())]);
    let h = Header::parse(&image).unwrap();
    let off = h.var_buf[0].buf_offset as usize;
    let row = Row::from_slice(&image[off..off + h.buf_len as usize]);
    let t = VarTable::parse(&image, &h).unwrap();
    assert_eq!(t.find::<f32>("Speed").unwrap().read(&row), 41.5);
    assert_eq!(t.find::<i32>("Lap").unwrap().read(&row), 3);
    assert_eq!(h.var_buf[0].tick_count, 7);
}
