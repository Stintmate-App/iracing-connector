//! Properties over the layout parsers and the session text path: nothing panics on arbitrary
//! bytes or on damaged valid images, and what the builders write is what the parsers read.

use irsdk::defs::VarType;
use irsdk::header::{DiskHeader, Header};
use irsdk::ibt::Ibt;
use irsdk::sampler::{Poll, Sampler};
use irsdk::testing::{Layout, LayoutBuilder, RowBuilder, ibt_bytes, live_image};
use irsdk::vars::VarTable;
use irsdk::view::{SliceView, View};
use irsdk::yaml::{self, decode, fallback, requote, sections};
use proptest::prelude::*;

const TYPES: [VarType; 6] = [
    VarType::Char,
    VarType::Bool,
    VarType::Int,
    VarType::BitField,
    VarType::Float,
    VarType::Double,
];

/// A variable of a generated layout: base name, type, count, unit, description.
type Spec = (String, usize, usize, String, String);

fn specs() -> impl Strategy<Value = Vec<Spec>> {
    prop::collection::vec(
        (
            "[A-Za-z]{1,12}",
            0..TYPES.len(),
            1usize..8,
            "[ -~]{0,31}",
            "[ -~]{0,63}",
        ),
        1..40,
    )
}

fn build(specs: &[Spec], tick_rate: i32, num_buf: usize) -> Layout {
    let mut b = LayoutBuilder::new().tick_rate(tick_rate).num_buf(num_buf);
    for (i, (name, ty, count, unit, desc)) in specs.iter().enumerate() {
        b = b.var(&format!("{name}{i}"), TYPES[*ty], *count, unit, desc);
    }
    b.build()
}

/// A valid live image of a generated layout with one row.
fn image(specs: &[Spec]) -> Vec<u8> {
    let layout = build(specs, 60, 3);
    let row = RowBuilder::new(&layout).into_bytes();
    live_image(&layout, "---\nWeekendInfo:\n TrackName: x\n", &[(5, row)])
}

fn flip(mut img: Vec<u8>, flips: &[(usize, u8)]) -> Vec<u8> {
    for &(at, v) in flips {
        if !img.is_empty() {
            let at = at % img.len();
            img[at] ^= v;
        }
    }
    img
}

/// Runs every layout parser over `bytes`; the results are not looked at.
fn parse_everything(bytes: &[u8]) {
    let _ = DiskHeader::parse(bytes);
    if let Ok(h) = Header::parse(bytes) {
        let _ = VarTable::parse(bytes, &h);
    }
    if let Ok(h) = Header::parse_in(bytes, bytes.len().saturating_mul(2), false) {
        let _ = VarTable::parse(bytes, &h);
    }
    let _ = Ibt::from_bytes(bytes.to_vec());
    if let Ok(mut s) = Sampler::attach(SliceView::new(bytes)) {
        for _ in 0..4 {
            let _ = s.poll();
        }
    }
}

/// Runs the whole session text path over `raw`.
fn text_path(raw: &[u8]) {
    let d = decode(raw);
    assert!(!d.text.contains('\0'));
    assert!(
        d.text
            .chars()
            .all(|c| c == '\n' || c == '\t' || !c.is_control())
    );
    text_path_str(&d.text);
}

