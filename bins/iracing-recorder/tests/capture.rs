//! The capture target and the actions through the recorder process, and the declarations it
//! makes to Pitwall at every connection.

mod common;
use common::*;
use iracing_core::SPEC;
use iracing_recorder::serve::{capture_target_of, to_json};
use serde_json::json;
use stintmate_connector_platform::game::game_running;
use stintmate_connector_platform::window::WindowInfo;

fn win(hwnd: i64, exe: &str, title: &str, class: &str, area: i64) -> WindowInfo {
    WindowInfo {
        hwnd,
        pid: 77,
        exe: exe.into(),
        title: title.into(),
        class: class.into(),
        area,
        minimized: area == 0,
    }
}

#[test]
fn the_capture_target_is_the_simulators_largest_window() {
    let sim = "iracingsim64dx11.exe";
    let windows = [
        win(1, "other.exe", "Notes", "Notepad", 4_000_000),
        win(2, sim, "iRacing.com Simulator", "SimWinClass", 1_000),
        win(3, sim, "iRacing.com Simulator", "SimWinClass", 2_000_000),
    ];
    let v = to_json(&capture_target_of(&windows).unwrap());
    assert_eq!(v["hwnd"], 3);
    assert_eq!(v["title"], "iRacing.com Simulator");
    // By its title and class when the process name is not known to the lister.
    let by_class = [win(9, "", "iRacing.com Simulator", "SimWinClass", 500)];
    assert_eq!(to_json(&capture_target_of(&by_class).unwrap())["hwnd"], 9);
    let wrong_class = [win(9, "", "iRacing.com Simulator", "Other", 500)];
    assert_eq!(
        capture_target_of(&wrong_class).unwrap_err().code,
        "game_not_running"
    );
    let minimized = [win(4, sim, "iRacing.com Simulator", "SimWinClass", 0)];
    assert_eq!(
        capture_target_of(&minimized).unwrap_err().code,
        "window_minimized"
    );
}

#[test]
fn declarations_capture_target_and_actions_over_the_link() {
    let rig = rig();
    let mut child = rig.spawn();
    assert!(
        rig.fake.wait_for(
            |c| ["catalog", "guidance", "actions"]
                .iter()
                .all(|k| c.iter().any(|x| x.method == format!("pitwall.{k}.declare"))),
            15
        ),
        "not declared: {:?}",
        rig.fake
            .calls()
            .iter()
            .map(|c| &c.method)
            .collect::<Vec<_>>()
    );
    let calls = rig.fake.calls();
    let find = |m: &str| calls.iter().find(|c| c.method == m).unwrap();
    let guidance = find("pitwall.guidance.declare");
    assert!(
        guidance
            .ik
            .as_deref()
            .unwrap()
            .starts_with("iracing/guidance/")
    );
    let items = to_json(&guidance.params)["items"].as_array().unwrap().len();
    assert!((1..=16).contains(&items), "{items} guidance items");
    let actions = to_json(&find("pitwall.actions.declare").params);
    let codes: Vec<_> = actions["actions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["code"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(codes, ["test.ping", "test.announce"]);
    assert!(
        find("pitwall.catalog.declare")
            .ik
            .as_deref()
            .unwrap()
            .starts_with("iracing/")
    );

    // The capture target: a test mapping is not a window, so with the simulator not running
    // on this machine there is none.
    if !game_running(&SPEC.game) {
        assert_eq!(
            remote_code(call(&rig, "connector.capture_target", json!({}))),
            "game_not_running"
        );
    }

    let ping = call(
        &rig,
        "connector.action",
        json!({"code": "test.ping", "data": {}}),
    )
    .unwrap();
    assert_eq!(
        ping,
        json!({"pong": "pong", "announced": true, "message_id": "m1"})
    );
    let quiet = call(
        &rig,
        "connector.action",
        json!({"code": "test.ping", "data": {"say": false}}),
    )
    .unwrap();
    assert_eq!(quiet, json!({"pong": "pong", "announced": false}));
    let said = call(
        &rig,
        "connector.action",
        json!({"code": "test.announce", "data": {"code": "engineer_test",
               "params": [{"name": "text", "value": "radio test"}]}}),
    )
    .unwrap();
    assert_eq!(said, json!({"announced": true, "message_id": "m1"}));
    let calls = rig.fake.calls();
    let last = calls
        .iter()
        .rev()
        .find(|c| c.method == "pitwall.announce")
        .unwrap();
    let p = stintmate_connector_testing::to_json(&last.params);
    assert_eq!(p["code"], "engineer_test");
    assert_eq!(p["params"]["text"], "radio test");
    let announces = calls
        .iter()
        .filter(|c| c.method == "pitwall.announce")
        .count();
    assert_eq!(announces, 2, "the ping with say=false announces nothing");

    for (data, why) in [
        (json!({"code": "nope"}), "an unknown code"),
        (json!({"code": "engineer_test"}), "a missing param"),
        (
            json!({"code": "engineer_test", "params": [{"name": "nope", "value": "1"}]}),
            "an unknown param",
        ),
        (
            json!({"code": "engineer_test", "variant": 300, "params": []}),
            "a bad variant",
        ),
    ] {
        assert_eq!(
            remote_code(call(
                &rig,
                "connector.action",
                json!({"code": "test.announce", "data": data})
            )),
            "bad_request",
            "{why}"
        );
    }
    assert_eq!(
        remote_code(call(
            &rig,
            "connector.action",
            json!({"code": "nope", "data": {}})
        )),
        "unknown_action"
    );
    assert_eq!(rig.stop(&mut child), Some(0));
    rig.fake.stop();
}
