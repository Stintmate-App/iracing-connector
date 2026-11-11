//! Which disk telemetry files belong to a session, and whether they are done being written.
//!
//! A file is chosen by what its headers say and by the wall clock, never by its name: the name
//! format is known for one car and track only. Everything is a function of the facts it is
//! given; the caller reads the folder, the file metadata and the clock.

use std::io::Read;

use irsdk::scan::Scan;

use super::file::PositionsHeader;

/// How long a file's size and modification time must stay the same before it is read, in
/// seconds.
pub const STABLE_SECS: i64 = 60;

/// How early a file may start before the connector's session opened, in seconds. The connector
/// learns of a session a moment after the simulator started it, and logging may start with it.
pub const START_SLACK_SECS: i64 = 30;

/// What the headers of a disk file say, with its size and modification time.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileFacts {
    /// The sub-header's start date of the file, Unix seconds.
    pub start_unix: i64,
    /// `WeekendInfo.TrackID`.
    pub track_id: Option<i64>,
    /// The player's `CarPath`.
    pub car_path: Option<String>,
    /// `WeekendInfo.SubSessionID`.
    pub sub_session_id: Option<i64>,
    /// The size of the file in bytes.
    pub size: u64,
    /// The file's modification time, Unix seconds.
    pub modified_unix: i64,
    /// Whether the size is exactly the first buffer offset plus the record count times the row
    /// length, with a record count above zero.
    pub complete: bool,
}

impl FileFacts {
    /// The facts of an opened file. `modified_unix` comes from the file's metadata.
    pub fn of<R: Read>(scan: &Scan<R>, modified_unix: i64) -> FileFacts {
        let doc = scan.session();
        let weekend = doc.info.weekend.as_ref();
        FileFacts {
            start_unix: scan.disk().session_start_date,
            track_id: weekend.and_then(|w| w.track_id).or(doc.fallback.track_id),
            car_path: doc.fallback.player_car_path.clone(),
            sub_session_id: weekend
                .and_then(|w| w.sub_session_id)
                .or(doc.fallback.sub_session_id),
            size: scan.file_len(),
            modified_unix,
            complete: scan.is_complete(),
        }
    }
}

/// Why a file is not one of the session's.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NotMine {
    /// The session has not ended: its files may still be growing.
    SessionOpen,
    /// The file was last written before the session opened.
    OlderThanSession,
    /// The file starts outside the session's wall-clock span.
    OutsideSpan,
    /// The track differs, or the file does not name one.
    Track,
    /// The player's car differs, or the file does not name one.
    Car,
    /// The sub-session differs.
    SubSession,
}

/// Whether a file belongs to the session `header` describes, which ended at `closed_unix`.
///
/// The file must have been written after the session opened and start inside
/// `[opened - START_SLACK_SECS, closed]`; its track and car must be the session's, and its
/// sub-session too when the file names one.
pub fn belongs(
    header: &PositionsHeader,
    closed_unix: Option<i64>,
    f: &FileFacts,
) -> Result<(), NotMine> {
    let closed = closed_unix.ok_or(NotMine::SessionOpen)?;
    if f.modified_unix < header.opened_unix {
        return Err(NotMine::OlderThanSession);
    }
    if f.start_unix < header.opened_unix.saturating_sub(START_SLACK_SECS) || f.start_unix > closed {
        return Err(NotMine::OutsideSpan);
    }
    if f.track_id != Some(header.track_id) {
        return Err(NotMine::Track);
    }
    if f.car_path.as_deref() != Some(header.car_path.as_str()) {
        return Err(NotMine::Car);
    }
    if f.sub_session_id.is_some_and(|s| s != header.sub_session_id) {
        return Err(NotMine::SubSession);
    }
    Ok(())
}

/// Watches one file until its size and modification time have stayed the same for
/// [`STABLE_SECS`].
#[derive(Clone, Debug, Default)]
pub struct Stability {
    seen: Option<(u64, i64)>,
    since: i64,
}

impl Stability {
    /// A watch that has seen nothing.
    pub fn new() -> Stability {
        Stability::default()
    }

    /// Records the file's `size` and `modified_unix` seen at `now_s` (any clock that does not go
    /// back, in seconds) and says whether they have been the same for [`STABLE_SECS`].
    pub fn observe(&mut self, size: u64, modified_unix: i64, now_s: i64) -> bool {
        if self.seen != Some((size, modified_unix)) {
            self.seen = Some((size, modified_unix));
            self.since = now_s;
        }
        now_s.saturating_sub(self.since) >= STABLE_SECS
    }
}

