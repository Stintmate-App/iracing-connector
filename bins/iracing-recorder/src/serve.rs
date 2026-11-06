//! What the recorder serves to Pitwall besides the live frame: `connector.capture_target` and
//! `connector.action`, and the announcer with its declarations. Both methods run on the handler
//! pool, never on the loop.

use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use iracing_core::SPEC;
use iracing_core::announce::{CATALOG, CATALOG_ID};
use iracing_core::guidance;
use serde_json::{Map, Value as Json, json};
use stintmate_connector_link::announcer::{Announcement, Announcer, declare_connection};
use stintmate_connector_link::link::{PitwallLink, Transport, to_value};
use stintmate_connector_platform::window::{Scope, WindowInfo, find_game_window, list_windows};
use stintmate_connector_wire::actions::{
    ACCEPT_TIMEOUT_S, TEST_ANNOUNCE, TEST_PING, standard_actions,
};
use stintmate_connector_wire::catalog::{AnnounceCatalog, check_params};
use stintmate_ipc::MethodOpts;
use stintmate_ipc::codec::{self, Value};
use stintmate_ipc::error::RemoteError;

/// The announcement catalog, parsed and validated once.
pub fn catalog() -> &'static AnnounceCatalog {
    static CATALOG_CELL: OnceLock<AnnounceCatalog> = OnceLock::new();
    CATALOG_CELL.get_or_init(|| {
        AnnounceCatalog::parse(CATALOG, CATALOG_ID).expect("the announcement catalog is valid")
    })
}

/// The actions declared to Pitwall.
pub fn actions() -> &'static Json {
    static ACTIONS: OnceLock<Json> = OnceLock::new();
    ACTIONS.get_or_init(|| standard_actions(&catalog().codes()))
}

/// An IPC value as JSON (`bin` as an array of bytes, `ext` as null).
pub fn to_json(v: &Value) -> Json {
    match v {
        Value::Nil | Value::Ext(..) => Json::Null,
        Value::Boolean(b) => json!(b),
        Value::Integer(i) => i.as_i64().map_or_else(|| json!(i.as_u64()), |x| json!(x)),
        Value::F32(f) => json!(f64::from(*f)),
        Value::F64(f) => json!(f),
        Value::String(s) => json!(s.as_str().unwrap_or_default()),
        Value::Binary(b) => json!(b),
        Value::Array(a) => Json::Array(a.iter().map(to_json).collect()),
        Value::Map(m) => Json::Object(
            m.iter()
                .map(|(k, v)| (k.as_str().unwrap_or_default().to_owned(), to_json(v)))
                .collect(),
        ),
    }
}

/// `connector.capture_target {}` -> `{hwnd, pid, title}` among `windows`: the simulator's
/// largest window, or the protocol's errors.
pub fn capture_target_of(windows: &[WindowInfo]) -> Result<Value, RemoteError> {
    let w = find_game_window(&SPEC.game, windows).ok_or_else(|| {
        RemoteError::new(
            "game_not_running",
            "iRacing is not running (no game window).",
        )
    })?;
    if w.minimized {
        return Err(RemoteError::new(
            "window_minimized",
            "The iRacing window is minimized: restore it (it needs no focus).",
        ));
    }
    Ok(to_value(
        &json!({"hwnd": w.hwnd, "pid": w.pid, "title": w.title}),
    ))
}

/// `connector.capture_target` for the live desktop (EnumWindows, off the loop).
pub fn capture_target() -> Result<Value, RemoteError> {
    capture_target_of(&list_windows(Scope::AllVisible))
}

/// The static string of a catalog code.
fn static_code(code: &str) -> Option<&'static str> {
    catalog().codes().into_iter().find(|c| *c == code)
}

/// Serves `connector.action` (`test.ping`, `test.announce`).
pub struct ActionServer {
    announcer: Arc<Announcer>,
}

impl ActionServer {
    /// A server announcing through `announcer` (its rate window bounds the radio).
    pub fn new(announcer: Arc<Announcer>) -> Self {
        Self { announcer }
    }

    /// One call: `{code, data?, call_id?}`.
    pub fn call(&self, params: &Value) -> Result<Value, RemoteError> {
        let p = to_json(params);
        let code = p.get("code").and_then(Json::as_str).unwrap_or_default();
        let data = p.get("data").cloned().unwrap_or(Json::Null);
        tracing::info!(
            "Action {code} (call {})",
            p.get("call_id").and_then(Json::as_str).unwrap_or("-")
        );
        let out = match code {
            TEST_PING => self.ping(&data),
            TEST_ANNOUNCE => self.announce(&data)?,
            other => {
                return Err(RemoteError::new(
                    "unknown_action",
                    format!("this connector does not serve {other:?}"),
                ));
            }
        };
        Ok(to_value(&out))
    }

    fn send(&self, code: &'static str, variant: u8, params: Map<String, Json>) -> Json {
        let a = Announcement {
            code,
            variant,
            params,
            data: None,
        };
        let wait = Duration::from_secs_f64(ACCEPT_TIMEOUT_S);
        match self.announcer.announce_wait(&a, wait) {
            Ok(res) => {
                let mut out = json!({"announced": true});
                if let Some(id) = codec::get(&res, "message_id").and_then(Value::as_str)
                    && !id.is_empty()
                {
                    out["message_id"] = json!(id.chars().take(64).collect::<String>());
                }
                out
            }
            Err(why) => {
                let why = if why.is_empty() {
                    "not announced".into()
                } else {
                    why
                };
                json!({"announced": false, "refused": why.chars().take(300).collect::<String>()})
            }
        }
    }

