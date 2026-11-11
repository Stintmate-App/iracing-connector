//! The positions file: what the recorder keeps of a session's laps until their exact positions
//! are found.
//!
//! One file per session, appended to. It starts with a header and continues with records, each
//! `tag (u8)`, `length (u32)`, payload, all little-endian:
//!
//! | Tag | Record | Payload |
//! |---|---|---|
//! | 1 | a lap | `lap_seq u64`, flags `u8` (bit 0: start crossing, bit 1: end crossing), start crossing `f64`, end crossing `f64`, `n u32`, `n` ticks `i32`, `n` `LapDistPct` `f32` |
//! | 2 | the session closed | wall-clock time `i64` (Unix seconds) |
//! | 3 | a lap resolved | `lap_seq u64` |
//!
//! A record cut short at the end of the file (a write that did not finish) is ignored. A record
//! with an unknown tag is skipped, so a later version can add records.

use std::collections::BTreeSet;

/// The first bytes of every positions file.
const MAGIC: &[u8; 8] = b"STMPOS\0\0";

/// The version of the format this module writes and reads.
pub const FORMAT_VERSION: u32 = 1;

/// How long a positions file is kept, in seconds (7 days), counted from the session's end, or
/// from its start while it is not closed.
pub const EXPIRY_SECS: i64 = 7 * 24 * 3600;

const TAG_LAP: u8 = 1;
const TAG_CLOSED: u8 = 2;
const TAG_RESOLVED: u8 = 3;

/// The longest string a header may hold.
const MAX_STRING: usize = 4096;

/// What identifies the session a positions file belongs to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PositionsHeader {
    /// The session key the laps were written under.
    pub session_key: String,
    /// `SessionNum`.
    pub session_num: i32,
    /// `SessionUniqueID`.
    pub session_unique_id: i32,
    /// `SubSessionID` (0 offline).
    pub sub_session_id: i64,
    /// The simulator's track id (`TrackID`).
    pub track_id: i64,
    /// The player's `CarPath`.
    pub car_path: String,
    /// The wall-clock time the session was opened, Unix seconds.
    pub opened_unix: i64,
}

/// The address of the samples of one lap, as recorded.
#[derive(Clone, Debug, PartialEq)]
pub struct LapRecord {
    /// The number of the lap's write in the session (`lap_seq`).
    pub lap_seq: u64,
    /// The session time at which the lap started at the line, when it did.
    pub start_crossing: Option<f64>,
    /// The session time at which the lap ended at the line, when it did.
    pub end_crossing: Option<f64>,
    /// `SessionTick` of every sample.
    pub ticks: Vec<i32>,
    /// `LapDistPct` of every sample.
    pub pct: Vec<f32>,
}

/// Why a file could not be read as a positions file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DecodeError {
    /// The file does not start with the format's magic bytes.
    NotAPositionsFile,
    /// The format version is not the one this module reads.
    UnsupportedVersion(u32),
    /// The header is cut short or holds an invalid value.
    BadHeader,
    /// A record is inconsistent (its counts do not fit its length).
    BadRecord,
}

impl std::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DecodeError::NotAPositionsFile => f.write_str("not a positions file"),
            DecodeError::UnsupportedVersion(v) => write!(f, "unsupported format version {v}"),
            DecodeError::BadHeader => f.write_str("the header is cut short or invalid"),
            DecodeError::BadRecord => f.write_str("a record is inconsistent"),
        }
    }
}

impl std::error::Error for DecodeError {}

/// What to do with a positions file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    /// Keep it: laps are still pending, or the session is still open.
    Keep,
    /// Delete it: every lap is resolved and the session is closed, or it is too old.
    Delete,
}

/// A decoded positions file.
#[derive(Clone, Debug, PartialEq)]
pub struct PositionsFile {
    /// The session the file belongs to.
    pub header: PositionsHeader,
    /// The laps, in the order they were appended.
    pub laps: Vec<LapRecord>,
    /// The wall-clock time the session closed, Unix seconds, when it did.
    pub closed_unix: Option<i64>,
    /// The `lap_seq` of the laps already resolved.
    pub resolved: BTreeSet<u64>,
    /// Whether the file ended in a record that was cut short (and ignored).
    pub truncated: bool,
}

