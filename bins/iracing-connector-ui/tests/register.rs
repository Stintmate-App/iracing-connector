//! `--register` and `--unregister` of `iracing-connector-ui.exe` into a temporary connectors
//! folder (`STINTMATE_CONNECTORS_DIR`), never the real one.

use serde_json::Value;
use std::path::Path;
use std::process::{Command, Output};

const EXE: &str = env!("CARGO_BIN_EXE_iracing-connector-ui");
const PRODUCT: &str = "52b2563a-02de-4911-86cc-87a28f1658b4";

fn run(conn: &Path, args: &[&str]) -> Output {
    Command::new(EXE)
        .args(args)
        .env_remove("STINTMATE_WORKSPACE")
        .env("STINTMATE_CONNECTORS_DIR", conn)
        .env(
            "STINTMATE_SETTINGS_FILE",
            std::env::temp_dir().join("iracing-register-test-no-settings.json"),
        )
        .output()
        .expect("run iracing-connector-ui")
}

fn text(o: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    )
}

fn read(conn: &Path) -> Value {
    serde_json::from_slice(&std::fs::read(conn.join("iracing.json")).unwrap()).unwrap()
}

#[test]
fn register_writes_the_descriptor() {
    let dir = tempfile::tempdir().unwrap();
    let conn = dir.path().join("connectors");
    let o = run(&conn, &["--register", "--flavour", "dev"]);
    assert_eq!(o.status.code(), Some(0), "{}", text(&o));
    let d = read(&conn);
    assert_eq!(d["id"], "iracing");
    assert_eq!(d["window"], true);
    assert_eq!(
        d["features"],
        serde_json::json!([
            "window",
            "notices",
            "translated_texts",
            "remembered_services",
            "options"
        ])
    );
    assert_eq!(d["endpoint"], "connector-iracing");
    assert_eq!(d["mutex"], "Local\\IRacingConnector.UI");
    assert_eq!(d["product"], PRODUCT);
    assert_eq!(d["logs"], serde_json::json!(["recorder.log", "ui.log"]));
    assert_eq!(d["launcher"].as_str().unwrap(), EXE);
}

#[test]
fn register_refuses_another_product_and_unregister_removes_only_ours() {
    let dir = tempfile::tempdir().unwrap();
    let conn = dir.path();
    let file = conn.join("iracing.json");
    let theirs = serde_json::json!({"schema": 1, "id": "iracing", "launcher": EXE,
        "product": "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee"})
    .to_string();
    std::fs::write(&file, &theirs).unwrap();
    let o = run(conn, &["--register"]);
    assert_eq!(o.status.code(), Some(1), "{}", text(&o));
    assert_eq!(std::fs::read_to_string(&file).unwrap(), theirs);
    // Unregistering leaves it as it is.
    let o = run(conn, &["--unregister"]);
    assert_eq!(o.status.code(), Some(0), "{}", text(&o));
    assert_eq!(std::fs::read_to_string(&file).unwrap(), theirs);
    // Ours is removed.
    std::fs::remove_file(&file).unwrap();
    assert_eq!(run(conn, &["--register"]).status.code(), Some(0));
    assert!(file.is_file());
    let o = run(conn, &["--unregister"]);
    assert_eq!(o.status.code(), Some(0), "{}", text(&o));
    assert!(!file.exists());
}
