//! Core types of the iRacing connector.
//!
//! This crate holds the connector description ([`SPEC`]), the texts of its control app
//! (`locales/en.json`), the SDK variable map ([`vars`], [`tick`], [`codes`]), the session data
//! types ([`meta`]) and the interfaces between the recorder parts ([`io`]), and the [`tracker`] that turns rows into laps.

#![deny(missing_docs)]
#![forbid(unsafe_code)]

pub mod announce;
pub mod catalog;
pub mod codes;
pub mod guidance;
pub mod ibt_source;
pub mod io;
pub mod lap;
pub mod live;
pub mod meta;
pub mod position;
pub mod resume;
pub mod rivals;
pub mod sectors;
pub mod setup;
pub mod telemetry;
#[cfg(feature = "testing")]
pub mod testing;
pub mod tick;
pub mod tracker;
pub mod validity;
pub mod vars;

use stintmate_connector_platform::spec::{ConnectorSpec, GameSpec};
use stintmate_connector_resources::Glyph;

/// What this connector is: its identity, the simulator it reads, the names of its executables
/// and kernel objects, the features it declares and its English UI catalog.
pub static SPEC: ConnectorSpec = ConnectorSpec {
    id: "iracing",
    name: "iRacing connector",
    sim: "iracing",
    sim_name: "iRacing",
    product: "52b2563a-02de-4911-86cc-87a28f1658b4",
    version: env!("CARGO_PKG_VERSION"),
    exe_prefix: "iracing",
    mutex_prefix: "IRacingConnector",
    autostart_value: "iRacing connector",
    game: GameSpec {
        exes: &["iracingsim64dx11.exe"],
        window_title: Some("iracing.com simulator"),
        window_class: Some("SimWinClass"),
        cheap_hint: Some(irsdk::live::sim_mapping_exists),
    },
    recorder_hz: 60.0,
    features: &[
        "window",
        "notices",
        "translated_texts",
        "remembered_services",
        "options",
        "setups",
        "fuel",
        "sectors",
        "conditions",
        "rivals",
        "live_frame",
        "lap_status",
        "capture_target",
        "brake_cues",
        "guidance",
        "actions",
        "announcements",
        "spotter",
    ],
    ui_catalog: include_str!("../locales/en.json"),
    glyph: Glyph::I,
    dev_env_aliases: &[],
};

pub use io::{
    Clock, Conditions, FinishedLap, LapType, ParticipantLap, SessionKey, SessionOpenInfo,
    SetupReason, Sink, Source, SourceEvent, StallTyres, TimeSource,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_is_valid() {
        assert_eq!(SPEC.validate(), Ok(()));
        assert_eq!(SPEC.endpoint(), "connector-iracing");
        assert_eq!(SPEC.catalog_id(), "iracing-connector");
    }

    #[test]
    fn ui_catalog_complete() {
        let problems = stintmate_connector_control::texts::check_ui_catalog(
            SPEC.ui_catalog,
            &SPEC.catalog_id(),
        );
        assert!(problems.is_empty(), "{problems:#?}");
    }
}