    fn ping(&self, data: &Json) -> Json {
        if data.get("say") == Some(&Json::Bool(false)) {
            return json!({"pong": "pong", "announced": false});
        }
        let mut sent = self.send("radio_check", 0, Map::new());
        if let Some(m) = sent.as_object_mut() {
            m.remove("refused"); // the ping's answer has no reason field
        }
        let mut out = json!({"pong": "pong"});
        if let (Some(o), Some(s)) = (out.as_object_mut(), sent.as_object()) {
            o.extend(s.clone());
        }
        out
    }

    fn announce(&self, data: &Json) -> Result<Json, RemoteError> {
        let bad = |m: String| RemoteError::new("bad_request", m);
        let code = data.get("code").and_then(Json::as_str).unwrap_or_default();
        let mut pairs = Vec::new();
        for item in data
            .get("params")
            .and_then(Json::as_array)
            .into_iter()
            .flatten()
        {
            let (Some(n), Some(v)) = (
                item.get("name").and_then(Json::as_str),
                item.get("value").and_then(Json::as_str),
            ) else {
                return Err(bad("params: each entry needs name and value".into()));
            };
            pairs.push(format!("{n}={v}"));
        }
        let cat = catalog();
        let params = cat
            .parse_params(code, &pairs)
            .map_err(|e| bad(format!("params: {e}")))?;
        if let Some(problem) = check_params(code, &Json::Object(params.clone()), cat.messages()) {
            return Err(bad(problem));
        }
        let variant = match data.get("variant") {
            None | Some(Json::Null) => 0,
            Some(v) => v
                .as_u64()
                .and_then(|n| u8::try_from(n).ok())
                .ok_or_else(|| bad("variant: must be an integer 0..255".into()))?,
        };
        let code = static_code(code).ok_or_else(|| bad(format!("unknown code {code:?}")))?;
        Ok(self.send(code, variant, params))
    }
}

/// The name of the session being recorded, shared with the announcer: the loop writes it when
/// it changes (twice a second at most it is compared, not locked).
pub type SessionKeyCell = Arc<Mutex<Option<String>>>;

/// Creates the announcer on `link`, declares catalog, guidance and actions at every new
/// connection and registers `connector.capture_target` and `connector.action` on its client.
/// Call before the link starts.
pub fn install(
    link: &Arc<PitwallLink>,
    key: SessionKeyCell,
    start: Instant,
) -> Result<Arc<Announcer>, String> {
    let transport: Arc<dyn Transport> = link.clone();
    let announcer = Arc::new(Announcer::new(
        transport,
        catalog(),
        SPEC.id,
        move || key.lock().unwrap_or_else(|p| p.into_inner()).clone(),
        move || start.elapsed().as_secs_f64(),
    ));
    let (l, a) = (link.clone(), announcer.clone());
    link.on_connected(move || declare_connection(l.as_ref(), &a, guidance::items(), actions()));
    let a = announcer.clone();
    link.on_disconnected(move || a.lost());
    let io = |e: stintmate_ipc::Error| e.to_string();
    link.client
        .method(
            "connector.capture_target",
            MethodOpts::default(),
            |_ctx, _params| capture_target(),
        )
        .map_err(io)?;
    let server = ActionServer::new(announcer.clone());
    link.client
        .method("connector.action", MethodOpts::default(), move |_ctx, p| {
            server.call(&p)
        })
        .map_err(io)?;
    Ok(announcer)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn win(minimized: bool) -> WindowInfo {
        WindowInfo {
            hwnd: 4242,
            pid: 77,
            exe: "iracingsim64dx11.exe".into(),
            title: "iRacing.com Simulator".into(),
            class: "SimWinClass".into(),
            area: if minimized { 0 } else { 1920 * 1080 },
            minimized,
        }
    }

    #[test]
    fn capture_target_is_hwnd_pid_title_or_the_protocols_errors() {
        let v = to_json(&capture_target_of(&[win(false)]).unwrap());
        assert_eq!(
            v,
            json!({"hwnd": 4242, "pid": 77, "title": "iRacing.com Simulator"})
        );
        let e = capture_target_of(&[]).unwrap_err();
        assert_eq!(e.code, "game_not_running");
        let e = capture_target_of(&[win(true)]).unwrap_err();
        assert_eq!(e.code, "window_minimized");
        assert!(e.message.contains("restore it"), "{}", e.message);
    }

    #[test]
    fn the_declared_actions_name_the_catalogs_codes() {
        let list = actions().as_array().expect("an array");
        assert_eq!(list.len(), 2);
        let codes = list[1]["params"]["properties"]["code"]["enum"]
            .as_array()
            .expect("an enum");
        assert!(codes.iter().any(|c| c == "radio_check"));
        assert_eq!(codes.len(), catalog().codes().len());
    }
}