/// Whether to read a file now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Readiness {
    /// Read it.
    Ready,
    /// Not complete, or changed within the last [`STABLE_SECS`]: look again at the next wake.
    Wait,
}

/// Whether `f` may be read now: complete and unchanged for [`STABLE_SECS`], by `stability`
/// (one per file, kept between wakes) at `now_s`.
pub fn readiness(f: &FileFacts, stability: &mut Stability, now_s: i64) -> Readiness {
    let stable = stability.observe(f.size, f.modified_unix, now_s);
    if f.complete && stable {
        Readiness::Ready
    } else {
        Readiness::Wait
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header() -> PositionsHeader {
        PositionsHeader {
            session_key: "k".into(),
            session_num: 1,
            session_unique_id: 2,
            sub_session_id: 0,
            track_id: 289,
            car_path: "mx5 mx52016".into(),
            opened_unix: 10_000,
        }
    }

    fn facts() -> FileFacts {
        FileFacts {
            start_unix: 10_100,
            track_id: Some(289),
            car_path: Some("mx5 mx52016".into()),
            sub_session_id: Some(0),
            size: 1000,
            modified_unix: 10_900,
            complete: true,
        }
    }

    #[test]
    fn a_file_of_the_session_is_chosen_by_its_headers() {
        let h = header();
        assert_eq!(belongs(&h, Some(11_000), &facts()), Ok(()));
        assert_eq!(belongs(&h, None, &facts()), Err(NotMine::SessionOpen));
        let mut f = facts();
        f.modified_unix = 9_999;
        assert_eq!(
            belongs(&h, Some(11_000), &f),
            Err(NotMine::OlderThanSession)
        );
        let mut f = facts();
        f.start_unix = 10_000 - START_SLACK_SECS;
        assert_eq!(belongs(&h, Some(11_000), &f), Ok(()));
        f.start_unix -= 1;
        assert_eq!(belongs(&h, Some(11_000), &f), Err(NotMine::OutsideSpan));
        let mut f = facts();
        f.start_unix = 11_001;
        assert_eq!(belongs(&h, Some(11_000), &f), Err(NotMine::OutsideSpan));
        let mut f = facts();
        f.track_id = Some(1);
        assert_eq!(belongs(&h, Some(11_000), &f), Err(NotMine::Track));
        f.track_id = None;
        assert_eq!(belongs(&h, Some(11_000), &f), Err(NotMine::Track));
        let mut f = facts();
        f.car_path = Some("other".into());
        assert_eq!(belongs(&h, Some(11_000), &f), Err(NotMine::Car));
        let mut f = facts();
        f.sub_session_id = Some(77);
        assert_eq!(belongs(&h, Some(11_000), &f), Err(NotMine::SubSession));
        f.sub_session_id = None;
        assert_eq!(belongs(&h, Some(11_000), &f), Ok(()));
    }

    #[test]
    fn a_file_is_read_when_complete_and_unchanged_for_a_minute() {
        let mut w = Stability::new();
        let f = facts();
        assert_eq!(readiness(&f, &mut w, 1000), Readiness::Wait);
        assert_eq!(
            readiness(&f, &mut w, 1000 + STABLE_SECS - 1),
            Readiness::Wait
        );
        assert_eq!(readiness(&f, &mut w, 1000 + STABLE_SECS), Readiness::Ready);
        // It grew: the minute starts again.
        let mut g = f.clone();
        g.size += 1081;
        assert_eq!(readiness(&g, &mut w, 1100), Readiness::Wait);
        assert_eq!(readiness(&g, &mut w, 1100 + STABLE_SECS), Readiness::Ready);
        // Only the modification time moved.
        let mut m = g.clone();
        m.modified_unix += 5;
        assert_eq!(readiness(&m, &mut w, 1200), Readiness::Wait);
        // Unchanged for a long time but not complete (the size does not fit its count).
        let mut w = Stability::new();
        let mut c = f.clone();
        c.complete = false;
        readiness(&c, &mut w, 0);
        assert_eq!(readiness(&c, &mut w, 10_000), Readiness::Wait);
    }
}
