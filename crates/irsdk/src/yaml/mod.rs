//! The session text: decoding, quoting of free-text names, the split into top-level sections and
//! the parse into the partial model of [`crate::session`].
//!
//! The text is written in Windows-1252 unless the simulator is told to write UTF-8 (the text then
//! starts with an `Encoding: UTF8` line). User names are written without quotes, so a name with a
//! colon, a quote or a line break can make a section invalid: every top-level section is parsed on
//! its own, and [`fallback::extract`] reads the key facts line by line.

use std::borrow::Cow;
use std::collections::HashMap;

use serde::de::DeserializeOwned;

use crate::session::{
    CarSetup, DriverInfo, QualifyResultsInfo, SessionInfo, SessionsInfo, SplitTimeInfo, WeekendInfo,
};

pub mod fallback;
pub mod units;

/// The result of [`decode`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decoded {
    /// The text, with `\n` line ends and no control characters other than `\t`.
    pub text: String,
    /// True when the bytes were read as UTF-8 (the text carries the UTF-8 marker).
    pub utf8: bool,
    /// How many invalid UTF-8 sequences were replaced (always 0 for Windows-1252).
    pub replaced: usize,
}

const UTF8_MARKER: &[u8] = b"---\nWeekendInfo:\n Encoding: UTF8";
const UTF8_MARKER_CRLF: &[u8] = b"---\r\nWeekendInfo:\r\n Encoding: UTF8";

/// The code points of the bytes `0x80..=0x9F` in Windows-1252 (0 = undefined).
const HIGH: [u16; 32] = [
    0x20AC, 0, 0x201A, 0x0192, 0x201E, 0x2026, 0x2020, 0x2021, 0x02C6, 0x2030, 0x0160, 0x2039,
    0x0152, 0, 0x017D, 0, 0, 0x2018, 0x2019, 0x201C, 0x201D, 0x2022, 0x2013, 0x2014, 0x02DC,
    0x2122, 0x0161, 0x203A, 0x0153, 0, 0x017E, 0x0178,
];

/// Decodes the raw session text of the map or of a file.
///
/// The text ends at the first NUL (the map pads with NULs). It is read as UTF-8 only when it
/// starts with the UTF-8 marker; otherwise as Windows-1252, dropping the undefined bytes.
/// Control characters other than `\n` and `\t` are dropped (so CRLF becomes LF).
pub fn decode(raw: &[u8]) -> Decoded {
    let raw = &raw[..raw.iter().position(|&b| b == 0).unwrap_or(raw.len())];
    let utf8 = raw.starts_with(UTF8_MARKER) || raw.starts_with(UTF8_MARKER_CRLF);
    let mut replaced = 0;
    let mut text = String::with_capacity(raw.len());
    let keep = |c: char| c == '\n' || c == '\t' || !c.is_control();
    if utf8 {
        for chunk in raw.utf8_chunks() {
            text.extend(chunk.valid().chars().filter(|&c| keep(c)));
            if !chunk.invalid().is_empty() {
                replaced += 1;
                text.push('\u{FFFD}');
            }
        }
    } else {
        for &b in raw {
            let c = match b {
                0x80..=0x9F => match HIGH[usize::from(b - 0x80)] {
                    0 => continue,
                    cp => char::from_u32(u32::from(cp)).unwrap_or('\u{FFFD}'),
                },
                _ => char::from(b),
            };
            if keep(c) {
                text.push(c);
            }
        }
    }
    Decoded {
        text,
        utf8,
        replaced,
    }
}

/// The keys whose values are free text typed by a user.
const NAME_KEYS: [&str; 5] = [
    "UserName",
    "TeamName",
    "AbbrevName",
    "Initials",
    "DriverSetupName",
];

/// Puts double quotes around the values of the name keys, escaping `\` and `"` inside.
///
/// An empty value becomes `""`; a value that already starts and ends with `"` is left as it is.
/// Other lines are untouched. Returns the input itself when nothing changes.
pub fn requote(text: &str) -> Cow<'_, str> {
    let mut out = String::with_capacity(text.len() + 64);
    let mut changed = false;
    for line in text.split_inclusive('\n') {
        let (body, nl) = match line.strip_suffix('\n') {
            Some(b) => (b, "\n"),
            None => (line, ""),
        };
        match requote_line(body) {
            Some(new) => {
                changed = true;
                out.push_str(&new);
                out.push_str(nl);
            }
            None => out.push_str(line),
        }
    }
    if changed {
        Cow::Owned(out)
    } else {
        Cow::Borrowed(text)
    }
}

