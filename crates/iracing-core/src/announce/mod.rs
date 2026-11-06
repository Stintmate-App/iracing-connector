//! Announcements: what the player is told, decided from the rows as they arrive.
//!
//! The detectors write [`Call`]s into a fixed-capacity [`Calls`]; building the announcement
//! that goes to the link is the caller's job and happens only when a call fires. [`Gate`] holds
//! the rules every detector shares: a session is open, no replay, the player is in the car, and
//! a quiet time after anything that makes the values jump.

use std::borrow::Cow;

use crate::codes::SessionKind;
use crate::meta::SessionMeta;
use crate::tick::Tick;

pub mod detector;
pub mod spotter;

/// The announcement catalog `connector.iracing`, English.
pub const CATALOG: &str = include_str!("../../data/catalog.json");

/// The catalog's name.
pub const CATALOG_ID: &str = "connector.iracing";

/// Seconds of silence after a session opens, the session state changes into or out of racing,
/// the session number changes, or the detectors are reset. Values are followed silently.
pub const QUIET_S: f64 = 10.0;

/// The most calls one row can produce.
pub const CALLS_CAPACITY: usize = 8;

/// A value of a call's parameter.
#[derive(Clone, Debug, PartialEq)]
pub enum ParamValue {
    /// An integer.
    Int(i64),
    /// A text; borrowed for fixed words, owned for a name (allocated only when a call fires).
    Str(Cow<'static, str>),
}

/// One announcement to make: a code of the catalog, which wording, and up to two parameters.
#[derive(Clone, Debug, PartialEq)]
pub struct Call {
    /// The catalog code.
    pub code: &'static str,
    /// Which wording: 0 is the catalog `text`, 1.. its `variants`.
    pub variant: u8,
    /// The parameters; an unused slot has an empty name.
    pub params: [(&'static str, ParamValue); 2],
}

impl Call {
    const EMPTY: Call = Call {
        code: "",
        variant: 0,
        params: [("", ParamValue::Int(0)), ("", ParamValue::Int(0))],
    };

    /// A call without parameters.
    pub fn new(code: &'static str) -> Call {
        Call {
            code,
            ..Call::EMPTY
        }
    }

    /// Sets the wording.
    pub fn variant(mut self, variant: u8) -> Call {
        self.variant = variant;
        self
    }

    /// Adds an integer parameter (the first free slot; a third is dropped).
    pub fn int(self, name: &'static str, value: i64) -> Call {
        self.with(name, ParamValue::Int(value))
    }

    /// Adds a text parameter (the first free slot; a third is dropped).
    pub fn text(self, name: &'static str, value: impl Into<Cow<'static, str>>) -> Call {
        self.with(name, ParamValue::Str(value.into()))
    }

    fn with(mut self, name: &'static str, value: ParamValue) -> Call {
        if let Some(slot) = self.params.iter_mut().find(|(n, _)| n.is_empty()) {
            *slot = (name, value);
        }
        self
    }

    /// The parameters that are set.
    pub fn set_params(&self) -> impl Iterator<Item = &(&'static str, ParamValue)> {
        self.params.iter().filter(|(n, _)| !n.is_empty())
    }
}

/// The calls one row produced: a list of fixed capacity, filled without allocation (a text
/// parameter that owns its string aside). A call over the capacity is dropped.
#[derive(Clone, Debug)]
pub struct Calls {
    items: [Call; CALLS_CAPACITY],
    len: usize,
}

impl Default for Calls {
    fn default() -> Calls {
        Calls::new()
    }
}

impl Calls {
    /// An empty list.
    pub fn new() -> Calls {
        Calls {
            items: [Call::EMPTY; CALLS_CAPACITY],
            len: 0,
        }
    }

    /// Adds a call; false when the list is full.
    pub fn push(&mut self, call: Call) -> bool {
        match self.items.get_mut(self.len) {
            Some(slot) => {
                *slot = call;
                self.len += 1;
                true
            }
            None => false,
        }
    }

    /// The calls, in the order they were made.
    pub fn as_slice(&self) -> &[Call] {
        &self.items[..self.len]
    }

    /// How many calls there are.
    pub fn len(&self) -> usize {
        self.len
    }

    /// Whether there are none.
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Empties the list.
    pub fn clear(&mut self) {
        for item in &mut self.items[..self.len] {
            *item = Call::EMPTY;
        }
        self.len = 0;
    }
}

/// What the gate says about one row.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GateState {
    /// The kind of the open session; `None` when no session is open or a replay plays: nothing
    /// is announced and nothing is followed.
    pub kind: Option<SessionKind>,
    /// The quiet time after a change is running: follow values, announce nothing.
    pub quiet: bool,
    /// The player is driving (on the track, pit road included).
    pub driving: bool,
    /// The player is driving or in the garage: enough for the race-state codes.
    pub in_car: bool,
    /// The player is on pit road.
    pub pit_road: bool,
    /// A quiet time started on this row: the detectors forget what they were following.
    pub restarted: bool,
    /// The session state is the parade lap (3) or racing (4).
    pub racing: bool,
    /// The session state is racing (4).
    pub green_state: bool,
}

/// The rules every detector shares, kept as a small state.
#[derive(Clone, Debug)]
pub struct Gate {
    primed: bool,
    quiet_until: f64,
    last_time: f64,
    last_num: i32,
    last_racing: bool,
}

impl Default for Gate {
    fn default() -> Gate {
        Gate::new()
    }
}

impl Gate {
    /// A gate with no session.
    pub fn new() -> Gate {
        Gate {
            primed: false,
            quiet_until: 0.0,
            last_time: 0.0,
            last_num: 0,
            last_racing: false,
        }
    }

