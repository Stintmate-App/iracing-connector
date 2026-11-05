//! The probe: waits for the simulator's map, then writes what it exposes into one folder.

use std::fs::{self, File, OpenOptions};
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use irsdk::defs::VarType;
use irsdk::header::{HEADER_LEN, VAR_HEADER_LEN};
use irsdk::live::{Live, OpenError};
use irsdk::sampler::Poll;
use irsdk::vars::VarTable;
use irsdk::view::View;

use crate::util::{absolute, cp1252, csv_field, iso, jstr, stamp, under_sim_documents};
use crate::win;

const OPEN_EVERY: Duration = Duration::from_secs(2);
const STATUS_EVERY: Duration = Duration::from_secs(10);
const WINDOWS_EVERY: Duration = Duration::from_secs(10);
const ROW_DUMP_EVERY: Duration = Duration::from_secs(30);
const DETACH_AFTER: Duration = Duration::from_secs(10);
const MAX_SESSION_FILES: u32 = 200;
const MAX_ROW_DUMPS: u32 = 120;
const MAX_SESSION_LEN: i32 = 4 * 1024 * 1024;
const UTF8_MARKER: &[u8] = b"---\nWeekendInfo:\n Encoding: UTF8";
/// Per-car arrays written for the player's index and the first entries.
const CAR_ARRAYS: [&str; 5] = [
    "CarIdxLapDistPct",
    "CarIdxTrackSurface",
    "CarIdxOnPitRoad",
    "CarIdxPosition",
    "CarIdxSessionFlags",
];
const FIRST_ENTRIES: usize = 8;

/// What to record and where.
#[derive(Clone, Debug)]
pub struct Config {
    /// The folder the run folder is created in.
    pub out: PathBuf,
    /// Stop after this many minutes.
    pub minutes: Option<f64>,
    /// Rows of `samples.csv` per second.
    pub csv_hz: f64,
    /// A map and event name to open instead of the simulator's (tests).
    pub names: Option<(String, String)>,
}

const README: &str = "\
iracing-probe output. Everything here was read from the simulator's shared memory and its
windows; nothing was written to the simulator or its folders.

header.json            Every header field and the layout facts (sizes, offsets).
                       header-N.json for a later layout.
vars.json, vars.csv    Every variable: name, type, count, count_as_time, unit, desc, offset.
                       vars-N.* is kept for each later layout; vars.* is the latest.
session/NNN.yaml.bin   The session text as it is in memory, trailing NULs trimmed, one file per
                       change of the update counter (at most 200).
