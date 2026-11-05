//! Ids and the catalog Pitwall needs before it can name a session's track, layout, car and class.
//!
//! | Id | Rule |
//! |---|---|
//! | layout | the simulator's `TrackID` as a decimal number |
//! | track | a slug of the display name, else of the track name |
//! | car | the `CarPath` as written, trimmed, at most 200 characters |
//! | class | the `CarClassID` as a decimal number |
//!
//! A car path may hold a space (`mx5 mx52016`): the write API takes any text of 1 to 200
//! characters as a car id, so the path is kept as the simulator writes it.

use stintmate_connector_wire::messages::{Car, CarClass, Catalog, Layout, Track};

use crate::meta::{DriverEntry, SessionMeta};

/// The longest track slug.
pub const SLUG_MAX: usize = 48;
/// The longest car id the write API takes.
pub const CAR_ID_MAX: usize = 200;

/// Lowercase ASCII, every run of other characters as one `-`, no `-` at either end, at most `max`
/// characters.
pub fn slug(text: &str, max: usize) -> String {
    let mut out = String::with_capacity(text.len().min(max));
    let mut pending = false;
    for c in text.chars() {
        if c.is_ascii_alphanumeric() {
            if pending && !out.is_empty() {
                out.push('-');
            }
            pending = false;
            out.push(c.to_ascii_lowercase());
        } else {
            pending = true;
        }
    }
    out.truncate(max);
    while out.ends_with('-') {
        out.pop();
    }
    out
}

/// The track id from the display name, else from the track name; empty when neither has a letter
/// or digit.
pub fn track_id(display_name: &str, track_name: &str) -> String {
    let s = slug(display_name, SLUG_MAX);
    if s.is_empty() {
        slug(track_name, SLUG_MAX)
    } else {
        s
    }
}

/// The layout id from the simulator's track id.
pub fn layout_id(track_id: i64) -> String {
    track_id.to_string()
}

/// The car id from the car path: trimmed and cut to the limit.
pub fn car_id(car_path: &str) -> String {
    car_path.trim().chars().take(CAR_ID_MAX).collect()
}

/// The class id from the class number.
pub fn class_id(class: i64) -> String {
    class.to_string()
}

/// The classes and cars of the player and every real car of the roster, once each, in slot order
/// with the player first. The player's car carries the tank capacity.
fn entries(meta: &SessionMeta) -> Catalog {
    let mut out = Catalog::default();
    let mut add = |d: &DriverEntry, player: bool| {
        if d.car_id.is_empty() {
            return;
        }
        if !d.class_id.is_empty() && !out.classes.iter().any(|c| c.class_id == d.class_id) {
            out.classes.push(CarClass {
                class_id: d.class_id.clone(),
                name: if d.class_name.is_empty() {
                    d.car_name.clone()
                } else {
                    d.class_name.clone()
                },
            });
        }
        if out.cars.iter().any(|c| c.car_id == d.car_id) {
            return;
        }
        out.cars.push(Car {
            car_id: d.car_id.clone(),
            name: if d.car_name.is_empty() {
                d.car_id.clone()
            } else {
                d.car_name.clone()
            },
            class_id: Some(d.class_id.clone()).filter(|c| !c.is_empty()),
            fuel_capacity_l: if player { meta.fuel_capacity_l } else { None },
        });
    };
    if let Some(p) = &meta.player {
        add(p, true);
    }
    for d in meta.roster.iter().flatten().filter(|d| d.is_real) {
        add(d, false);
    }
    out
}

/// The catalog of a session at its start: the track, its layout with the length, the player's
/// class and car, and every other real car and class of the roster.
pub fn session_catalog(meta: &SessionMeta) -> Catalog {
    let mut out = entries(meta);
    let t = &meta.track;
    if !t.track_id.is_empty() {
        out.tracks.push(Track {
            track_id: t.track_id.clone(),
            name: t.track_name.clone(),
        });
        out.layouts.push(Layout {
            track_id: t.track_id.clone(),
            layout_id: t.layout_id.clone(),
            name: t.layout_name.clone(),
            length_m: t.length_m,
        });
    }
    out
}

