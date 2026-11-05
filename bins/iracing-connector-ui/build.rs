//! Windows resources of `iracing-connector-ui.exe`: the icon and the version information.

use std::path::PathBuf;
use stintmate_connector_resources::{FileInfo, Glyph, ico_bytes, version_rc};

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let out = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR"));
    let ico = out.join("app.ico");
    std::fs::write(&ico, ico_bytes(Glyph::I, "ok", &[16, 24, 32, 48, 64])).expect("write app.ico");
    let rc = version_rc(
        &FileInfo {
            description: "iRacing connector",
            internal_name: "iracing-connector-ui",
            original_filename: "iracing-connector-ui.exe",
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