/// The requoted line, or `None` when it is not a name line or needs no change.
fn requote_line(line: &str) -> Option<String> {
    let rest = line.trim_start_matches(' ');
    let indent = &line[..line.len() - rest.len()];
    let (dash, rest) = match rest.strip_prefix("- ") {
        Some(r) => ("- ", r.trim_start_matches(' ')),
        None => ("", rest),
    };
    let key = NAME_KEYS.iter().find(|k| rest.starts_with(**k))?;
    let after = &rest[key.len()..];
    let value = if let Some(v) = after.strip_prefix(": ") {
        v.trim()
    } else if after.trim_end() == ":" {
        ""
    } else {
        return None;
    };
    if value.len() >= 2 && value.starts_with('"') && value.ends_with('"') {
        return None;
    }
    let mut new = String::with_capacity(line.len() + 8);
    new.push_str(indent);
    new.push_str(dash);
    new.push_str(key);
    new.push_str(": \"");
    for c in value.chars() {
        if c == '\\' || c == '"' {
            new.push('\\');
        }
        new.push(c);
    }
    new.push('"');
    Some(new)
}

/// Splits the text into its top-level sections: `(name, text)`, the text including the header
/// line. A line with no indentation that ends in `:` starts a section; `---` and `...` lines are
/// ignored. Text before the first section is dropped.
pub fn sections(text: &str) -> Vec<(&str, &str)> {
    let mut starts: Vec<(usize, &str)> = Vec::new();
    let mut pos = 0;
    for line in text.split_inclusive('\n') {
        if !line.starts_with([' ', '\t', '-', '.', '#']) {
            let body = line.trim_end_matches(['\n', ' ']);
            if let Some(name) = body.strip_suffix(':')
                && !name.is_empty()
                && !name.contains([' ', ':'])
            {
                starts.push((pos, name));
            }
        }
        pos += line.len();
    }
    (0..starts.len())
        .map(|i| {
            let end = starts.get(i + 1).map_or(text.len(), |s| s.0);
            (starts[i].1, &text[starts[i].0..end])
        })
        .collect()
}

/// FNV-1a 64 of a text.
pub fn fnv1a64(s: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in s.as_bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

/// A hash of each section's text (0 = the section is absent), so that a consumer knows which part
/// of the session text changed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SectionHashes {
    /// `WeekendInfo`.
    pub weekend: u64,
    /// `SessionInfo`.
    pub session: u64,
    /// `QualifyResultsInfo`.
    pub qualify: u64,
    /// `SplitTimeInfo`.
    pub split: u64,
    /// `DriverInfo`.
    pub driver: u64,
    /// `CarSetup`.
    pub car_setup: u64,
    /// Every other section together.
    pub other: u64,
}

impl SectionHashes {
    /// Hashes the sections of a text.
    pub fn of(text: &str) -> SectionHashes {
        let mut h = SectionHashes::default();
        let mut other = String::new();
        for (name, body) in sections(text) {
            let v = fnv1a64(body).max(1);
            match name {
                "WeekendInfo" => h.weekend = v,
                "SessionInfo" => h.session = v,
                "QualifyResultsInfo" => h.qualify = v,
                "SplitTimeInfo" => h.split = v,
                "DriverInfo" => h.driver = v,
                "CarSetup" => h.car_setup = v,
                _ => other.push_str(body),
            }
        }
        if !other.is_empty() {
            h.other = fnv1a64(&other).max(1);
        }
        h
    }
}

/// The result of [`parse`].
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Parsed {
    /// The sections that parsed.
    pub info: SessionInfo,
    /// `(section, message)` for each section that did not parse.
    pub errors: Vec<(String, String)>,
}

/// Quotes the names, splits the text and parses each top-level section on its own, so a broken
/// section loses only itself.
pub fn parse(text: &str) -> Parsed {
    let text = requote(text);
    let mut p = Parsed::default();
    for (name, body) in sections(&text) {
        match name {
            "WeekendInfo" => p.info.weekend = take::<WeekendInfo>(name, body, &mut p.errors),
            "SessionInfo" => p.info.session = take::<SessionsInfo>(name, body, &mut p.errors),
            "QualifyResultsInfo" => {
                p.info.qualify = take::<QualifyResultsInfo>(name, body, &mut p.errors);
            }
            "SplitTimeInfo" => p.info.split = take::<SplitTimeInfo>(name, body, &mut p.errors),
            "DriverInfo" => p.info.driver = take::<DriverInfo>(name, body, &mut p.errors),
            "CarSetup" => p.info.car_setup = take::<CarSetup>(name, body, &mut p.errors),
            _ => {}
        }
    }
    p
}

/// Deserializes one section (`Name:` and its body) into `T`; an error goes to `errors`.
fn take<T: DeserializeOwned>(
    name: &str,
    body: &str,
    errors: &mut Vec<(String, String)>,
) -> Option<T> {
    match serde_saphyr::from_str_with_options::<HashMap<String, Option<T>>>(body, options()) {
        Ok(mut m) => m.remove(name).flatten(),
        Err(e) => {
            errors.push((name.to_owned(), e.to_string()));
            None
        }
    }
}

/// The parser options: the defaults (duplicate keys are an error, plain scalars keep their text in
/// `String` fields), with the budget sized for a session string of a few hundred KB (64 drivers).
fn options() -> serde_saphyr::Options {
    let mut budget = serde_saphyr::Budget::default();
    budget.max_nodes = 1_000_000;
    budget.max_events = 4_000_000;
    budget.max_depth = 64;
    let mut options = serde_saphyr::Options::default();
    options.budget = Some(budget);
    options
}
