//! The sampler: attach to a view, poll it for new rows, copy the newest row with a tick check.

use crate::defs::STATUS_CONNECTED;
use crate::header::{HEADER_LEN, Header, LayoutError, VAR_HEADER_LEN};
use crate::row::Row;
use crate::vars::VarTable;
use crate::view::View;

const OFF_STATUS: usize = 4;
const OFF_SESSION_UPDATE: usize = 12;
const OFF_NUM_VARS: usize = 24;
const OFF_VAR_HEADER: usize = 28;
const OFF_NUM_BUF: usize = 32;
const OFF_BUF_LEN: usize = 36;
const OFF_BUFS: usize = 48;
/// Copies of one buffer repeated when the tick moved under the copy.
const MAX_RETRIES: u32 = 2;

/// What one [`Sampler::poll`] found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Poll {
    /// A new row was copied; read it with [`Sampler::row`].
    Row,
    /// Nothing newer than the last row (or the row was dropped by the rate decimation).
    NoNewRow,
    /// The layout changed or the tick count dropped: the variables were resolved again, so
    /// handles from before must be resolved again too.
    Reset,
    /// The simulator is not connected, or the layout could not be read again.
    Disconnected,
    /// The row kept changing while it was copied; the last row is unchanged.
    Torn,
}

/// Counters of what the sampler did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SamplerStats {
    /// Rows delivered.
    pub rows: u64,
    /// Polls that gave up on a changing row.
    pub torn: u64,
    /// Copies repeated because the tick moved during the copy.
    pub retries: u64,
    /// Ticks dropped to bring a faster tick rate down to 60 rows per second.
    pub skipped_rate: u64,
    /// Times the layout was read again.
    pub resets: u64,
}

/// Reads rows from a [`View`] of the live data.
///
/// Parsing runs on copies, never on the view itself, because the simulator writes the view while
/// it is read. [`Sampler::poll`] does not allocate except when it reads the layout again.
#[derive(Debug)]
pub struct Sampler<V: View> {
    view: V,
    header: Header,
    vars: VarTable,
    row: Row,
    generation: u64,
    last_tick: i32,
    last_kept: i64,
    every: i32,
    stats: SamplerStats,
}

struct Layout {
    header: Header,
    vars: VarTable,
    row: Row,
}

fn read_layout<V: View>(view: &V, generation: u64) -> Result<Layout, LayoutError> {
    let mut head = [0u8; HEADER_LEN];
    if !view.copy_out(0, &mut head) {
        return Err(LayoutError::TooShort {
            field: "header",
            need: HEADER_LEN,
            have: view.len(),
        });
    }
    let header = Header::parse_in(&head, view.len(), true)?;
    // The header was validated: the variable headers lie inside the view.
    let end = header.var_header_offset as usize + header.num_vars as usize * VAR_HEADER_LEN;
    let mut region = vec![0u8; end];
    if !view.copy_out(0, &mut region) {
        return Err(LayoutError::PastEnd {
            field: "var_header_offset",
        });
    }
    let vars = VarTable::parse(&region, &header)?.with_generation(generation);
    let row = Row::new(header.buf_len as usize, generation);
    Ok(Layout { header, vars, row })
}

fn every_of(header: &Header) -> i32 {
    (header.tick_rate / 60).max(1)
}

impl<V: View> Sampler<V> {
    /// Copies the header and the variable headers out of `view`, parses them and allocates the
    /// row. The first row is delivered by the first poll that finds a tick above zero.
    pub fn attach(view: V) -> Result<Sampler<V>, LayoutError> {
        let l = read_layout(&view, 1)?;
        Ok(Sampler {
            view,
            every: every_of(&l.header),
            header: l.header,
            vars: l.vars,
            row: l.row,
            generation: 1,
            last_tick: 0,
            last_kept: i64::MIN / 2,
            stats: SamplerStats::default(),
        })
    }

    /// Looks for a new row and copies it.
    pub fn poll(&mut self) -> Poll {
        match self.view.load_i32(OFF_STATUS) {
            Some(status) if status & STATUS_CONNECTED != 0 => {}
            _ => return Poll::Disconnected,
        }
        if self.layout_changed() {
            return self.reset();
        }
        let mut attempt = 0;
        loop {
            let Some((slot, tick)) = self.newest() else {
                return self.reset();
            };
            if tick < self.last_tick {
                return self.reset();
            }
            if tick == self.last_tick {
                return Poll::NoNewRow;
            }
            if i64::from(tick) - self.last_kept < i64::from(self.every) {
                self.last_tick = tick;
                self.stats.skipped_rate += 1;
                return Poll::NoNewRow;
            }
            let off = self
                .view
                .load_i32(OFF_BUFS + 16 * slot + 4)
                .and_then(|o| usize::try_from(o).ok());
            let Some(off) = off else {
                return self.reset();
            };
            if !self.view.copy_out(off, self.row.bytes_mut()) {
                return self.reset();
            }
            if self.view.load_i32(OFF_BUFS + 16 * slot) == Some(tick) {
                self.row.set_tick(tick);
                self.last_tick = tick;
                self.last_kept = i64::from(tick);
                self.stats.rows += 1;
                return Poll::Row;
            }
            attempt += 1;
            if attempt > MAX_RETRIES {
                self.stats.torn += 1;
                return Poll::Torn;
            }
            self.stats.retries += 1;
        }
    }

    /// The slot with the highest tick count, and that tick.
    fn newest(&self) -> Option<(usize, i32)> {
        let mut best: Option<(usize, i32)> = None;
        for slot in 0..self.header.num_buf as usize {
            let tick = self.view.load_i32(OFF_BUFS + 16 * slot)?;
            if best.is_none_or(|(_, t)| tick > t) {
                best = Some((slot, tick));
            }
        }
        best
    }

    fn layout_changed(&self) -> bool {
        let h = &self.header;
        let same = |off: usize, want: i32| self.view.load_i32(off) == Some(want);
        !(same(OFF_NUM_VARS, h.num_vars)
            && same(OFF_VAR_HEADER, h.var_header_offset)
            && same(OFF_NUM_BUF, h.num_buf)
            && same(OFF_BUF_LEN, h.buf_len))
    }

    fn reset(&mut self) -> Poll {
        self.stats.resets += 1;
        match read_layout(&self.view, self.generation + 1) {
            Ok(l) => {
                self.generation += 1;
                self.every = every_of(&l.header);
                self.header = l.header;
                self.vars = l.vars;
                self.row = l.row;
                self.last_tick = 0;
                self.last_kept = i64::MIN / 2;
                Poll::Reset
            }
            Err(_) => Poll::Disconnected,
        }
    }

    /// The last row delivered.
    pub fn row(&self) -> &Row {
        &self.row
    }

    /// The variables of the current layout.
    pub fn vars(&self) -> &VarTable {
        &self.vars
    }

    /// The header as of the attach or the last reset.
    pub fn header(&self) -> &Header {
        &self.header
    }

    /// Whether the simulator marks itself as connected (an atomic load).
    pub fn connected(&self) -> bool {
        self.view
            .load_i32(OFF_STATUS)
            .is_some_and(|s| s & STATUS_CONNECTED != 0)
    }

    /// The counter that moves when the session YAML changes (an atomic load).
    pub fn session_info_update(&self) -> i32 {
        self.view.load_i32(OFF_SESSION_UPDATE).unwrap_or(0)
    }

    /// What the sampler did so far.
    pub fn stats(&self) -> SamplerStats {
        self.stats
    }

    /// The view being read.
    pub fn view(&self) -> &V {
        &self.view
    }
}
