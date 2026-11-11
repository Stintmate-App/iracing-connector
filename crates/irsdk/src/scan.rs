//! A streaming reader of `.ibt` files.
//!
//! [`crate::ibt::Ibt`] reads a whole file into memory. [`Scan`] reads the headers, the variable
//! table and the session text once, then hands the rows out in chunks of about one mebibyte, so
//! a job that needs a few variables of a file that grows by hundreds of megabytes an hour keeps
//! only one chunk in memory. The file is only ever read, sequentially.

use std::fs::File;
use std::io::{self, Read};
use std::marker::PhantomData;
use std::path::Path;

use crate::header::{DISK_HEADER_END, DiskHeader, Header, LayoutError};
use crate::ibt::IbtError;
use crate::vars::{VarError, VarTable, VarValue};
use crate::yaml::{self, Decoded};

/// The target size of one chunk of rows, in bytes.
pub const CHUNK_BYTES: usize = 1024 * 1024;

/// The most bytes the headers, variable table and session text of a file may take before its
/// first row.
const MAX_PREFIX: u64 = 64 * 1024 * 1024;

/// A variable resolved for reading from the raw bytes of a row.
#[derive(Clone, Copy, Debug)]
pub struct Field<T> {
    offset: usize,
    _t: PhantomData<T>,
}

impl<T: VarValue> Field<T> {
    /// The value in `row`, as [`Scan::next_chunk`] hands the rows out; zero when the row is
    /// too short.
    pub fn get(&self, row: &[u8]) -> T {
        self.offset
            .checked_add(T::SIZE)
            .and_then(|end| row.get(self.offset..end))
            .map_or(T::ZERO, T::from_le)
    }
}

/// A chunk of consecutive rows.
#[derive(Debug)]
pub struct Chunk<'a> {
    first: u64,
    row_len: usize,
    data: &'a [u8],
}

impl<'a> Chunk<'a> {
    /// The index of the chunk's first row in the file.
    pub fn first_row(&self) -> u64 {
        self.first
    }

    /// The number of rows in the chunk.
    pub fn len(&self) -> usize {
        self.data.len() / self.row_len
    }

    /// Whether the chunk has no rows (never true for a chunk [`Scan::next_chunk`] returns).
    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    /// The rows in order, each `row_len` bytes.
    pub fn rows(&self) -> impl Iterator<Item = &'a [u8]> {
        self.data.chunks_exact(self.row_len)
    }
}

/// A streaming reader of one `.ibt` file.
#[derive(Debug)]
pub struct Scan<R> {
    reader: R,
    header: Header,
    disk: DiskHeader,
    vars: VarTable,
    yaml: Vec<u8>,
    file_len: u64,
    buf_len: usize,
    rows: u64,
    complete: bool,
    next_row: u64,
    chunk: Vec<u8>,
}

impl Scan<File> {
    /// Opens the file at `path` read-only, with the default sharing, and reads its headers.
    pub fn open(path: &Path) -> Result<Scan<File>, IbtError> {
        let file = File::open(path)?;
        let len = file.metadata()?.len();
        Scan::new(file, len)
    }
}

impl<R: Read> Scan<R> {
    /// Reads the headers, the variable table and the session text from `reader`, which holds a
    /// file of `file_len` bytes, positioned at its start.
    ///
    /// A file without a row yields [`IbtError::NoRows`].
    pub fn new(mut reader: R, file_len: u64) -> Result<Scan<R>, IbtError> {
        let mut prefix = vec![0u8; DISK_HEADER_END];
        reader.read_exact(&mut prefix)?;
        let total = usize::try_from(file_len).unwrap_or(usize::MAX);
        let header = Header::parse_in(&prefix, total, false)?;
        let rows_at = u64::try_from(header.var_buf[0].buf_offset)
            .ok()
            .filter(|&at| at >= DISK_HEADER_END as u64 && at <= file_len && at <= MAX_PREFIX)
            .ok_or(LayoutError::PastEnd {
                field: "var_buf[0].buf_offset",
            })?;
        let prefix_len = usize::try_from(rows_at).unwrap_or(DISK_HEADER_END);
        prefix.resize(prefix_len, 0);
        reader.read_exact(&mut prefix[DISK_HEADER_END..])?;
        let disk = DiskHeader::parse(&prefix)?;
        let vars = VarTable::parse(&prefix, &header)?;
        let at = usize::try_from(header.session_info_offset).unwrap_or(0);
        let len = usize::try_from(header.session_info_len).unwrap_or(0);
        let yaml = prefix
            .get(at..at.saturating_add(len))
            .ok_or(LayoutError::PastEnd {
                field: "session_info_offset",
            })?
            .to_vec();
        let buf_len = usize::try_from(header.buf_len).unwrap_or(0).max(1);
        let fit = (file_len - rows_at) / buf_len as u64;
        let declared = u64::try_from(disk.session_record_count).unwrap_or(0);
        let rows = if declared > 0 { declared.min(fit) } else { fit };
        if rows == 0 {
            return Err(IbtError::NoRows);
        }
        let complete = declared > 0 && file_len == rows_at + declared * buf_len as u64;
        let chunk_rows = (CHUNK_BYTES / buf_len).max(1);
        Ok(Scan {
            reader,
            header,
            disk,
            vars,
            yaml,
            file_len,
            buf_len,
            rows,
            complete,
            next_row: 0,
            chunk: Vec::with_capacity(chunk_rows * buf_len),
        })
    }

    /// Whether the file is complete: a record count above zero and a size of exactly the first
    /// buffer offset plus the record count times the row length.
    pub fn is_complete(&self) -> bool {
        self.complete
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

    /// The size of the file in bytes, as given at the start.
    pub fn file_len(&self) -> u64 {
        self.file_len
    }

    /// The session text, decoded.
    pub fn session_text(&self) -> Decoded {
        yaml::decode(&self.yaml)
    }

    /// The session text, decoded and parsed now.
    pub fn session(&self) -> crate::worker::SessionDoc {
        crate::worker::SessionDoc::from_raw(self.header.session_info_update, &self.yaml)
    }

    /// The number of rows the scan will yield.
    pub fn rows(&self) -> u64 {
        self.rows
    }

    /// The length of one row in bytes.
    pub fn row_len(&self) -> usize {
        self.buf_len
    }

    /// Resolves the scalar variable `name` as `T` for reading from the rows of a chunk.
    pub fn field<T: VarValue>(&self, name: &str) -> Result<Field<T>, VarError> {
        let h = self.vars.get(name).ok_or(VarError::Missing)?;
        if !T::accepts(h.ty) {
            return Err(VarError::WrongType { found: h.ty });
        }
        if h.count != 1 {
            return Err(VarError::NotScalar { count: h.count });
        }
        Ok(Field {
            offset: h.offset,
            _t: PhantomData,
        })
    }

    /// Reads the next chunk of rows (about [`CHUNK_BYTES`]); `None` after the last row.
    pub fn next_chunk(&mut self) -> io::Result<Option<Chunk<'_>>> {
        if self.next_row >= self.rows {
            return Ok(None);
        }
        let per_chunk = (CHUNK_BYTES / self.buf_len).max(1) as u64;
        let n = per_chunk.min(self.rows - self.next_row);
        let bytes = usize::try_from(n).unwrap_or(0) * self.buf_len;
        self.chunk.resize(bytes, 0);
        self.reader.read_exact(&mut self.chunk)?;
        let first = self.next_row;
        self.next_row += n;
        Ok(Some(Chunk {
            first,
            row_len: self.buf_len,
            data: &self.chunk,
        }))
    }
}
