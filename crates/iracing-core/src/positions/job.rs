//! The job: match the recorded samples of a session's laps to the rows of its disk telemetry
//! files and project them.
//!
//! A sample is found in a file by `(SessionNum, SessionTick)`: the live map and the file hold
//! the same rows, so no interpolation or clock alignment is needed. `LapDistPct` guards the
//! match. A lap gets exact positions whole or not at all: every sample must be a row of **one**
//! file, in the world, with the same `LapDistPct`; otherwise the lap is left as it is and the
//! reason goes to the report.
//!
//! The job reads the files in chunks and asks `stop` before each one. A stopped job sends
//! nothing: laps are sent only after every file was read.

use std::io::Read;

use irsdk::scan::{Field, Scan};

use super::file::{LapRecord, PositionsFile};
use super::project::{Fix, interpolate, project};

/// The most `LapDistPct` may differ between a recorded sample and its file row.
pub const PCT_TOLERANCE: f32 = 1e-6;

/// The longest gap between the two rows around a line crossing that is still interpolated, in
/// seconds.
pub const MAX_BRACKET_S: f64 = 0.25;

/// Why a lap keeps the position it was sent with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Reason {
    /// Some samples are not in any file (the lap began before the file, or no file covers it).
    NotCovered {
        /// How many samples were not found.
        missing: usize,
    },
    /// The samples are in more than one file.
    SplitAcrossFiles,
    /// A row has latitude and longitude both 0: the car was out of the world.
    OutOfWorld,
    /// A row's `LapDistPct` differs from the recorded one: another file or session.
    Mismatch,
    /// No line crossing could be placed between two rows of a file.
    NoOrigin,
}

impl std::fmt::Display for Reason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Reason::NotCovered { missing } => {
                write!(f, "not covered by a file ({missing} samples)")
            }
            Reason::SplitAcrossFiles => f.write_str("samples in more than one file"),
            Reason::OutOfWorld => f.write_str("the car was out of the world"),
            Reason::Mismatch => f.write_str("lap distance differs from the recording"),
            Reason::NoOrigin => f.write_str("no line crossing found in the files"),
        }
    }
}

/// What became of a lap.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Its positions were given to the sink.
    Sent,
    /// It keeps its position.
    Left(Reason),
}

/// The exact positions of one lap, in metres, one value per sample.
#[derive(Clone, Debug, PartialEq)]
pub struct LapPositions {
    /// The lap's `lap_seq`.
    pub lap_seq: u64,
    /// East.
    pub x: Vec<f32>,
    /// North.
    pub z: Vec<f32>,
    /// Up.
    pub y: Vec<f32>,
}

/// What a run did.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Report {
    /// One entry per pending lap, in the file's order. Empty when `stopped`.
    pub outcomes: Vec<(u64, Outcome)>,
    /// Whether `stop` ended the run before every file was read: nothing was sent.
    pub stopped: bool,
    /// How many files could not be used (a variable missing, a read error).
    pub unusable_files: usize,
}

struct Slot {
    t: f64,
    fix: Option<Fix>,
}

struct LapState<'a> {
    rec: &'a LapRecord,
    samples: Vec<Option<Fix>>,
    found: usize,
    file: Option<usize>,
    split: bool,
    mismatch: bool,
    out_of_world: bool,
    start_slot: Option<usize>,
    end_slot: Option<usize>,
}

struct Fields {
    session_num: Field<i32>,
    tick: Field<i32>,
    time: Field<f64>,
    pct: Field<f32>,
    lat: Field<f64>,
    lon: Field<f64>,
    alt: Field<f32>,
}

impl Fields {
    fn of<R: Read>(scan: &Scan<R>) -> Option<Fields> {
        Some(Fields {
            session_num: scan.field("SessionNum").ok()?,
            tick: scan.field("SessionTick").ok()?,
            time: scan.field("SessionTime").ok()?,
            pct: scan.field("LapDistPct").ok()?,
            lat: scan.field("Lat").ok()?,
            lon: scan.field("Lon").ok()?,
            alt: scan.field("Alt").ok()?,
        })
    }
}

