//! Small helpers: UTC time text, JSON text, the cp1252 decoder, the output path check.

use std::path::{Component, Path, PathBuf};
use std::time::SystemTime;

/// Year, month, day, hour, minute, second and millisecond of `t` in UTC.
fn civil(t: SystemTime) -> (i64, u32, u32, u32, u32, u32, u32) {
    let d = t.duration_since(SystemTime::UNIX_EPOCH).unwrap_or_default();
    let secs = d.as_secs() as i64;
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400) as u32;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = yoe + era * 400 + i64::from(month <= 2);
    (
        year,
        month,
        day,
        rem / 3600,
        rem % 3600 / 60,
        rem % 60,
        d.subsec_millis(),
    )
}

/// `2026-11-05T12:34:56.789Z`.
pub fn iso(t: SystemTime) -> String {
    let (y, mo, d, h, mi, s, ms) = civil(t);
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:{s:02}.{ms:03}Z")
}

/// `20261105T123456Z`, safe in a folder name.
pub fn stamp(t: SystemTime) -> String {
    let (y, mo, d, h, mi, s, _) = civil(t);
    format!("{y:04}{mo:02}{d:02}T{h:02}{mi:02}{s:02}Z")
}

/// A JSON string literal for `s`.
pub fn jstr(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// A CSV field for `s`, quoted when it needs to be.
pub fn csv_field(s: &str) -> String {
    if s.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_owned()
    }
}

/// Decodes Windows-1252 text. Bytes the code page leaves undefined become U+FFFD.
pub fn cp1252(bytes: &[u8]) -> String {
    const HIGH: [u16; 32] = [
        0x20AC, 0, 0x201A, 0x0192, 0x201E, 0x2026, 0x2020, 0x2021, 0x02C6, 0x2030, 0x0160, 0x2039,
        0x0152, 0, 0x017D, 0, 0, 0x2018, 0x2019, 0x201C, 0x201D, 0x2022, 0x2013, 0x2014, 0x02DC,
        0x2122, 0x0161, 0x203A, 0x0153, 0, 0x017E, 0x0178,
    ];
    bytes
        .iter()
        .map(|&b| match b {
            0x80..=0x9F => match HIGH[usize::from(b - 0x80)] {
                0 => '\u{FFFD}',
                cp => char::from_u32(u32::from(cp)).unwrap_or('\u{FFFD}'),
            },
            _ => char::from(b),
        })
        .collect()
}

/// Makes `p` absolute without touching the file system.
pub fn absolute(p: &Path) -> PathBuf {
    std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf())
}

/// Whether `p` lies under a `Documents\iRacing` folder (the simulator's own data).
pub fn under_sim_documents(p: &Path) -> bool {
    let names: Vec<String> = absolute(p)
        .components()
        .filter_map(|c| match c {
            Component::Normal(n) => Some(n.to_string_lossy().to_ascii_lowercase()),
            _ => None,
        })
        .collect();
    names
        .windows(2)
        .any(|w| w[0] == "documents" && w[1] == "iracing")
}
