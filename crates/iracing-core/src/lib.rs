//! Core types of the iRacing connector.
//!
//! This crate holds the connector description ([`SPEC`]), the texts of its control app
//! (`locales/en.json`), the SDK variable map ([`vars`], [`tick`], [`codes`]), the session data
//! types ([`meta`]) and the interfaces between the recorder parts, today three traits without
//! methods.

#![deny(missing_docs)]
#![forbid(unsafe_code)]

pub mod codes;
pub mod lap;
pub mod meta;
pub mod sectors;
#[cfg(feature = "testing")]
pub mod testing;
pub mod tick;
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
    ],
    ui_catalog: include_str!("../locales/en.json"),
    glyph: Glyph::I,
    dev_env_aliases: &[],
};

/// The recorder frame source, live or replayed.
pub trait Source {}

/// Where the tracker records go.
pub trait Sink {}

/// The tracker time source.
pub trait Clock {}

#[cfg(test)]
mod tests {
    use super::*;

    struct Nothing;
    impl Source for Nothing {}
    impl Sink for Nothing {}
    impl Clock for Nothing {}

    #[test]
    fn traits_can_be_implemented() {
        fn takes<T: Source + Sink + Clock>(_: &T) {}
        takes(&Nothing);
    }

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
