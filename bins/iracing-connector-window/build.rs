//! Windows resources of `iracing-connector-window.exe`: the icon and the version information.

use std::path::PathBuf;
use stintmate_connector_resources::{FileInfo, version_rc};

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let out = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR"));
    // The official icon, checked in once for the three programs.
    let ico = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"))
        .join("../iracing-connector-ui/assets/icon.ico");
    println!("cargo:rerun-if-changed={}", ico.display());
    let rc = version_rc(
        &FileInfo {
            description: "iRacing connector window",
            internal_name: "iracing-connector-window",
            original_filename: "iracing-connector-window.exe",
            product_name: "Stintmate iRacing connector",
            version: &std::env::var("CARGO_PKG_VERSION").expect("version"),
        },
        &ico,
    );
    let rc_path = out.join("app.rc");
    std::fs::write(&rc_path, rc).expect("write app.rc");
    embed_resource::compile(&rc_path, embed_resource::NONE)
        .manifest_required()
        .expect("compile the resources");
}