/// The classes and cars only, for the cars that join after the session opened.
pub fn cars_only(meta: &SessionMeta) -> Catalog {
    entries(meta)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::meta::{TrackIds, build};
    use crate::testing::YamlBuilder;
    use irsdk::worker::SessionDoc;

    fn meta_of(b: &YamlBuilder) -> SessionMeta {
        build(&SessionDoc::from_raw(1, b.build().as_bytes()), 0)
    }

    #[test]
    fn slugs_are_lowercase_ascii_with_single_dashes() {
        assert_eq!(
            slug("Circuit de Spa-Francorchamps", 48),
            "circuit-de-spa-francorchamps"
        );
        assert_eq!(slug("  --N\u{fc}rburgring   GP!! ", 48), "n-rburgring-gp");
        assert_eq!(slug("!!!", 48), "");
        assert_eq!(slug("abcdef ghij", 7), "abcdef");
        assert_eq!(slug("ab cd", 3), "ab");
    }

    #[test]
    fn the_track_id_falls_back_to_the_track_name() {
        assert_eq!(track_id("Oulton Park", "oulton"), "oulton-park");
        assert_eq!(track_id("", "oulton international"), "oulton-international");
        assert_eq!(track_id("!!", "spa up"), "spa-up");
        assert_eq!(layout_id(261), "261");
        assert_eq!(class_id(74), "74");
    }

    #[test]
    fn a_car_path_keeps_its_space_and_is_cut_to_the_limit() {
        assert_eq!(car_id(" mx5 mx52016 "), "mx5 mx52016");
        assert_eq!(car_id(&"a".repeat(300)).chars().count(), CAR_ID_MAX);
    }

    #[test]
    fn a_single_class_session_catalogs_track_layout_class_and_car() {
        let meta = meta_of(&YamlBuilder::new());
        let c = session_catalog(&meta);
        assert_eq!(c.tracks.len(), 1);
        assert_eq!(c.tracks[0].track_id, "circuit-de-spa-francorchamps");
        assert_eq!(c.tracks[0].name, "Circuit de Spa-Francorchamps");
        assert_eq!(c.layouts[0].layout_id, "261");
        assert_eq!(c.layouts[0].name, "Grand Prix Pits");
        assert_eq!(c.layouts[0].length_m, Some(4010.0));
        assert_eq!(c.classes.len(), 1);
        assert_eq!(c.cars.len(), 1);
        assert_eq!(c.cars[0].car_id, "porsche992rgt3");
        assert_eq!(c.cars[0].class_id.as_deref(), Some("74"));
        assert_eq!(c.cars[0].fuel_capacity_l, Some(100.0));
    }

    #[test]
    fn a_multiclass_field_is_catalogued_at_the_start_without_the_pace_car() {
        let meta = meta_of(&YamlBuilder::new().roster(12, 3));
        let c = session_catalog(&meta);
        assert_eq!(c.classes.len(), 3);
        assert_eq!(c.cars.len(), 3);
        assert!(c.cars.iter().all(|car| car.car_id != "safety pcfast"));
        // Only the player's car has a capacity.
        let with_cap: Vec<_> = c
            .cars
            .iter()
            .filter(|x| x.fuel_capacity_l.is_some())
            .collect();
        assert_eq!(with_cap.len(), 1);
        assert!(c.cars.iter().any(|x| x.car_id == "mx5 mx52016"));
    }

    #[test]
    fn cars_only_has_no_track() {
        let meta = meta_of(&YamlBuilder::new().roster(4, 2));
        let c = cars_only(&meta);
        assert!(c.tracks.is_empty() && c.layouts.is_empty());
        assert_eq!(c.classes.len(), 2);
    }

    #[test]
    fn a_meta_with_nothing_read_has_an_empty_catalog() {
        let c = session_catalog(&SessionMeta::empty(0));
        assert_eq!(c, Catalog::default());
        let mut m = SessionMeta::empty(0);
        m.track = TrackIds::default();
        assert!(cars_only(&m).cars.is_empty());
    }
}
