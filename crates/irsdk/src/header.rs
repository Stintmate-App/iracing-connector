//! The fixed headers: the main header, and the sub-header of `.ibt` files.
//!
//! Every function works on `&[u8]` with bounds checks and never panics, whatever the input.

use crate::defs::{MAX_BUFS, VER};

/// Size of the main header in bytes.
pub const HEADER_LEN: usize = 112;
/// End of the `.ibt` sub-header (it starts at [`HEADER_LEN`]).
pub const DISK_HEADER_END: usize = 144;
/// Size of one variable header in bytes.
pub const VAR_HEADER_LEN: usize = 144;

/// The most bytes one row may have (4 MiB).
pub const MAX_BUF_LEN: i32 = 4 * 1024 * 1024;
/// The most variables a header may announce.
pub const MAX_VARS: i32 = 4096;

const _: () = assert!(HEADER_LEN == 48 + 16 * MAX_BUFS);
const _: () = assert!(DISK_HEADER_END == HEADER_LEN + 32);
const _: () = assert!(VAR_HEADER_LEN == 16 + 32 + 64 + 32);

/// Why a layout was refused. The field names the part of the layout at fault.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LayoutError {
    /// The input ends before the field does.
    TooShort {
        /// The field or structure being read.
        field: &'static str,
        /// Bytes needed.
        need: usize,
        /// Bytes available.
        have: usize,
    },
    /// The layout version is not the one this crate reads.
    UnsupportedVersion(i32),
    /// The value of the field is outside its allowed range.
    OutOfRange {
        /// The field.
        field: &'static str,
        /// The value found.
        value: i64,
    },
    /// The region the field points at ends past the input.
    PastEnd {
        /// The field.
        field: &'static str,
    },
}

impl LayoutError {
    /// The field at fault.
    pub fn field(&self) -> &'static str {
        match self {
            LayoutError::TooShort { field, .. }
            | LayoutError::OutOfRange { field, .. }
            | LayoutError::PastEnd { field } => field,
            LayoutError::UnsupportedVersion(_) => "ver",
        }
    }
}

impl std::fmt::Display for LayoutError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LayoutError::TooShort { field, need, have } => {
                write!(f, "{field}: need {need} bytes, have {have}")
            }
            LayoutError::UnsupportedVersion(v) => write!(f, "unsupported layout version {v}"),
            LayoutError::OutOfRange { field, value } => {
                write!(f, "{field}: value {value} out of range")
            }
            LayoutError::PastEnd { field } => write!(f, "{field}: region ends past the data"),
        }
    }
}

impl std::error::Error for LayoutError {}

/// Reads the little-endian `i32` at `off`; 0 when out of range (callers check lengths first).
pub(crate) fn i32_at(b: &[u8], off: usize) -> i32 {
    off.checked_add(4)
        .and_then(|end| b.get(off..end))
        .and_then(|s| s.try_into().ok())
        .map_or(0, i32::from_le_bytes)
}

fn i64_at(b: &[u8], off: usize) -> i64 {
    off.checked_add(8)
        .and_then(|end| b.get(off..end))
        .and_then(|s| s.try_into().ok())
        .map_or(0, i64::from_le_bytes)
}

fn f64_at(b: &[u8], off: usize) -> f64 {
    off.checked_add(8)
        .and_then(|end| b.get(off..end))
        .and_then(|s| s.try_into().ok())
        .map_or(0.0, f64::from_le_bytes)
}

/// Decodes a NUL-terminated cp1252 field: the bytes up to the first NUL. Bytes the code page
/// leaves undefined are dropped.
pub(crate) fn cp1252_string(bytes: &[u8]) -> String {
    const HIGH: [u16; 32] = [
        0x20AC, 0, 0x201A, 0x0192, 0x201E, 0x2026, 0x2020, 0x2021, 0x02C6, 0x2030, 0x0160, 0x2039,
        0x0152, 0, 0x017D, 0, 0, 0x2018, 0x2019, 0x201C, 0x201D, 0x2022, 0x2013, 0x2014, 0x02DC,
        0x2122, 0x0161, 0x203A, 0x0153, 0, 0x017E, 0x0178,
    ];
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    let mut out = String::with_capacity(end);
    for &b in &bytes[..end] {
        let cp = match b {
            0x80..=0x9F => u32::from(HIGH[usize::from(b - 0x80)]),
            _ => u32::from(b),
        };
        if let Some(c) = char::from_u32(cp).filter(|_| cp != 0) {
            out.push(c);
        }
    }
    out
}

/// One of the row buffers the header describes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct VarBuf {
    /// The tick count of the row in this buffer.
    pub tick_count: i32,
    /// Offset of the buffer from the start of the data.
    pub buf_offset: i32,
}

/// The main header (112 bytes at the start of the data).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Header {
    /// Layout version.
    pub ver: i32,
    /// Status bits; bit 0 is "connected".
    pub status: i32,
    /// Rows per second the simulator writes.
    pub tick_rate: i32,
    /// Counter that moves when the session YAML changes.
    pub session_info_update: i32,
    /// Length of the session YAML in bytes.
    pub session_info_len: i32,
    /// Offset of the session YAML.
    pub session_info_offset: i32,
    /// Number of variable headers.
    pub num_vars: i32,
    /// Offset of the first variable header.
    pub var_header_offset: i32,
    /// Number of row buffers in use (1 to 4).
    pub num_buf: i32,
    /// Length of one row in bytes.
    pub buf_len: i32,
    /// The row buffers; only the first `num_buf` are meaningful.
    pub var_buf: [VarBuf; MAX_BUFS],
}