fn text_path_str(text: &str) {
    let _ = requote(text);
    let _ = sections(text);
    let _ = yaml::SectionHashes::of(text);
    let p = yaml::parse(text);
    let _ = p.info.driver.as_ref().and_then(|d| d.player());
    let _ = fallback::extract(text);
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]

    #[test]
    fn layout_parsers_never_panic_on_random_bytes(
        bytes in prop::collection::vec(any::<u8>(), 0..1200),
    ) {
        parse_everything(&bytes);
    }

    #[test]
    fn layout_parsers_never_panic_on_flipped_valid_images(
        s in specs(),
        flips in prop::collection::vec((any::<usize>(), 1u8..=255), 1..12),
    ) {
        parse_everything(&flip(image(&s), &flips));
    }

    #[test]
    fn layout_parsers_never_panic_on_truncated_valid_images(s in specs(), cut in any::<usize>()) {
        let img = image(&s);
        let n = cut % (img.len() + 1);
        parse_everything(&img[..n]);
    }

    #[test]
    fn the_builder_layout_round_trips_through_the_parsers(
        s in specs(),
        rate in prop::sample::select(vec![60, 120, 360]),
        num_buf in 1usize..=4,
    ) {
        let layout = build(&s, rate, num_buf);
        let row = RowBuilder::new(&layout).into_bytes();
        let img = live_image(&layout, "---\n", &[(1, row)]);
        let h = Header::parse(&img).unwrap();
        prop_assert_eq!(h.tick_rate, rate);
        prop_assert_eq!(h.num_buf as usize, num_buf);
        prop_assert_eq!(h.buf_len as usize, layout.buf_len());
        prop_assert_eq!(h.num_vars as usize, layout.vars().len());
        let t = VarTable::parse(&img, &h).unwrap();
        prop_assert!(t.skipped().is_empty(), "{:?}", t.skipped());
        prop_assert_eq!(t.len(), layout.vars().len());
        for v in layout.vars() {
            let got = t.get(&v.name).expect("variable present");
            prop_assert_eq!(got.ty, v.ty);
            prop_assert_eq!(got.count, v.count);
            prop_assert_eq!(got.offset, v.offset);
            prop_assert_eq!(&got.unit, &v.unit);
            prop_assert_eq!(&got.desc, &v.desc);
        }
    }

    #[test]
    fn integer_values_survive_the_sampler(values in prop::collection::vec(any::<i32>(), 1..12)) {
        let mut b = LayoutBuilder::new();
        for i in 0..values.len() {
            b = b.var(&format!("V{i}"), VarType::Int, 1, "", "");
        }
        let layout = b.build();
        let mut rb = RowBuilder::new(&layout);
        for (i, v) in values.iter().enumerate() {
            rb.set_i32(&format!("V{i}"), *v);
        }
        let img = live_image(&layout, "---\n", &[(7, rb.into_bytes())]);
        let mut s = Sampler::attach(SliceView::new(&img)).unwrap();
        prop_assert_eq!(s.poll(), Poll::Row);
        for (i, v) in values.iter().enumerate() {
            let var = s.vars().find::<i32>(&format!("V{i}")).unwrap();
            prop_assert_eq!(var.read(s.row()), *v);
        }
    }

    #[test]
    fn ibt_row_count_follows_the_record_count_and_the_file(
        rows in 1usize..20,
        claimed in -2i32..40,
    ) {
        let layout = LayoutBuilder::new()
            .var("Speed", VarType::Float, 1, "m/s", "")
            .build();
        let data: Vec<Vec<u8>> = (0..rows)
            .map(|k| {
                let mut b = RowBuilder::new(&layout);
                b.set_f32("Speed", k as f32);
                b.into_bytes()
            })
            .collect();
        let disk = DiskHeader {
            session_start_date: 0,
            session_start_time: 0.0,
            session_end_time: 0.0,
            session_lap_count: 0,
            session_record_count: claimed,
        };
        let ibt = Ibt::from_bytes(ibt_bytes(&layout, "---\n", &data, disk)).unwrap();
        let want = if claimed > 0 { (claimed as usize).min(rows) } else { rows };
        prop_assert_eq!(ibt.len(), want);
        prop_assert_eq!(ibt.rows().count(), want);
        prop_assert!(ibt.row(want).is_none());
        prop_assert_eq!(ibt.row(0).unwrap(), data[0].as_slice());
    }

    #[test]
    fn slice_view_never_reads_out_of_bounds(
        bytes in prop::collection::vec(any::<u8>(), 0..64),
        off in any::<usize>(),
        len in 0usize..80,
    ) {
        let v = SliceView::new(&bytes);
        let mut dst = vec![0xAAu8; len];
        let ok = v.copy_out(off, &mut dst);
        let fits = off.checked_add(len).is_some_and(|e| e <= bytes.len());
        prop_assert_eq!(ok, fits);
        if ok {
            prop_assert_eq!(&dst[..], &bytes[off..off + len]);
        }
        if let Some(x) = v.load_i32(off) {
            prop_assert!(off.checked_add(4).is_some_and(|e| e <= bytes.len()));
            prop_assert_eq!(
                x.to_le_bytes(),
                <[u8; 4]>::try_from(&bytes[off..off + 4]).unwrap()
            );
        }
    }

    #[test]
    fn the_text_path_never_panics_on_random_bytes(
        bytes in prop::collection::vec(any::<u8>(), 0..2000),
    ) {
        text_path(&bytes);
    }

    #[test]
    fn the_text_path_never_panics_on_random_text(text in "\\PC{0,600}") {
        text_path_str(&text);
    }

    #[test]
    fn the_text_path_never_panics_on_damaged_yaml(
        flips in prop::collection::vec((any::<usize>(), 1u8..=255), 1..16),
    ) {
        let path = format!("{}/tests/corpus/yaml/full.yaml", env!("CARGO_MANIFEST_DIR"));
        text_path(&flip(std::fs::read(path).unwrap(), &flips));
    }

    #[test]
    fn requote_is_idempotent(text in "[ -~\n]{0,300}") {
        let once = requote(&text).into_owned();
        let twice = requote(&once).into_owned();
        prop_assert_eq!(once, twice);
    }
}

