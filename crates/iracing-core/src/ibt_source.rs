//! A [`Source`] over a recorded `.ibt` file: one row per call, the session text parsed once.

use std::sync::Arc;

use irsdk::ibt::Ibt;
use irsdk::row::Row;

use crate::io::{Source, SourceEvent};
use crate::meta::{self, SessionMeta};
use crate::tick::Tick;
use crate::vars::{Vars, VarsError};

/// Replays the rows of an `.ibt` file.
///
/// The file holds one session text, so the whole replay has one [`SessionMeta`], built for the
/// session number of the first row.
pub struct IbtSource {
    ibt: Ibt,
    vars: Vars,
    row: Row,
    next: usize,
    meta: Option<Arc<SessionMeta>>,
}

impl IbtSource {
    /// A source over `ibt`. Fails when the file lacks a variable the recorder requires.
    pub fn new(ibt: Ibt) -> Result<IbtSource, VarsError> {
        let vars = Vars::resolve(ibt.vars())?;
        let mut row = Row::new(ibt.row_len(), 0);
        let mut meta = None;
        if let Some(first) = ibt.row(0) {
            row.bytes_mut().copy_from_slice(first);
            let mut tick = Tick::new();
            tick.fill(&vars, &row);
            meta = Some(Arc::new(meta::build(&ibt.session(), tick.session_num)));
        }
        Ok(IbtSource {
            ibt,
            vars,
            row,
            next: 0,
            meta,
        })
    }

    /// The index of the next row.
    pub fn position(&self) -> usize {
        self.next
    }
}

impl Source for IbtSource {
    fn next(&mut self, tick: &mut Tick) -> SourceEvent {
        let Some(bytes) = self.ibt.row(self.next) else {
            return SourceEvent::End;
        };
        self.row.bytes_mut().copy_from_slice(bytes);
        self.row
            .set_tick(i32::try_from(self.next).unwrap_or(i32::MAX));
        tick.fill(&self.vars, &self.row);
        tick.connected = true;
        tick.fresh = true;
        self.next += 1;
        SourceEvent::Row
    }

    fn meta(&self) -> Option<&Arc<SessionMeta>> {
        self.meta.as_ref()
    }
}

#[cfg(all(test, feature = "testing"))]
mod tests {
    use super::*;
    use crate::testing::{LapProfile, Scenario, YamlBuilder};

    #[test]
    fn replays_every_row_then_ends() {
        let mut s = Scenario::new(YamlBuilder::new());
        s.garage(0.5).to_stall().drive_out();
        s.lap(LapProfile::new(10.0));
        let rows = s.rows().len();
        let mut src = IbtSource::new(s.ibt()).expect("source");
        assert!(src.meta().is_some_and(|m| m.player.is_some()));
        let mut tick = Tick::new();
        let mut n = 0;
        let mut last_time = -1.0;
        while src.next(&mut tick) == SourceEvent::Row {
            assert!(tick.connected && tick.fresh);
            assert!(tick.session_time > last_time);
            last_time = tick.session_time;
            n += 1;
        }
        assert_eq!(n, rows);
        assert_eq!(src.position(), rows);
        assert_eq!(src.next(&mut tick), SourceEvent::End);
    }
}
