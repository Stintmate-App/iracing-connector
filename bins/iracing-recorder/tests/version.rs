//! The `--version` option of `iracing-recorder.exe`: prints `<exe name> <version>` and exits 0.

#[test]
fn version_prints_the_name_and_the_version() {
    let o = std::process::Command::new(env!("CARGO_BIN_EXE_iracing-recorder"))
        .arg("--version")
        .output()
        .expect("run iracing-recorder");
    assert_eq!(o.status.code(), Some(0));
    assert_eq!(
        String::from_utf8_lossy(&o.stdout).trim(),
        format!("iracing-recorder {}", env!("CARGO_PKG_VERSION"))
    );
}