impl Header {
    /// Parses the header of live data held in `bytes`; the buffers are checked against
    /// `bytes.len()`.
    pub fn parse(bytes: &[u8]) -> Result<Header, LayoutError> {
        Header::parse_in(bytes, bytes.len(), true)
    }

    /// Parses a header of which `bytes` holds only the first part: `total_len` is the length of
    /// the whole data the offsets point into. With `check_bufs` false the row buffers are not
    /// checked against `total_len` (an `.ibt` file may hold no row yet).
    pub fn parse_in(
        bytes: &[u8],
        total_len: usize,
        check_bufs: bool,
    ) -> Result<Header, LayoutError> {
        if bytes.len() < HEADER_LEN {
            return Err(LayoutError::TooShort {
                field: "header",
                need: HEADER_LEN,
                have: bytes.len(),
            });
        }
        let ver = i32_at(bytes, 0);
        if ver != VER {
            return Err(LayoutError::UnsupportedVersion(ver));
        }
        let mut var_buf = [VarBuf::default(); MAX_BUFS];
        for (i, slot) in var_buf.iter_mut().enumerate() {
            slot.tick_count = i32_at(bytes, 48 + 16 * i);
            slot.buf_offset = i32_at(bytes, 52 + 16 * i);
        }
        let h = Header {
            ver,
            status: i32_at(bytes, 4),
            tick_rate: i32_at(bytes, 8),
            session_info_update: i32_at(bytes, 12),
            session_info_len: i32_at(bytes, 16),
            session_info_offset: i32_at(bytes, 20),
            num_vars: i32_at(bytes, 24),
            var_header_offset: i32_at(bytes, 28),
            num_buf: i32_at(bytes, 32),
            buf_len: i32_at(bytes, 36),
            var_buf,
        };
        h.validate(total_len, check_bufs)?;
        Ok(h)
    }

    fn validate(&self, total_len: usize, check_bufs: bool) -> Result<(), LayoutError> {
        let range = |field: &'static str, v: i32, lo: i32, hi: i32| {
            if (lo..=hi).contains(&v) {
                Ok(())
            } else {
                Err(LayoutError::OutOfRange {
                    field,
                    value: i64::from(v),
                })
            }
        };
        range("num_buf", self.num_buf, 1, MAX_BUFS as i32)?;
        range("buf_len", self.buf_len, 1, MAX_BUF_LEN)?;
        range("num_vars", self.num_vars, 0, MAX_VARS)?;
        range("var_header_offset", self.var_header_offset, 0, i32::MAX)?;
        range("session_info_offset", self.session_info_offset, 0, i32::MAX)?;
        range("session_info_len", self.session_info_len, 0, i32::MAX)?;

        let fits = |start: i32, len: usize| {
            usize::try_from(start)
                .ok()
                .and_then(|s| s.checked_add(len))
                .is_some_and(|end| end <= total_len)
        };
        let var_bytes = (self.num_vars as usize) * VAR_HEADER_LEN;
        if !fits(self.var_header_offset, var_bytes) {
            return Err(LayoutError::PastEnd {
                field: "var_header_offset",
            });
        }
        if !fits(self.session_info_offset, self.session_info_len as usize) {
            return Err(LayoutError::PastEnd {
                field: "session_info_offset",
            });
        }
        if check_bufs {
            for slot in &self.var_buf[..self.num_buf as usize] {
                range("buf_offset", slot.buf_offset, 0, i32::MAX)?;
                if !fits(slot.buf_offset, self.buf_len as usize) {
                    return Err(LayoutError::PastEnd {
                        field: "buf_offset",
                    });
                }
            }
        }
        Ok(())
    }

    /// Whether the simulator marks itself as connected.
    pub fn connected(&self) -> bool {
        self.status & crate::defs::STATUS_CONNECTED != 0
    }
}

/// The sub-header of `.ibt` files (32 bytes at offset 112).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DiskHeader {
    /// Start of the session, as a Unix time.
    pub session_start_date: i64,
    /// Session time at the start of the file, in seconds.
    pub session_start_time: f64,
    /// Session time at the end of the file, in seconds.
    pub session_end_time: f64,
    /// Laps in the file.
    pub session_lap_count: i32,
    /// Rows in the file.
    pub session_record_count: i32,
}

impl DiskHeader {
    /// Parses the sub-header from the start of a whole file image.
    pub fn parse(bytes: &[u8]) -> Result<DiskHeader, LayoutError> {
        if bytes.len() < DISK_HEADER_END {
            return Err(LayoutError::TooShort {
                field: "disk header",
                need: DISK_HEADER_END,
                have: bytes.len(),
            });
        }
        Ok(DiskHeader {
            session_start_date: i64_at(bytes, 112),
            session_start_time: f64_at(bytes, 120),
            session_end_time: f64_at(bytes, 128),
            session_lap_count: i32_at(bytes, 136),
            session_record_count: i32_at(bytes, 140),
        })
    }
}