impl PositionsFile {
    /// The laps not resolved yet.
    pub fn pending(&self) -> impl Iterator<Item = &LapRecord> {
        self.laps
            .iter()
            .filter(|l| !self.resolved.contains(&l.lap_seq))
    }

    /// Whether the session closed and every lap is resolved.
    pub fn is_finished(&self) -> bool {
        self.closed_unix.is_some() && self.pending().next().is_none()
    }

    /// Whether the session closed, so the disk files are complete or never will be.
    pub fn is_closed(&self) -> bool {
        self.closed_unix.is_some()
    }

    /// Keep or delete the file at wall-clock time `now_unix`: it is deleted when the session is
    /// closed and every lap is resolved, or after [`EXPIRY_SECS`].
    pub fn decide(&self, now_unix: i64) -> Decision {
        let since = self.closed_unix.unwrap_or(self.header.opened_unix);
        if self.is_finished() || now_unix.saturating_sub(since) > EXPIRY_SECS {
            Decision::Delete
        } else {
            Decision::Keep
        }
    }

    /// Reads a file image.
    pub fn decode(bytes: &[u8]) -> Result<PositionsFile, DecodeError> {
        let mut r = Reader { b: bytes, at: 0 };
        if r.take(MAGIC.len()).ok_or(DecodeError::NotAPositionsFile)? != MAGIC {
            return Err(DecodeError::NotAPositionsFile);
        }
        let version = r.u32().ok_or(DecodeError::BadHeader)?;
        if version != FORMAT_VERSION {
            return Err(DecodeError::UnsupportedVersion(version));
        }
        let header = (|| {
            Some(PositionsHeader {
                session_key: r.string()?,
                session_num: r.i32()?,
                session_unique_id: r.i32()?,
                sub_session_id: r.i64()?,
                track_id: r.i64()?,
                car_path: r.string()?,
                opened_unix: r.i64()?,
            })
        })()
        .ok_or(DecodeError::BadHeader)?;
        let mut file = PositionsFile {
            header,
            laps: Vec::new(),
            closed_unix: None,
            resolved: BTreeSet::new(),
            truncated: false,
        };
        while r.at < bytes.len() {
            let (Some(tag), Some(len)) = (r.u8(), r.u32()) else {
                file.truncated = true;
                break;
            };
            let Some(payload) = r.take(len as usize) else {
                file.truncated = true;
                break;
            };
            let mut p = Reader { b: payload, at: 0 };
            match tag {
                TAG_LAP => file
                    .laps
                    .push(decode_lap(&mut p).ok_or(DecodeError::BadRecord)?),
                TAG_CLOSED => file.closed_unix = Some(p.i64().ok_or(DecodeError::BadRecord)?),
                TAG_RESOLVED => {
                    file.resolved.insert(p.u64().ok_or(DecodeError::BadRecord)?);
                }
                _ => {}
            }
        }
        Ok(file)
    }
}

fn decode_lap(p: &mut Reader<'_>) -> Option<LapRecord> {
    let lap_seq = p.u64()?;
    let flags = p.u8()?;
    let start = p.f64()?;
    let end = p.f64()?;
    let n = p.u32()? as usize;
    // The counts must fit the payload exactly, whatever `n` claims.
    if p.b.len().checked_sub(p.at)? != n.checked_mul(8)? {
        return None;
    }
    let ticks = (0..n).map(|_| p.i32()).collect::<Option<Vec<_>>>()?;
    let pct = (0..n).map(|_| p.f32()).collect::<Option<Vec<_>>>()?;
    Some(LapRecord {
        lap_seq,
        start_crossing: (flags & 1 != 0).then_some(start),
        end_crossing: (flags & 2 != 0).then_some(end),
        ticks,
        pct,
    })
}

