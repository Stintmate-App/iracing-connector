//! By-name lookup, types, Int/BitField leniency, arrays, missing variables, stale generations.

use irsdk::defs::VarType;
use irsdk::header::Header;
use irsdk::row::Row;
use irsdk::testing::{Layout, LayoutBuilder, RowBuilder, live_image};
use irsdk::vars::{VarError, VarTable};

fn layout() -> Layout {
    LayoutBuilder::new()
        .var("SessionTime", VarType::Double, 1, "s", "")
        .var("Speed", VarType::Float, 1, "m/s", "")
        .var("Lap", VarType::Int, 1, "", "")
        .var("SessionFlags", VarType::BitField, 1, "irsdk_Flags", "")
        .var(
            "CarLeftRight",
            VarType::BitField,
            1,
            "irsdk_CarLeftRight",
            "",
        )
        .var("IsOnTrack", VarType::Bool, 1, "", "")
        .var("Gear", VarType::Int, 1, "", "")
        .var("Initial", VarType::Char, 4, "", "")
        .var("LFtempCL", VarType::Float, 1, "C", "")
        .var("CarIdxLapDistPct", VarType::Float, 64, "%", "")
        .var("Flags", VarType::BitField, 3, "", "")
        .build()
}

fn table(l: &Layout) -> (Vec<u8>, VarTable) {
    let image = live_image(l, "", &[]);
    let h = Header::parse(&image).unwrap();
    let t = VarTable::parse(&image, &h).unwrap();
    (image, t)
}

fn row_of(l: &Layout, f: impl FnOnce(&mut RowBuilder)) -> Row {
    let mut rb = RowBuilder::new(l);
    f(&mut rb);
    Row::from_slice(rb.bytes())
}

#[test]
fn scalars_by_name_and_type() {
    let l = layout();
    let (_, t) = table(&l);
    let row = row_of(&l, |r| {
        r.set_f64("SessionTime", 123.456)
            .set_f32("Speed", 55.25)
            .set_i32("Lap", -2)
            .set_u32("SessionFlags", 0x8000_0004)
            .set_bool("IsOnTrack", true)
            .set_i32("Gear", -1);
    });
    assert_eq!(t.find::<f64>("SessionTime").unwrap().read(&row), 123.456);
    assert_eq!(t.find::<f32>("Speed").unwrap().read(&row), 55.25);
    assert_eq!(t.find::<i32>("Lap").unwrap().read(&row), -2);
    assert_eq!(
        t.find::<u32>("SessionFlags").unwrap().read(&row),
        0x8000_0004
    );
    assert!(t.find::<bool>("IsOnTrack").unwrap().read(&row));
    assert_eq!(t.find::<i32>("Gear").unwrap().read(&row), -1);
}

#[test]
fn int_and_bitfield_are_interchangeable() {
    let l = layout();
    let (_, t) = table(&l);
    let row = row_of(&l, |r| {
        r.set_u32("CarLeftRight", 3).set_i32("Lap", 9);
    });
    // A bit field read as i32, an int read as u32.
    assert_eq!(t.find::<i32>("CarLeftRight").unwrap().read(&row), 3);
    assert_eq!(t.find::<u32>("CarLeftRight").unwrap().read(&row), 3);
    assert_eq!(t.find::<u32>("Lap").unwrap().read(&row), 9);
}

#[test]
fn wrong_types_are_refused() {
    let l = layout();
    let (_, t) = table(&l);
    assert_eq!(
        t.find::<f64>("Speed").unwrap_err(),
        VarError::WrongType {
            found: VarType::Float
        }
    );
    assert_eq!(
        t.find::<f32>("Lap").unwrap_err(),
        VarError::WrongType {
            found: VarType::Int
        }
    );
    assert_eq!(
        t.find::<bool>("Gear").unwrap_err(),
        VarError::WrongType {
            found: VarType::Int
        }
    );
    assert_eq!(
        t.find::<i32>("IsOnTrack").unwrap_err(),
        VarError::WrongType {
            found: VarType::Bool
        }
    );
    assert!(t.find_array::<f64>("CarIdxLapDistPct").is_err());
}

