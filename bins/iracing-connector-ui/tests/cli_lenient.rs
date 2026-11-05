//! An option this build does not know does not stop the command: a newer Stintmate may pass one.

#[test]
fn an_unknown_option_beside_version_still_exits_0() {
    let o = std::process::Command::new(env!("CARGO_BIN_EXE_iracing-connector-ui"))
        .args(["--version", "--some-new-flag"])
        .output()
        .expect("run iracing-connector-ui");
    assert_eq!(o.status.code(), Some(0));
    assert!(
        String::from_utf8_lossy(&o.stdout).starts_with("iracing-connector-ui "),
        "{}",
        String::from_utf8_lossy(&o.stdout)
    );
}