/// The bytes that start a file: magic, version and header.
pub fn encode_header(h: &PositionsHeader) -> Vec<u8> {
    let mut out = Vec::with_capacity(64 + h.session_key.len() + h.car_path.len());
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
    put_string(&mut out, &h.session_key);
    out.extend_from_slice(&h.session_num.to_le_bytes());
    out.extend_from_slice(&h.session_unique_id.to_le_bytes());
    out.extend_from_slice(&h.sub_session_id.to_le_bytes());
    out.extend_from_slice(&h.track_id.to_le_bytes());
    put_string(&mut out, &h.car_path);
    out.extend_from_slice(&h.opened_unix.to_le_bytes());
    out
}

/// The bytes of a lap record, to append. `ticks` and `pct` must have the same length.
pub fn encode_lap(l: &LapRecord) -> Vec<u8> {
    debug_assert_eq!(l.ticks.len(), l.pct.len());
    let n = l.ticks.len().min(l.pct.len());
    let mut payload = Vec::with_capacity(29 + n * 8);
    payload.extend_from_slice(&l.lap_seq.to_le_bytes());
    let flags = u8::from(l.start_crossing.is_some()) | (u8::from(l.end_crossing.is_some()) << 1);
    payload.push(flags);
    payload.extend_from_slice(&l.start_crossing.unwrap_or(0.0).to_le_bytes());
    payload.extend_from_slice(&l.end_crossing.unwrap_or(0.0).to_le_bytes());
    payload.extend_from_slice(&(n as u32).to_le_bytes());
    for t in &l.ticks[..n] {
        payload.extend_from_slice(&t.to_le_bytes());
    }
    for p in &l.pct[..n] {
        payload.extend_from_slice(&p.to_le_bytes());
    }
    record(TAG_LAP, &payload)
}

/// The bytes of the record that says the session closed at `unix` (wall clock, seconds).
pub fn encode_closed(unix: i64) -> Vec<u8> {
    record(TAG_CLOSED, &unix.to_le_bytes())
}

/// The bytes of the record that says the lap `lap_seq` is resolved.
pub fn encode_resolved(lap_seq: u64) -> Vec<u8> {
    record(TAG_RESOLVED, &lap_seq.to_le_bytes())
}

fn record(tag: u8, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(5 + payload.len());
    out.push(tag);
    out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    out.extend_from_slice(payload);
    out
}

fn put_string(out: &mut Vec<u8>, s: &str) {
    let bytes = &s.as_bytes()[..s.len().min(MAX_STRING)];
    out.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
    out.extend_from_slice(bytes);
}