/// A free-text name: printable ASCII and Latin-1, no edge spaces, not wrapped in quotes.
fn name() -> impl Strategy<Value = String> {
    "[ -~\u{a1}-\u{ff}]{1,24}".prop_filter("edge spaces or quotes", |s| {
        s.trim() == s && !(s.len() >= 2 && s.starts_with('"') && s.ends_with('"'))
    })
}

/// The cp1252 bytes of a text made of ASCII and Latin-1 only.
fn cp1252(text: &str) -> Vec<u8> {
    text.chars().map(|c| c as u8).collect()
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(96))]

    /// A text written from the values of the typed fields reads back as those values, names
    /// with quotes, backslashes, colons and `#` included.
    #[test]
    fn typed_fields_round_trip_through_text(
        track_id in 0i64..1_000_000,
        track_km in 0u32..30_000,
        limit in prop::option::of(0u32..100),
        fuel in 1u32..200_000,
        pct in 1u32..100,
        names in prop::collection::vec((name(), name(), name(), name()), 1..6),
        player in any::<prop::sample::Index>(),
    ) {
        let km = f64::from(track_km) / 100.0;
        let fuel_l = f64::from(fuel) / 1000.0;
        let pct_f = f64::from(pct) / 100.0;
        let player = player.index(names.len());
        let mut t = String::from("---\nWeekendInfo:\n");
        t += &format!(" TrackName: track {track_id}\n TrackID: {track_id}\n TrackLength: {km:.2} km\n");
        t += " WeekendOptions:\n";
        match limit {
            Some(n) => t += &format!("  IncidentLimit: {n}\n"),
            None => t += "  IncidentLimit: unlimited\n",
        }
        t += &format!(
            "DriverInfo:\n DriverCarIdx: {player}\n DriverCarFuelMaxLtr: {fuel_l:.3}\n DriverCarMaxFuelPct: {pct_f:.2}\n Drivers:\n"
        );
        for (i, (user, team, abbrev, initials)) in names.iter().enumerate() {
            t += &format!(
                " - CarIdx: {i}\n   UserName: {user}\n   TeamName: {team}\n   AbbrevName: {abbrev}\n   Initials: {initials}\n   CarScreenName: Car {i}\n"
            );
        }
        let text = decode(&cp1252(&t)).text;
        let p = yaml::parse(&text);
        prop_assert!(p.errors.is_empty(), "{:?}", p.errors);

        let w = p.info.weekend.unwrap();
        prop_assert_eq!(w.track_id, Some(track_id));
        prop_assert!((w.track_length_m().unwrap() - km * 1000.0).abs() < 0.01);
        prop_assert_eq!(w.incident_limit(), limit);

        let d = p.info.driver.unwrap();
        let drivers = d.drivers.clone().unwrap();
        prop_assert_eq!(drivers.len(), names.len());
        for (got, (user, team, abbrev, initials)) in drivers.iter().zip(&names) {
            prop_assert_eq!(got.user_name.as_deref(), Some(user.as_str()));
            prop_assert_eq!(got.team_name.as_deref(), Some(team.as_str()));
            prop_assert_eq!(got.abbrev_name.as_deref(), Some(abbrev.as_str()));
            prop_assert_eq!(got.initials.as_deref(), Some(initials.as_str()));
        }
        prop_assert_eq!(d.player().unwrap().car_idx, Some(player as i64));
        let expect = format!("{fuel_l:.3}").parse::<f64>().unwrap()
            * format!("{pct_f:.2}").parse::<f64>().unwrap();
        prop_assert!((d.fuel_capacity_l().unwrap() - expect).abs() < 1e-9);

        let f = fallback::extract(&text);
        prop_assert_eq!(f.track_id, Some(track_id));
        prop_assert_eq!(f.driver_car_idx, Some(player as i64));
        let want_car = format!("Car {player}");
        prop_assert_eq!(f.player_car_screen_name.as_deref(), Some(want_car.as_str()));
    }
}
