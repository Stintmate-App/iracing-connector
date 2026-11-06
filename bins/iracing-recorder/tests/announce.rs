//! Announcements: the catalog at connect, the events of a drive arriving as `pitwall.announce`,
//! a bad test announcement refused, the windows of the runtime, and a flickering `CarLeftRight`
//! staying inside the spotter limit.

mod common;
use common::*;
use iracing_core::announce::Calls;
use iracing_core::announce::detector::Detector;
use iracing_core::announce::spotter::Spotter;
use iracing_core::codes::SessionKind;
use iracing_core::meta::SessionMeta;
use iracing_core::testing::{LapProfile, Scenario, YamlBuilder, table_of};
use iracing_core::tick::Tick;
use iracing_core::vars::Vars;
use iracing_recorder::announce::{Fired, announcement, channel};
use irsdk::defs::session_flags as flag;
use irsdk::row::Row;
use serde_json::json;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use stintmate_connector_link::announcer::Announcer;
use stintmate_connector_link::link::{CallError, Transport};
use stintmate_connector_testing::to_json;
use stintmate_ipc::codec::Value;

/// The `pitwall.announce` calls the fake Pitwall got: (code, params).
fn announced(rig: &Rig) -> Vec<(String, serde_json::Value)> {
    rig.fake
        .calls()
        .iter()
        .filter(|c| c.method == "pitwall.announce")
        .map(|c| {
            let p = to_json(&c.params);
            (p["code"].as_str().unwrap().to_owned(), p)
        })
        .collect()
}

/// Two quiet laps, then `f`, then a lap more.
fn drive(f: impl FnOnce(&mut Scenario)) -> Scenario {
    let mut s = Scenario::new(YamlBuilder::new());
    s.garage(0.5).to_stall().drive_out();
    s.lap(LapProfile::new(25.0));
    s.lap(LapProfile::new(25.0));
    f(&mut s);
    s.lap(LapProfile::new(25.0));
    s
}

#[test]
fn the_catalog_is_declared_and_a_bad_test_announcement_is_refused() {
    let rig = rig();
    let mut child = rig.spawn();
    assert!(
        rig.fake.wait_for(
            |c| c.iter().any(|x| x.method == "pitwall.catalog.declare"),
            15
        ),
        "no catalog"
    );
    let catalog = rig
        .fake
        .calls()
        .into_iter()
        .find(|c| c.method == "pitwall.catalog.declare")
        .unwrap();
    assert_eq!(
        to_json(&catalog.params)["catalog"]["catalog"],
        "connector.iracing"
    );
    let bad = call(
        &rig,
        "connector.action",
        json!({"code": "test.announce", "data": {"code": "incident",
               "params": [{"name": "points", "value": "many"}]}}),
    );
    assert_eq!(remote_code(bad), "bad_request");
    assert!(announced(&rig).is_empty(), "a refused test was announced");
    rig.stop(&mut child);
    rig.fake.stop();
}

#[test]
fn a_yellow_flag_and_an_incident_arrive_as_announcements() {
    let rig = rig();
    let s = drive(|s| {
        s.flags(flag::YELLOW, 3.0);
        s.incident(2, 0);
        s.flags(0, 3.0);
    });
    let _replay = Replay::start_at(&rig, s, 120.0);
    let mut child = rig.spawn();
    assert!(
        wait_for(60, || {
            let got = announced(&rig);
            got.iter().any(|a| a.0.starts_with("flag_yellow"))
                && got.iter().any(|a| a.0 == "incident")
        }),
        "not announced: {:?}",
        announced(&rig).iter().map(|a| &a.0).collect::<Vec<_>>()
    );
    let got = announced(&rig);
    let inc = got.iter().find(|a| a.0 == "incident").unwrap();
    assert_eq!(inc.1["params"], json!({"points": 2, "total": 2}));
    assert!(inc.1["data"]["session_time"].is_number());
    assert!(inc.1["session_key"].is_string(), "{}", inc.1);
    rig.stop(&mut child);
    rig.fake.stop();
}

#[test]
fn the_eleventh_general_announcement_in_a_minute_is_dropped() {
    let rig = rig();
    let mut child = rig.spawn();
    let mut said = 0;
    for _ in 0..11 {
        let r = call(
            &rig,
            "connector.action",
            json!({"code": "test.ping", "data": {}}),
        )
        .unwrap();
        if r["announced"] == json!(true) {
            said += 1;
        }
    }
    assert_eq!(said, 10);
    assert_eq!(announced(&rig).len(), 10);
    rig.stop(&mut child);
    rig.fake.stop();
}

