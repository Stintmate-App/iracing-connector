//! The connector's guidance for the engineer, as data.
//!
//! Facts about this simulator that an agent cannot infer. The connector declares them with
//! `pitwall.guidance.declare` at every connection; the runtime derives the idempotency key from
//! the items. Texts are English, at most 16 items and 8,000 characters of `when` and `text` in
//! all.

use std::sync::OnceLock;

use serde_json::{Value, json};

/// The most items a declaration may hold.
pub const MAX_ITEMS: usize = 16;
/// The most characters of all `when` and `text` fields together.
pub const MAX_CHARS: usize = 8000;

static ITEMS: OnceLock<Value> = OnceLock::new();

/// The guidance items (a JSON array), built once.
pub fn items() -> &'static Value {
    ITEMS.get_or_init(|| {
        json!([
            {
                "id": "setup_from_session",
                "topic": "setup",
                "when": "you read or compare a car setup",
                "text": "Setups come from the simulator's session data and carry every garage value, as typed fields plus `extra` entries. No screen capture is needed to read a setup. Setup files (`.sto`) are not read. In an event with a fixed setup the values cannot be changed.",
            },
            {
                "id": "tyres_at_stops",
                "topic": "tyres",
                "when": "you talk about tyre temperatures, pressures or wear",
                "text": "iRacing gives tyre temperatures and wear only at each stall entry (a pit stop or a reset), not live while driving, and they are not reset when the tyres are changed. Hot pressures are given only after a visit to the garage. Laps carry no tyre channels. The live frame's wheels are the readings of the last stall entry; a wheel is missing until the car has entered its stall once.",
            },
            {
                "id": "lap_validity",
                "topic": "session",
                "when": "you judge whether a lap counts",
                "text": "iRacing marks no lap valid or invalid. The connector infers it from incidents, leaving the track, the surface and tows. `invalid_cause_certain` is true only when an incident caused it. A lap that touches the pit road is sent as invalid. The inference can disagree with the game.",
            },
            {
                "id": "incidents",
                "topic": "session",
                "when": "you talk about incident points or penalties",
                "text": "Incident points (1x, 2x and 4x) are counted and called, and the event's limit is warned about near the end. A penalty shows as a black flag without its type, so the call is to box.",
            },
            {
                "id": "rivals_limits",
                "topic": "strategy",
                "when": "you compare with other drivers",
                "text": "The validity of other drivers' laps is not known: a timed lap that did not touch the pit road counts as valid. Online, some data may be missing for other cars.",
            },
        ])
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn items_respect_the_limits() {
        let items = items().as_array().expect("array");
        assert!(items.len() <= MAX_ITEMS);
        let mut chars = 0;
        let mut ids = std::collections::HashSet::new();
        for i in items {
            let (id, when, text) = (
                i["id"].as_str().expect("id"),
                i["when"].as_str().expect("when"),
                i["text"].as_str().expect("text"),
            );
            assert!(ids.insert(id), "duplicate {id}");
            assert!(id.len() <= 32 && id.starts_with(|c: char| c.is_ascii_lowercase()));
            assert!(
                id.chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
            );
            assert!((1..=120).contains(&when.chars().count()) && !when.contains('\n'));
            assert!((1..=1500).contains(&text.chars().count()));
            assert!(
                [
                    "setup",
                    "capture",
                    "telemetry",
                    "session",
                    "car",
                    "track",
                    "tyres",
                    "fuel",
                    "strategy",
                    "controls",
                    "other"
                ]
                .contains(&i["topic"].as_str().expect("topic"))
            );
            chars += when.chars().count() + text.chars().count();
        }
        assert!(chars <= MAX_CHARS, "{chars} characters");
    }
}