    /// Forgets the session: the next row starts a quiet time.
    pub fn reset(&mut self) {
        self.primed = false;
    }

    /// Reads one row.
    pub fn step(&mut self, t: &Tick, meta: Option<&SessionMeta>) -> GateState {
        let now = t.session_time;
        let green_state = t.session_state == 4;
        let racing = green_state || t.session_state == 3;
        let Some(meta) = meta.filter(|_| !t.is_replay) else {
            self.primed = false;
            return GateState {
                kind: None,
                quiet: true,
                driving: false,
                in_car: false,
                pit_road: false,
                restarted: false,
                racing,
                green_state,
            };
        };
        // The chequered state (5) follows green without a quiet time: the finish is announced.
        let running = t.session_state == 4 || t.session_state == 5;
        let jumped = now < self.last_time - 1.0;
        let restarted =
            !self.primed || jumped || t.session_num != self.last_num || running != self.last_racing;
        if restarted {
            self.primed = true;
            self.quiet_until = now + QUIET_S;
        }
        self.last_time = now;
        self.last_num = t.session_num;
        self.last_racing = running;
        let driving = t.is_on_track;
        GateState {
            kind: Some(meta.kind),
            quiet: now < self.quiet_until,
            driving,
            in_car: driving || t.is_in_garage,
            pit_road: t.on_pit_road,
            restarted,
            racing,
            green_state,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use stintmate_connector_wire::catalog::AnnounceCatalog;

    #[test]
    fn catalog_validates_with_the_runtime() {
        let cat = AnnounceCatalog::parse(CATALOG, CATALOG_ID).expect("catalog");
        assert_eq!(cat.codes().len(), 39);
    }

    #[test]
    fn catalog_has_every_code_and_none_without_counterpart() {
        let cat = AnnounceCatalog::parse(CATALOG, CATALOG_ID).expect("catalog");
        let codes = cat.codes();
        for code in [
            "flag_yellow",
            "flag_double_yellow",
            "flag_yellow_ahead",
            "flag_double_yellow_ahead",
            "flag_yellow_cause",
            "flag_debris",
            "flag_yellow_self",
            "flag_clear",
            "flag_blue",
            "flag_blue_next",
            "flag_blue_unknown",
            "flag_red",
            "flag_black_orange",
            "penalty_black_flag",
            "penalty_served",
            "final_lap",
            "chequered",
            "chequered_no_position",
            "disqualified",
            "retired",
            "fcy_start",
            "fcy_pits_open",
            "fcy_last_lap",
            "fcy_restart",
            "rival_disqualified",
            "rival_out",
            "incident",
            "incident_limit_warning",
            "spotter_car_left",
            "spotter_car_right",
            "spotter_in_the_middle",
            "spotter_still_left",
            "spotter_still_right",
            "spotter_still_middle",
            "spotter_clear_left",
            "spotter_clear_right",
            "spotter_all_clear",
            "radio_check",
            "engineer_test",
        ] {
            assert!(codes.contains(&code), "missing {code}");
        }
        for code in [
            "flag_yellow_driver",
            "flag_slow_car",
            "flag_slow_car_driver",
            "flag_black_white",
            "flag_track_limits",
            "fcy_pits_lead_lap",
            "fcy_halt",
            "penalty_drive_through",
            "penalty_stop_go",
        ] {
            assert!(!codes.contains(&code), "{code} has no counterpart");
        }
    }

    #[test]
    fn catalog_texts_follow_the_format_rules() {
        let cat = AnnounceCatalog::parse(CATALOG, CATALOG_ID).expect("catalog");
        for (code, msg) in cat.messages() {
            let text = msg["text"].as_str().expect("text");
            assert!(text.chars().count() <= 200, "{code} too long");
            assert!(
                !text.chars().any(char::is_control),
                "{code} control character"
            );
            assert!(msg["group"].is_string(), "{code} has no group");
        }
    }

    #[test]
    fn calls_hold_eight_and_drop_the_rest() {
        let mut calls = Calls::new();
        for _ in 0..CALLS_CAPACITY {
            assert!(calls.push(Call::new("flag_red")));
        }
        assert!(!calls.push(Call::new("flag_red")));
        assert_eq!(calls.len(), CALLS_CAPACITY);
        calls.clear();
        assert!(calls.is_empty());
    }

    #[test]
    fn a_call_keeps_two_params() {
        let call = Call::new("incident")
            .int("points", 2)
            .int("total", 5)
            .int("x", 1);
        let set: Vec<_> = call.set_params().map(|(n, _)| *n).collect();
        assert_eq!(set, ["points", "total"]);
    }
}