/// Runs the job for the session of `file` over `scans`, the disk files already chosen for it
/// (see [`super::select`]).
///
/// Each lap that is fully matched goes to `sink` as [`LapPositions`], once. `stop` is asked
/// before every chunk is read; when it says `true` the run ends at once, sends nothing and
/// reports `stopped`.
pub fn run<R: Read>(
    file: &PositionsFile,
    scans: &mut [Scan<R>],
    stop: &dyn Fn() -> bool,
    sink: &mut dyn FnMut(LapPositions),
) -> Report {
    let mut laps: Vec<LapState<'_>> = file
        .pending()
        .map(|rec| LapState {
            rec,
            samples: vec![None; rec.ticks.len().min(rec.pct.len())],
            found: 0,
            file: None,
            split: false,
            mismatch: false,
            out_of_world: false,
            start_slot: None,
            end_slot: None,
        })
        .collect();
    let mut report = Report::default();
    if laps.is_empty() {
        return report;
    }

    // Every crossing of the session, pending laps or not: a lap with none of its own takes the
    // origin of another.
    let mut slots: Vec<Slot> = Vec::new();
    let mut add = |t: Option<f64>| {
        t.filter(|t| t.is_finite()).map(|t| {
            slots.push(Slot { t, fix: None });
            slots.len() - 1
        })
    };
    for rec in &file.laps {
        let (s, e) = (add(rec.start_crossing), add(rec.end_crossing));
        if let Some(l) = laps.iter_mut().find(|l| std::ptr::eq(l.rec, rec)) {
            l.start_slot = s;
            l.end_slot = e;
        }
    }
    let mut order: Vec<usize> = (0..slots.len()).collect();
    order.sort_by(|&a, &b| slots[a].t.total_cmp(&slots[b].t));

    // The recorded samples by tick: (tick, lap, sample).
    let mut index: Vec<(i32, u32, u32)> = laps
        .iter()
        .enumerate()
        .flat_map(|(li, l)| {
            l.samples
                .iter()
                .enumerate()
                .map(move |(si, _)| (l.rec.ticks[si], li as u32, si as u32))
        })
        .collect();
    index.sort_unstable_by_key(|e| e.0);

    for (fi, scan) in scans.iter_mut().enumerate() {
        let Some(fields) = Fields::of(scan) else {
            report.unusable_files += 1;
            continue;
        };
        let mut prev: Option<(f64, Fix)> = None;
        let mut next_slot = 0usize;
        loop {
            if stop() {
                report.stopped = true;
                return report;
            }
            let chunk = match scan.next_chunk() {
                Ok(Some(c)) => c,
                Ok(None) => break,
                Err(_) => {
                    report.unusable_files += 1;
                    break;
                }
            };
            for row in chunk.rows() {
                if fields.session_num.get(row) != file.header.session_num {
                    prev = None;
                    continue;
                }
                let time = fields.time.get(row);
                let fix = Fix {
                    lat: fields.lat.get(row),
                    lon: fields.lon.get(row),
                    alt: f64::from(fields.alt.get(row)),
                };
                // The crossings this row is the first at or after: with the row before, they
                // give the origin.
                while let Some(&si) = order.get(next_slot) {
                    if slots[si].t > time {
                        break;
                    }
                    next_slot += 1;
                    if slots[si].fix.is_some() {
                        continue;
                    }
                    if let Some((pt, pf)) = prev
                        && pf.in_world()
                        && fix.in_world()
                        && time - pt <= MAX_BRACKET_S
                    {
                        slots[si].fix = interpolate(pf, pt, fix, time, slots[si].t);
                    }
                }
                prev = Some((time, fix));

                let tick = fields.tick.get(row);
                let from = index.partition_point(|e| e.0 < tick);
                for &(t, li, si) in &index[from..] {
                    if t != tick {
                        break;
                    }
                    let lap = &mut laps[li as usize];
                    let si = si as usize;
                    if (fields.pct.get(row) - lap.rec.pct[si]).abs() > PCT_TOLERANCE {
                        lap.mismatch = true;
                        continue;
                    }
                    if !fix.in_world() {
                        lap.out_of_world = true;
                        continue;
                    }
                    match lap.file {
                        None => lap.file = Some(fi),
                        Some(f) if f != fi => {
                            lap.split = true;
                            continue;
                        }
                        Some(_) => {}
                    }
                    if lap.samples[si].replace(fix).is_none() {
                        lap.found += 1;
                    }
                }
            }
        }
    }

    // The origin of a lap without a crossing of its own: the first crossing placed.
    let any_origin = order.iter().find_map(|&i| slots[i].fix);
    for lap in &laps {
        let n = lap.samples.len();
        let origin = [lap.start_slot, lap.end_slot]
            .into_iter()
            .flatten()
            .find_map(|i| slots[i].fix)
            .or(any_origin);
        let outcome = if lap.mismatch {
            Outcome::Left(Reason::Mismatch)
        } else if lap.out_of_world {
            Outcome::Left(Reason::OutOfWorld)
        } else if lap.split {
            Outcome::Left(Reason::SplitAcrossFiles)
        } else if n == 0 || lap.found < n {
            Outcome::Left(Reason::NotCovered {
                missing: n - lap.found,
            })
        } else if let Some(o) = origin {
            let mut out = LapPositions {
                lap_seq: lap.rec.lap_seq,
                x: Vec::with_capacity(n),
                z: Vec::with_capacity(n),
                y: Vec::with_capacity(n),
            };
            for fix in lap.samples.iter().flatten() {
                let (x, z, y) = project(&o, fix);
                out.x.push(x);
                out.z.push(z);
                out.y.push(y);
            }
            sink(out);
            Outcome::Sent
        } else {
            Outcome::Left(Reason::NoOrigin)
        };
        report.outcomes.push((lap.rec.lap_seq, outcome));
    }
    report
}
