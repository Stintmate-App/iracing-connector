//! From the detectors to Pitwall. The sampling thread pushes each fired [`Call`] into a bounded
//! channel whose slots exist from the start (a push moves the call, allocates nothing and never
//! waits); a sender thread of its own builds the announcement and hands it to the runtime's
//! [`Announcer`], whose windows (10 messages in any 60 s, 120 of the `spotter` group) drop what is
//! over the limit. A full channel drops the call as well: a late call about a car alongside or a
//! flag is worse than none.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TrySendError, sync_channel};
use std::thread::JoinHandle;

use iracing_core::announce::{Call, ParamValue};
use serde_json::{Map, Value as Json, json};
use stintmate_connector_link::announcer::{Announcement, Announcer};

/// Calls that can wait for the sender thread. A row makes at most 8 calls and the windows pass
/// far fewer per minute, so a full channel means the sender is stuck.
pub const CAPACITY: usize = 64;

/// A fired call with the facts about the moment it fired.
#[derive(Clone, Debug)]
pub struct Fired {
    /// The call.
    pub call: Call,
    /// The session clock of the row, in seconds.
    pub session_time: f64,
    /// The player's lap number.
    pub lap: i32,
    /// Metres from the start line, when the track length is known.
    pub lap_dist: Option<f32>,
}

/// The sampling thread's end of the channel.
pub struct Outlet {
    tx: SyncSender<Fired>,
    dropped: Arc<AtomicU64>,
}

impl Outlet {
    /// Passes a call on without waiting; a full channel counts it as dropped.
    pub fn push(&self, fired: Fired) {
        if let Err(TrySendError::Full(_) | TrySendError::Disconnected(_)) = self.tx.try_send(fired)
        {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Calls dropped because the sender thread did not keep up.
    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }
}

/// A channel of [`CAPACITY`] preallocated slots.
pub fn channel() -> (Outlet, Receiver<Fired>) {
    let (tx, rx) = sync_channel(CAPACITY);
    (
        Outlet {
            tx,
            dropped: Arc::default(),
        },
        rx,
    )
}

/// The announcement a fired call makes: the catalog's params and the facts for the engineer.
pub fn announcement(f: &Fired) -> Announcement {
    let params: Map<String, Json> = f
        .call
        .set_params()
        .map(|(name, value)| {
            let v = match value {
                ParamValue::Int(i) => json!(i),
                ParamValue::Str(s) => json!(s),
            };
            ((*name).to_owned(), v)
        })
        .collect();
    let mut data = Map::new();
    data.insert("session_time".into(), json!(round(f.session_time, 1)));
    data.insert("lap".into(), json!(f.lap));
    if let Some(d) = f.lap_dist {
        data.insert("lap_dist".into(), json!(round(f64::from(d), 0)));
    }
    Announcement {
        code: f.call.code,
        variant: f.call.variant,
        params,
        data: Some(data),
    }
}

fn round(v: f64, places: i32) -> f64 {
    let k = 10f64.powi(places);
    (v * k).round() / k
}

/// Runs until every [`Outlet`] is dropped, announcing what arrives.
pub fn spawn_sender(
    rx: Receiver<Fired>,
    announcer: Arc<Announcer>,
) -> std::io::Result<JoinHandle<()>> {
    std::thread::Builder::new()
        .name("announce".into())
        .spawn(move || {
            while let Ok(fired) = rx.recv() {
                announcer.announce(&announcement(&fired));
            }
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_call_becomes_an_announcement_with_its_facts() {
        let f = Fired {
            call: Call::new("incident").int("points", 2).int("total", 6),
            session_time: 123.456,
            lap: 4,
            lap_dist: Some(1234.6),
        };
        let a = announcement(&f);
        assert_eq!(a.code, "incident");
        assert_eq!(a.params["points"], 2);
        assert_eq!(a.params["total"], 6);
        let d = a.data.unwrap();
        assert_eq!(d["session_time"], 123.5);
        assert_eq!(d["lap"], 4);
        assert_eq!(d["lap_dist"], 1235.0);
    }

    #[test]
    fn a_full_channel_drops_instead_of_waiting() {
        let (out, _rx) = channel();
        let fired = Fired {
            call: Call::new("flag_red"),
            session_time: 1.0,
            lap: 1,
            lap_dist: None,
        };
        for _ in 0..CAPACITY + 5 {
            out.push(fired.clone());
        }
        assert_eq!(out.dropped(), 5);
    }
}