#[test]
fn missing_is_never_a_silent_zero() {
    let l = layout();
    let (_, t) = table(&l);
    assert_eq!(t.find::<f32>("RRtempCL").unwrap_err(), VarError::Missing);
    assert_eq!(t.find_array::<f32>("Nope").unwrap_err(), VarError::Missing);
    assert!(t.get("Nope").is_none());
}

#[test]
fn scalar_lookup_refuses_arrays() {
    let l = layout();
    let (_, t) = table(&l);
    assert_eq!(
        t.find::<f32>("CarIdxLapDistPct").unwrap_err(),
        VarError::NotScalar { count: 64 }
    );
}

#[test]
fn arrays() {
    let l = layout();
    let (_, t) = table(&l);
    let row = row_of(&l, |r| {
        r.set_f32_at("CarIdxLapDistPct", 0, 0.25)
            .set_f32_at("CarIdxLapDistPct", 63, 0.75)
            .set_u8_at("Initial", 1, b'Z')
            .set_u32_at("Flags", 2, 0xdead_beef);
    });
    let pct = t.find_array::<f32>("CarIdxLapDistPct").unwrap();
    assert_eq!(pct.count(), 64);
    assert_eq!(pct.get(&row, 0), Some(0.25));
    assert_eq!(pct.get(&row, 1), Some(0.0));
    assert_eq!(pct.get(&row, 63), Some(0.75));
    assert_eq!(pct.get(&row, 64), None);
    let all: Vec<f32> = pct.iter(&row).collect();
    assert_eq!(all.len(), 64);
    assert_eq!(all[63], 0.75);
    let initial = t.find_array::<u8>("Initial").unwrap();
    assert_eq!(initial.iter(&row).collect::<Vec<_>>(), [0, b'Z', 0, 0]);
    let flags = t.find_array::<u32>("Flags").unwrap();
    assert_eq!(flags.get(&row, 2), Some(0xdead_beef));
    // A scalar is an array of one.
    let speed = t.find_array::<f32>("Speed").unwrap();
    assert_eq!(speed.count(), 1);
}

#[test]
fn a_short_row_reads_as_zero() {
    let l = layout();
    let (_, t) = table(&l);
    let speed = t.find::<f32>("Speed").unwrap();
    let short = Row::from_slice(&[1, 2, 3]);
    assert_eq!(speed.read(&short), 0.0);
    let pct = t.find_array::<f32>("CarIdxLapDistPct").unwrap();
    assert_eq!(pct.get(&short, 5), Some(0.0));
}

#[test]
fn iteration_follows_the_table() {
    let l = layout();
    let (_, t) = table(&l);
    let names: Vec<&str> = t.iter().map(|v| v.name.as_str()).collect();
    assert_eq!(names[0], "SessionTime");
    assert_eq!(names.len(), l.vars().len());
}

#[test]
fn generations() {
    let l = layout();
    let (_, t) = table(&l);
    assert_eq!(t.generation(), 0);
    let t = t.with_generation(5);
    let speed = t.find::<f32>("Speed").unwrap();
    let mut row = row_of(&l, |r| {
        r.set_f32("Speed", 10.0);
    });
    row.set_generation(5);
    assert_eq!(speed.read(&row), 10.0);
}

#[cfg(debug_assertions)]
#[test]
#[should_panic(expected = "another generation")]
fn a_stale_handle_fails_a_debug_assertion() {
    let l = layout();
    let (_, t) = table(&l);
    let speed = t.find::<f32>("Speed").unwrap();
    let mut row = row_of(&l, |r| {
        r.set_f32("Speed", 10.0);
    });
    row.set_generation(1);
    speed.read(&row);
}

#[cfg(not(debug_assertions))]
#[test]
fn a_stale_handle_reads_zero() {
    let l = layout();
    let (_, t) = table(&l);
    let speed = t.find::<f32>("Speed").unwrap();
    let mut row = row_of(&l, |r| {
        r.set_f32("Speed", 10.0);
    });
    row.set_generation(1);
    assert_eq!(speed.read(&row), 0.0);
}