struct Reader<'a> {
    b: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let end = self.at.checked_add(n)?;
        let s = self.b.get(self.at..end)?;
        self.at = end;
        Some(s)
    }
    fn u8(&mut self) -> Option<u8> {
        self.take(1).map(|s| s[0])
    }
    fn u32(&mut self) -> Option<u32> {
        self.take(4)?.try_into().ok().map(u32::from_le_bytes)
    }
    fn i32(&mut self) -> Option<i32> {
        self.take(4)?.try_into().ok().map(i32::from_le_bytes)
    }
    fn f32(&mut self) -> Option<f32> {
        self.take(4)?.try_into().ok().map(f32::from_le_bytes)
    }
    fn u64(&mut self) -> Option<u64> {
        self.take(8)?.try_into().ok().map(u64::from_le_bytes)
    }
    fn i64(&mut self) -> Option<i64> {
        self.take(8)?.try_into().ok().map(i64::from_le_bytes)
    }
    fn f64(&mut self) -> Option<f64> {
        self.take(8)?.try_into().ok().map(f64::from_le_bytes)
    }
    fn string(&mut self) -> Option<String> {
        let n = self.u32()? as usize;
        if n > MAX_STRING {
            return None;
        }
        String::from_utf8(self.take(n)?.to_vec()).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header() -> PositionsHeader {
        PositionsHeader {
            session_key: "k-1".into(),
            session_num: 1,
            session_unique_id: 2,
            sub_session_id: 0,
            track_id: 289,
            car_path: "mx5 mx52016".into(),
            opened_unix: 1_000_000,
        }
    }

    fn lap(seq: u64) -> LapRecord {
        LapRecord {
            lap_seq: seq,
            start_crossing: Some(10.5),
            end_crossing: None,
            ticks: vec![5, 6, 7],
            pct: vec![0.001, 0.002, 0.003],
        }
    }

    fn image(extra: &[Vec<u8>]) -> Vec<u8> {
        let mut b = encode_header(&header());
        for e in extra {
            b.extend_from_slice(e);
        }
        b
    }

    #[test]
    fn round_trip_with_appended_records() {
        let b = image(&[encode_lap(&lap(1)), encode_lap(&lap(2)), encode_resolved(1)]);
        let f = PositionsFile::decode(&b).unwrap();
        assert_eq!(f.header, header());
        assert_eq!(f.laps, vec![lap(1), lap(2)]);
        assert_eq!(f.pending().map(|l| l.lap_seq).collect::<Vec<_>>(), [2]);
        assert!(!f.truncated && f.closed_unix.is_none());
        // Appending is plain concatenation.
        let mut more = b.clone();
        more.extend_from_slice(&encode_closed(1_000_500));
        more.extend_from_slice(&encode_resolved(2));
        let f = PositionsFile::decode(&more).unwrap();
        assert_eq!(f.closed_unix, Some(1_000_500));
        assert!(f.is_finished());
    }

    #[test]
    fn decisions() {
        let open =
            PositionsFile::decode(&image(&[encode_lap(&lap(1)), encode_resolved(1)])).unwrap();
        // All laps resolved but the session is still open: more laps may come.
        assert_eq!(open.decide(1_000_100), Decision::Keep);
        let pending =
            PositionsFile::decode(&image(&[encode_lap(&lap(1)), encode_closed(1_000_500)]))
                .unwrap();
        assert_eq!(pending.decide(1_000_600), Decision::Keep);
        assert_eq!(pending.decide(1_000_500 + EXPIRY_SECS), Decision::Keep);
        assert_eq!(
            pending.decide(1_000_500 + EXPIRY_SECS + 1),
            Decision::Delete
        );
        let done = PositionsFile::decode(&image(&[
            encode_lap(&lap(1)),
            encode_closed(1_000_500),
            encode_resolved(1),
        ]))
        .unwrap();
        assert_eq!(done.decide(1_000_501), Decision::Delete);
        // A session never closed expires from its start.
        assert_eq!(open.decide(1_000_000 + EXPIRY_SECS + 1), Decision::Delete);
        // A closed session with no laps has nothing to wait for.
        let empty = PositionsFile::decode(&image(&[encode_closed(1_000_500)])).unwrap();
        assert_eq!(empty.decide(1_000_501), Decision::Delete);
    }

    #[test]
    fn a_cut_record_is_ignored_and_flagged() {
        let full = image(&[encode_lap(&lap(1)), encode_lap(&lap(2))]);
        for cut in 1..encode_lap(&lap(2)).len() {
            let f = PositionsFile::decode(&full[..full.len() - cut]).unwrap();
            assert_eq!(f.laps, vec![lap(1)], "cut {cut}");
            assert!(f.truncated);
        }
    }

    #[test]
    fn unknown_records_are_skipped_and_bad_input_is_refused() {
        let unknown = record(9, &[1, 2, 3]);
        let f = PositionsFile::decode(&image(&[unknown, encode_lap(&lap(1))])).unwrap();
        assert_eq!(f.laps.len(), 1);
        assert_eq!(
            PositionsFile::decode(b"nonsense, not a file at all"),
            Err(DecodeError::NotAPositionsFile)
        );
        let mut v2 = image(&[]);
        v2[8] = 2;
        assert_eq!(
            PositionsFile::decode(&v2),
            Err(DecodeError::UnsupportedVersion(2))
        );
        let h = image(&[]);
        for cut in 12..h.len() {
            assert_eq!(
                PositionsFile::decode(&h[..cut]),
                Err(DecodeError::BadHeader),
                "cut {cut}"
            );
        }
        // A lap record whose count does not fit its payload.
        let mut bad = encode_lap(&lap(1));
        let n_at = 5 + 8 + 1 + 8 + 8;
        bad[n_at..n_at + 4].copy_from_slice(&1_000_000u32.to_le_bytes());
        assert_eq!(
            PositionsFile::decode(&image(&[bad])),
            Err(DecodeError::BadRecord)
        );
    }
}
