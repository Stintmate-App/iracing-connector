//! An `.ibt` file read through the same parsers as the live map.
//!
//! The file is read whole and kept in memory. Its rows start at the first buffer offset of the
//! header and are `buf_len` bytes each. A file holds one session: the session text is written
//! once.

use std::io;
use std::path::Path;

use crate::header::{DISK_HEADER_END, DiskHeader, Header, LayoutError};
use crate::vars::VarTable;
use crate::yaml::{self, Decoded, Parsed};

/// Why an `.ibt` file could not be read.
#[derive(Debug)]
pub enum IbtError {
    /// The file could not be read.
    Io(io::Error),
    /// A header of the file is not valid.
    Layout(LayoutError),
    /// The file holds no complete row.
    NoRows,
}

impl std::fmt::Display for IbtError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            IbtError::Io(e) => write!(f, "cannot read the file: {e}"),
            IbtError::Layout(e) => write!(f, "invalid file layout: {e}"),
            IbtError::NoRows => f.write_str("the file holds no complete row"),
        }
    }
}

impl std::error::Error for IbtError {}

impl From<io::Error> for IbtError {
    fn from(e: io::Error) -> Self {
        IbtError::Io(e)
    }
}

impl From<LayoutError> for IbtError {
    fn from(e: LayoutError) -> Self {
        IbtError::Layout(e)
    }
}

/// An `.ibt` file held in memory.
#[derive(Debug)]
pub struct Ibt {
    bytes: Vec<u8>,
    header: Header,
    disk: DiskHeader,
    vars: VarTable,
    rows_at: usize,
    buf_len: usize,
    rows: usize,
}

impl Ibt {
    /// Reads the whole file at `path`, read-only.
    pub fn open(path: &Path) -> Result<Ibt, IbtError> {
        Ibt::from_bytes(std::fs::read(path)?)
    }

    /// Reads a file image.
    ///
    /// The row count is the record count of the disk header, cut to the rows that fit in the
    /// file; a record count of zero (a file that was not closed) uses what fits.
    pub fn from_bytes(bytes: Vec<u8>) -> Result<Ibt, IbtError> {
        let header = Header::parse_in(&bytes, bytes.len(), false)?;
        let disk = DiskHeader::parse(&bytes)?;
        let vars = VarTable::parse(&bytes, &header)?;
        let rows_at = usize::try_from(header.var_buf[0].buf_offset)
            .ok()
            .filter(|&at| at >= DISK_HEADER_END && at <= bytes.len())
            .ok_or(LayoutError::PastEnd {
                field: "var_buf[0].buf_offset",
            })?;
        let buf_len = usize::try_from(header.buf_len).unwrap_or(0).max(1);
        let fit = (bytes.len() - rows_at) / buf_len;
        let rows = match usize::try_from(disk.session_record_count) {
            Ok(n) if n > 0 => n.min(fit),
            _ => fit,
        };
        if rows == 0 {
            return Err(IbtError::NoRows);
        }
        Ok(Ibt {
            bytes,
            header,
            disk,
            vars,
            rows_at,
            buf_len,
            rows,
        })
    }

    /// The main header.
    pub fn header(&self) -> &Header {
        &self.header
    }

    /// The disk sub-header.
    pub fn disk(&self) -> &DiskHeader {
        &self.disk
    }

    /// The variables of the file.
    pub fn vars(&self) -> &VarTable {
        &self.vars
    }

    /// The raw session text, as stored (not cut at the first NUL).
    pub fn yaml_raw(&self) -> &[u8] {
        let at = usize::try_from(self.header.session_info_offset).unwrap_or(0);
        let len = usize::try_from(self.header.session_info_len).unwrap_or(0);
        self.bytes.get(at..at.saturating_add(len)).unwrap_or(&[])
    }

    /// The session text, decoded.
    pub fn session_text(&self) -> Decoded {
        yaml::decode(self.yaml_raw())
    }

    /// The session text, decoded and parsed now.
    pub fn session(&self) -> Parsed {
        yaml::parse(&self.session_text().text)
    }

    /// The number of rows.
    pub fn len(&self) -> usize {
        self.rows
    }

    /// Whether the file has no rows (never true for a file that opened).
    pub fn is_empty(&self) -> bool {
        self.rows == 0
    }

    /// The length of one row in bytes.
    pub fn row_len(&self) -> usize {
        self.buf_len
    }

    /// Row `i`.
    pub fn row(&self, i: usize) -> Option<&[u8]> {
        if i >= self.rows {
            return None;
        }
        let at = self.rows_at + i * self.buf_len;
        self.bytes.get(at..at + self.buf_len)
    }

    /// The rows in order.
    pub fn rows(&self) -> impl Iterator<Item = &[u8]> {
        self.bytes[self.rows_at..]
            .chunks_exact(self.buf_len)
            .take(self.rows)
    }
}