/// A scenario whose `CarLeftRight` flickers for `secs` seconds of driving.
fn flicker(secs: f64) -> Scenario {
    let mut s = Scenario::new(YamlBuilder::new());
    s.garage(0.5).to_stall().drive_out();
    s.lap(LapProfile::new(25.0));
    s.lap(LapProfile::new(25.0));
    // Each side shows a car for 0.05 s every 1.75 s, the sides half a period apart: the shortest
    // cycle the spotter can follow (the car, a clear after 0.15 s, a bounce window of 1.5 s).
    let steps = (secs / 0.05) as u32;
    for k in 0..steps {
        let t = f64::from(k) * 0.05;
        let left = t % 1.75 < 0.06;
        let right = (t + 0.875) % 1.75 < 0.06;
        let word = match (left, right) {
            (true, true) => 4,
            (true, false) => 2,
            (false, true) => 3,
            (false, false) => 1,
        };
        s.car_left_right(word, 0.05);
    }
    s.lap(LapProfile::new(25.0));
    s
}

#[test]
fn flickering_car_left_right_stays_inside_the_spotter_limit_on_the_wire() {
    let rig = rig();
    let _replay = Replay::start_at(&rig, flicker(60.0), 240.0);
    let mut child = rig.spawn();
    assert!(
        wait_for(60, || announced(&rig)
            .iter()
            .any(|a| a.0.starts_with("spotter_"))),
        "the spotter never spoke"
    );
    // The replay ends and the sender settles.
    std::thread::sleep(Duration::from_secs(30));
    let got = announced(&rig);
    let spotter = got.iter().filter(|a| a.0.starts_with("spotter_")).count();
    let general = got.len() - spotter;
    assert!(spotter >= 20, "only {spotter} spotter calls");
    assert!(spotter <= 120, "{spotter} spotter calls in a minute");
    assert!(general <= 10, "{general} general calls in a minute");
    rig.stop(&mut child);
    rig.fake.stop();
}

/// A link that counts what is sent and answers everything.
#[derive(Default)]
struct Counting {
    sent: Mutex<Vec<String>>,
}

impl Transport for Counting {
    fn connected(&self) -> bool {
        true
    }
    fn wait_connected(&self, _: Duration) -> bool {
        true
    }
    fn call(&self, _: &str, _: Value, _: Option<&str>, _: Duration) -> Result<Value, CallError> {
        Ok(Value::Nil)
    }
    fn call_async(
        &self,
        method: &str,
        params: Value,
        _: Duration,
        _: Box<dyn FnOnce(Result<Value, CallError>) + Send>,
    ) -> Result<(), CallError> {
        assert_eq!(method, "pitwall.announce");
        let code = to_json(&params)["code"]
            .as_str()
            .unwrap_or_default()
            .to_owned();
        self.sent.lock().unwrap().push(code);
        Ok(())
    }
}

/// Over a minute of adversarial flicker the spotter offers more than 120 calls; the runtime's
/// window keeps the wire at 120. The session clock drives the window, so the run is exact.
#[test]
fn the_window_cuts_a_flickering_spotter_to_120_a_minute() {
    let s = flicker(59.0);
    let table = table_of(s.layout());
    let vars = Vars::resolve(&table).expect("vars");
    let mut meta = SessionMeta::empty(0);
    meta.kind = SessionKind::Practice;
    meta.player_idx = 1;
    let (mut det, mut spot) = (Detector::new(2.0), Spotter::new());
    let (mut tick, mut calls) = (Tick::new(), Calls::new());
    let link = Arc::new(Counting::default());
    let now = Arc::new(AtomicU64::new(0));
    let clock = now.clone();
    let announcer = Arc::new(Announcer::new(
        link.clone(),
        iracing_recorder::serve::catalog(),
        "iracing",
        || None,
        move || f64::from_bits(clock.load(Ordering::Relaxed)),
    ));
    assert!(announcer.declare());
    let (outlet, rx) = channel();
    let (mut offered, mut first) = (0, None);
    for (i, bytes) in s.rows().iter().enumerate() {
        let mut row = Row::from_slice(bytes);
        row.set_tick(i as i32 + 1);
        tick.fill(&vars, &row);
        tick.connected = true;
        let t0 = *first.get_or_insert(tick.session_time);
        now.store((tick.session_time - t0).to_bits(), Ordering::Relaxed);
        calls.clear();
        det.step(&tick, Some(&meta), &mut calls);
        spot.step(&tick, Some(&meta), None, &mut calls);
        for c in calls.as_slice() {
            offered += 1;
            outlet.push(Fired {
                call: c.clone(),
                session_time: tick.session_time,
                lap: tick.lap,
                lap_dist: None,
            });
        }
        while let Ok(f) = rx.try_recv() {
            announcer.announce(&announcement(&f));
        }
    }
    let sent = link.sent.lock().unwrap().clone();
    let spotter = sent.iter().filter(|c| c.starts_with("spotter_")).count();
    let general = sent.len() - spotter;
    assert!(offered > 120, "only {offered} calls offered: no pressure");
    assert!(spotter <= 120, "{spotter} spotter calls sent");
    assert!(general <= 10, "{general} general calls sent");
    assert_eq!(outlet.dropped(), 0);
    assert!(announcer.stats().dropped > 0, "the window never cut a call");
}