session/NNN.cp1252.yaml  The same bytes decoded as Windows-1252 and saved as UTF-8.
session/NNN.meta.json  Update counter, time, length, UTF-8 marker, UTF-8 validity.
samples.csv            One row per --csv-hz: UTC time, tick, every scalar variable and the
                       per-car arrays (first 8 entries and the player's, as name[player]).
                       Booleans are 0/1, bit fields hex. samples-N.csv after a layout reset.
rows/<tick>.row.bin    Raw rows (<tick>-gN.row.bin after the first layout).
layout.bin             Bytes 0 to the end of the variable headers and the session text
                       (layout-N.bin for later layouts): enough to rebuild a synthetic map.
windows.jsonl          Every 10 s: the simulator's visible top-level windows.
stats.json             Counters written at exit.
";

/// Refuses an output folder under the simulator's own documents folder.
pub fn check_out(out: &Path) -> Result<(), String> {
    if under_sim_documents(out) {
        return Err(format!(
            "refusing to write under a Documents\\iRacing folder: {}",
            absolute(out).display()
        ));
    }
    Ok(())
}

/// How to read one `samples.csv` column from a row.
enum Source {
    Direct {
        off: usize,
        ty: VarType,
    },
    Player {
        off: usize,
        ty: VarType,
        count: usize,
        idx_off: usize,
    },
}

struct Csv {
    out: BufWriter<File>,
    cols: Vec<Source>,
}

#[derive(Default)]
struct Counters {
    attaches: u32,
    resets: u32,
    rows_seen: u64,
    csv_rows: u64,
    session_changes: u64,
    session_files: u32,
    session_errors: u32,
    row_dumps: u32,
    disconnected_polls: u64,
    write_errors: u64,
    window_samples: u64,
    first_row: Option<Instant>,
    last_row: Option<Instant>,
}

/// One run of the probe.
pub struct Probe {
    cfg: Config,
    dir: PathBuf,
    live: Option<Live>,
    started: Instant,
    layout_no: u32,
    gens: Vec<String>,
    last_update: Option<i32>,
    csv: Option<Csv>,
    dump_now: bool,
    disconnected_since: Option<Instant>,
    next_open: Instant,
    next_status: Instant,
    next_windows: Instant,
    next_csv: Instant,
    next_dump: Instant,
    n: Counters,
}

fn suffix(layout_no: u32) -> String {
    if layout_no <= 1 {
        String::new()
    } else {
        format!("-{layout_no}")
    }
}

fn fmt_value(ty: VarType, b: &[u8]) -> String {
    let word = || b.get(..4).and_then(|x| <[u8; 4]>::try_from(x).ok());
    match ty {
        VarType::Char => b.first().map(|v| v.to_string()),
        VarType::Bool => b.first().map(|v| u8::from(*v != 0).to_string()),
        VarType::Int => word().map(|w| i32::from_le_bytes(w).to_string()),
        VarType::BitField => word().map(|w| format!("0x{:08X}", u32::from_le_bytes(w))),
        VarType::Float => word().map(|w| f32::from_le_bytes(w).to_string()),
        VarType::Double => b
            .get(..8)
            .and_then(|x| <[u8; 8]>::try_from(x).ok())
            .map(|w| f64::from_le_bytes(w).to_string()),
    }
    .unwrap_or_default()
}

fn build_columns(vars: &VarTable) -> (Vec<String>, Vec<Source>) {
    let mut names = Vec::new();
    let mut cols = Vec::new();
    let player = vars
        .get("PlayerCarIdx")
        .filter(|v| v.count == 1 && v.ty == VarType::Int)
        .map(|v| v.offset);
    for v in vars.iter() {
        if v.count == 1 {
            names.push(v.name.clone());
            cols.push(Source::Direct {
                off: v.offset,
                ty: v.ty,
            });
        } else if CAR_ARRAYS.contains(&v.name.as_str()) {
            for i in 0..v.count.min(FIRST_ENTRIES) {
                names.push(format!("{}[{i}]", v.name));
                cols.push(Source::Direct {
                    off: v.offset + i * v.ty.size(),
                    ty: v.ty,
                });
            }
            if let Some(idx_off) = player {
                names.push(format!("{}[player]", v.name));
                cols.push(Source::Player {
                    off: v.offset,
                    ty: v.ty,
                    count: v.count,
                    idx_off,
                });
            }
        }
    }
    (names, cols)
}

impl Probe {
    /// Creates `<out>\probe-<UTC timestamp>\` and the README.
    pub fn new(cfg: Config) -> Result<Probe, String> {
        check_out(&cfg.out)?;
        let dir = absolute(&cfg.out).join(format!("probe-{}", stamp(SystemTime::now())));
        for sub in ["session", "rows"] {
            fs::create_dir_all(dir.join(sub)).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        fs::write(dir.join("README.txt"), README).map_err(|e| e.to_string())?;
        let now = Instant::now();
        Ok(Probe {
            cfg,
            dir,
            live: None,
            started: now,
            layout_no: 0,
            gens: Vec::new(),
            last_update: None,
            csv: None,
            dump_now: false,
            disconnected_since: None,
            next_open: now,
            next_status: now + STATUS_EVERY,
            next_windows: now,
            next_csv: now,
            next_dump: now,
            n: Counters::default(),
        })
    }

    /// The run folder.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Runs until `stop` is set or the time is up, then writes `stats.json`.
    pub fn run(&mut self, stop: &AtomicBool) {
        println!("writing to {}", self.dir().display());
        let deadline = self
            .cfg
            .minutes
            .map(|m| self.started + Duration::from_secs_f64(m * 60.0));
        while !stop.load(Ordering::SeqCst) && deadline.is_none_or(|d| Instant::now() < d) {
            self.step();
        }
        self.finish();
    }

    fn note(&mut self, r: io::Result<()>) {
        if let Err(e) = r {
            self.n.write_errors += 1;
            eprintln!("write failed: {e}");
        }
    }

    fn open(&self) -> Result<Live, OpenError> {
        match &self.cfg.names {
            Some((m, e)) => Live::open_named(m, e),
            None => Live::open(),
        }
    }

    /// One pass of the main loop: at most 50 ms of waiting.
    pub fn step(&mut self) {
        let now = Instant::now();
        if now >= self.next_windows {
            self.next_windows = now + WINDOWS_EVERY;
            let r = self.write_windows();
            self.note(r);
        }
        if now >= self.next_status {
            self.next_status = now + STATUS_EVERY;
            self.status();
        }
        let Some(mut live) = self.live.take() else {
            if now >= self.next_open {
                self.next_open = now + OPEN_EVERY;
                match self.open() {
                    Ok(live) => {
                        println!("attached to the simulator's map");
                        self.n.attaches += 1;
                        self.disconnected_since = None;
                        self.last_update = None;
                        self.new_generation(&live);
                        self.live = Some(live);
                    }
                    Err(OpenError::NotRunning) => {}
                    Err(e) => eprintln!("cannot attach: {e}"),
                }
            }
            thread::sleep(Duration::from_millis(50));
            return;
        };
        let poll = live.wait(Duration::from_millis(50));
        self.check_session(&live);
        match poll {
            Poll::Row => {
                self.disconnected_since = None;
                self.on_row(&live);
            }
            Poll::Reset => {
                self.disconnected_since = None;
                self.n.resets += 1;
                println!("layout reset");
                self.new_generation(&live);
            }
            Poll::Disconnected => {
                self.n.disconnected_polls += 1;
                let since = *self.disconnected_since.get_or_insert(now);
                if now.duration_since(since) > DETACH_AFTER {
                    println!("simulator disconnected; waiting for it");
                    self.disconnected_since = None;
                    self.csv = None;
                    return;
                }
            }
            Poll::NoNewRow | Poll::Torn => self.disconnected_since = None,
        }
        self.live = Some(live);
    }

    fn status(&self) {
        let t = self.started.elapsed().as_secs();
        if self.live.is_none() {
            println!("[{t} s] waiting for the simulator (map absent)");
        } else {
            println!(
                "[{t} s] attached: layout {}, {} rows, {} session texts, {} samples",
                self.layout_no, self.n.rows_seen, self.n.session_files, self.n.csv_rows
            );
        }
    }

    /// Starts a generation: the header, the variables and the layout bytes.
    fn new_generation(&mut self, live: &Live) {
        self.layout_no += 1;
        self.csv = None;
        self.dump_now = true;
        let r = self.write_layout_files(live);
        self.note(r);
    }

    fn write_layout_files(&mut self, live: &Live) -> io::Result<()> {
        let s = live.sampler();
        let h = *s.header();
        let vars = s.vars();
        let view = s.view();
        let sfx = suffix(self.layout_no);
        let var_end =
            i64::from(h.var_header_offset) + i64::from(h.num_vars) * VAR_HEADER_LEN as i64;
        let sess_off = view.load_i32(20).unwrap_or(h.session_info_offset);
        let sess_len = view.load_i32(16).unwrap_or(h.session_info_len);
        let sess_end = i64::from(sess_off) + i64::from(sess_len);
        let used = vars
            .iter()
            .map(|v| v.offset + v.ty.size() * v.count)
            .max()
            .unwrap_or(0);
        let mut offs: Vec<i32> = h
            .var_buf
            .iter()
            .take(h.num_buf.max(0) as usize)
            .map(|b| b.buf_offset)
            .collect();
        offs.sort_unstable();
        let strides: Vec<String> = offs.windows(2).map(|w| (w[1] - w[0]).to_string()).collect();
        let bufs: Vec<String> = h
            .var_buf
            .iter()
            .map(|b| {
                format!(
                    "{{\"tick_count\":{},\"buf_offset\":{}}}",
                    b.tick_count, b.buf_offset
                )
            })
            .collect();
        let json = format!(
            "{{\n \"utc\":{},\n \"generation\":{},\n \"ver\":{},\n \"status\":{},\n \"tick_rate\":{},\n \
             \"session_info_update\":{},\n \"session_info_len\":{},\n \"session_info_offset\":{},\n \
             \"num_vars\":{},\n \"var_header_offset\":{},\n \"num_buf\":{},\n \"buf_len\":{},\n \
             \"var_buf\":[{}],\n \"layout\":{{\n  \"map_len\":{},\n  \"header_len\":{},\n  \
             \"var_headers_end\":{},\n  \"session_text_end\":{},\n  \"gap_var_headers_to_session_text\":{},\n  \
             \"first_buf_offset\":{},\n  \"buf_strides\":[{}],\n  \"row_bytes_used\":{},\n  \
             \"skipped_vars\":{}\n }}\n}}\n",
            jstr(&iso(SystemTime::now())),
            self.layout_no,
            h.ver,
            h.status,
            h.tick_rate,
            h.session_info_update,
            h.session_info_len,
            h.session_info_offset,
            h.num_vars,
            h.var_header_offset,
            h.num_buf,
            h.buf_len,
            bufs.join(","),
            view.len(),
            HEADER_LEN,
            var_end,
            sess_end,
            i64::from(sess_off) - var_end,
            offs.first().copied().unwrap_or(0),
            strides.join(","),
            used,
            vars.skipped().len(),
        );
        fs::write(self.dir.join(format!("header{sfx}.json")), &json)?;

        let mut vj = String::from("[\n");
        let mut vc = String::from("name,type,count,count_as_time,unit,desc,offset\n");
        let total = vars.len();
        for (i, v) in vars.iter().enumerate() {
            vj.push_str(&format!(
                " {{\"name\":{},\"type\":\"{:?}\",\"count\":{},\"count_as_time\":{},\"unit\":{},\"desc\":{},\"offset\":{}}}{}\n",
                jstr(&v.name),
                v.ty,
                v.count,
                v.count_as_time,
                jstr(&v.unit),
                jstr(&v.desc),
                v.offset,
                if i + 1 < total { "," } else { "" }
            ));
            vc.push_str(&format!(
                "{},{:?},{},{},{},{},{}\n",
                csv_field(&v.name),
                v.ty,
                v.count,
                v.count_as_time,
                csv_field(&v.unit),
                csv_field(&v.desc),
                v.offset
            ));
        }
        vj.push_str("]\n");
        fs::write(self.dir.join("vars.json"), &vj)?;
        fs::write(self.dir.join("vars.csv"), &vc)?;
        if self.layout_no > 1 {
            fs::write(self.dir.join(format!("vars{sfx}.json")), &vj)?;
            fs::write(self.dir.join(format!("vars{sfx}.csv")), &vc)?;
        }

        // The bytes from 0 to the end of the variable headers and the session text.
        let end = var_end
            .max(sess_end)
            .clamp(0, view.len() as i64)
            .min(16 << 20) as usize;
        let mut bytes = vec![0u8; end];
        if view.copy_out(0, &mut bytes) {
            fs::write(self.dir.join(format!("layout{sfx}.bin")), &bytes)?;
        }
        self.gens.push(format!(
            "{{\"generation\":{},\"utc\":{},\"num_vars\":{},\"buf_len\":{},\"tick_rate\":{}}}",
            self.layout_no,
            jstr(&iso(SystemTime::now())),
            h.num_vars,
            h.buf_len,
            h.tick_rate
        ));
        Ok(())
    }

    /// Saves the session text when its update counter moved.
    fn check_session(&mut self, live: &Live) {
        let view = live.sampler().view();
        let Some(upd) = view.load_i32(12) else { return };
        if self.last_update == Some(upd) {
            return;
        }
        let len = view.load_i32(16).unwrap_or(0);
        let off = view.load_i32(20).unwrap_or(0);
        let ok = (0..=MAX_SESSION_LEN).contains(&len)
            && off >= 0
            && (off as usize).saturating_add(len as usize) <= view.len();
        if !ok {
            self.last_update = Some(upd);
            self.n.session_errors += 1;
            return;
        }
        let mut bytes = vec![0u8; len as usize];
        if !view.copy_out(off as usize, &mut bytes) || view.load_i32(12) != Some(upd) {
            return; // changed under the copy: read it again on the next pass
        }
        self.last_update = Some(upd);
        self.n.session_changes += 1;
        self.dump_now = true;
        if self.n.session_files >= MAX_SESSION_FILES {
            return;
        }
        while bytes.last() == Some(&0) {
            bytes.pop();
        }
        self.n.session_files += 1;
        let tick = live.sampler().row().tick();
        let r = self.write_session(self.n.session_files, upd, &bytes, tick);
        self.note(r);
    }

    fn write_session(&self, idx: u32, upd: i32, bytes: &[u8], tick: i32) -> io::Result<()> {
        let base = self.dir.join("session").join(format!("{idx:03}"));
        let with = |ext: &str| PathBuf::from(format!("{}.{ext}", base.display()));
        fs::write(with("yaml.bin"), bytes)?;
        fs::write(with("cp1252.yaml"), cp1252(bytes))?;
        let meta = format!(
            "{{\"index\":{idx},\"update\":{upd},\"utc\":{},\"elapsed_ms\":{},\"last_tick\":{tick},\"len\":{},\
             \"starts_with_utf8_marker\":{},\"valid_utf8\":{}}}\n",
            jstr(&iso(SystemTime::now())),
            self.started.elapsed().as_millis(),
            bytes.len(),
            bytes.starts_with(UTF8_MARKER),
            std::str::from_utf8(bytes).is_ok()
        );
        fs::write(with("meta.json"), meta)
    }

    fn on_row(&mut self, live: &Live) {
        let now = Instant::now();
        self.n.rows_seen += 1;
        self.n.first_row.get_or_insert(now);
        self.n.last_row = Some(now);
        let row = live.sampler().row();
        if (self.dump_now || now >= self.next_dump) && self.n.row_dumps < MAX_ROW_DUMPS {
            self.dump_now = false;
            self.next_dump = now + ROW_DUMP_EVERY;
            self.n.row_dumps += 1;
            let name = if self.layout_no <= 1 {
                format!("{}.row.bin", row.tick())
            } else {
                format!("{}-g{}.row.bin", row.tick(), self.layout_no)
            };
            let r = fs::write(self.dir.join("rows").join(name), row.bytes());
            self.note(r);
        }
        if now >= self.next_csv {
            self.next_csv = now + Duration::from_secs_f64(1.0 / self.cfg.csv_hz);
            let r = self.write_sample(live);
            self.note(r);
        }
    }

    fn write_sample(&mut self, live: &Live) -> io::Result<()> {
        let s = live.sampler();
        if self.csv.is_none() {
            let (names, cols) = build_columns(s.vars());
            let path = self
                .dir
                .join(format!("samples{}.csv", suffix(self.layout_no)));
            let mut out = BufWriter::new(File::create(path)?);
            let head: Vec<String> = names.iter().map(|n| csv_field(n)).collect();
            writeln!(out, "utc,tick,{}", head.join(","))?;
            self.csv = Some(Csv { out, cols });
        }
        let row = s.row().bytes();
        let Some(csv) = self.csv.as_mut() else {
            return Ok(());
        };
        let mut line = format!("{},{}", iso(SystemTime::now()), s.row().tick());
        for c in &csv.cols {
            let text = match *c {
                Source::Direct { off, ty } => {
                    row.get(off..).map(|b| fmt_value(ty, b)).unwrap_or_default()
                }
                Source::Player {
                    off,
                    ty,
                    count,
                    idx_off,
                } => row
                    .get(idx_off..idx_off + 4)
                    .and_then(|b| <[u8; 4]>::try_from(b).ok())
                    .map(i32::from_le_bytes)
                    .and_then(|i| usize::try_from(i).ok())
                    .filter(|&i| i < count)
                    .and_then(|i| row.get(off + i * ty.size()..))
                    .map(|b| fmt_value(ty, b))
                    .unwrap_or_default(),
            };
            line.push(',');
            line.push_str(&text);
        }
        writeln!(csv.out, "{line}")?;
        csv.out.flush()?;
        self.n.csv_rows += 1;
        Ok(())
    }

    fn write_windows(&mut self) -> io::Result<()> {
        let list = win::sim_windows();
        self.n.window_samples += 1;
        let items: Vec<String> = list
            .iter()
            .map(|w| {
                format!(
                    "{{\"hwnd\":\"0x{:X}\",\"pid\":{},\"class\":{},\"title\":{},\"client_width\":{},\"client_height\":{},\"minimized\":{}}}",
                    w.hwnd,
                    w.pid,
                    jstr(&w.class),
                    jstr(&w.title),
                    w.width,
                    w.height,
                    w.minimized
                )
            })
            .collect();
        let mut f = OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.dir.join("windows.jsonl"))?;
        writeln!(
            f,
            "{{\"utc\":{},\"map_present\":{},\"windows\":[{}]}}",
            jstr(&iso(SystemTime::now())),
            self.live.is_some(),
            items.join(",")
        )
    }

    /// Writes `stats.json`.
    pub fn finish(&mut self) {
        if let Some(c) = self.csv.as_mut() {
            let _ = c.out.flush();
        }
        let sampler_json = self.live.as_ref().map_or("null".to_owned(), |l| {
            let s = l.sampler().stats();
            format!(
                "{{\"rows\":{},\"torn\":{},\"retries\":{},\"skipped_rate\":{},\"resets\":{}}}",
                s.rows, s.torn, s.retries, s.skipped_rate, s.resets
            )
        });
        let n = &self.n;
        let rate = match (n.first_row, n.last_row) {
            (Some(a), Some(b)) if b > a => {
                (n.rows_seen - 1) as f64 / b.duration_since(a).as_secs_f64()
            }
            _ => 0.0,
        };
        let json = format!(
            "{{\n \"ended_utc\":{},\n \"elapsed_s\":{:.1},\n \"attaches\":{},\n \"sampler\":{},\n \"rows_seen\":{},\n \
             \"measured_row_rate_hz\":{:.2},\n \"layout_resets\":{},\n \"generations\":[{}],\n \"session_changes\":{},\n \
             \"session_files\":{},\n \"session_errors\":{},\n \"samples_written\":{},\n \"row_dumps\":{},\n \
             \"disconnected_polls\":{},\n \"window_samples\":{},\n \"write_errors\":{}\n}}\n",
            jstr(&iso(SystemTime::now())),
            self.started.elapsed().as_secs_f64(),
            n.attaches,
            sampler_json,
            n.rows_seen,
            rate,
            n.resets,
            self.gens.join(","),
            n.session_changes,
            n.session_files,
            n.session_errors,
            n.csv_rows,
            n.row_dumps,
            n.disconnected_polls,
            n.window_samples,
            n.write_errors
        );
        if let Err(e) = fs::write(self.dir.join("stats.json"), json) {
            eprintln!("stats.json: {e}");
        }
        println!(
            "done: {} rows, {} session texts, in {}",
            n.rows_seen,
            n.session_files,
            self.dir.display()
        );
    }
}
