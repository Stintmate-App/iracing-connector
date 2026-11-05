//! The live reader: the simulator's memory map and its data-valid event, read-only.
//!
//! Windows only. The real names are opened for reading; nothing in this crate writes to them.

pub mod sys;

use std::sync::Arc;
use std::time::Duration;

use crate::defs::{DATA_VALID_EVENT_NAME, MEM_MAP_NAME};
use crate::header::LayoutError;
use crate::sampler::{Poll, Sampler};

pub use sys::{Event, MapView};

/// Why a live map could not be opened.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OpenError {
    /// The mapping or the event does not exist: the simulator is not running.
    NotRunning,
    /// The mapping exists but its layout was refused.
    Layout(LayoutError),
    /// Another Windows error, with its code.
    Os(u32),
}

impl std::fmt::Display for OpenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OpenError::NotRunning => write!(f, "the simulator's data is not available"),
            OpenError::Layout(e) => write!(f, "the live layout was refused: {e}"),
            OpenError::Os(code) => write!(f, "Windows error {code}"),
        }
    }
}

impl std::error::Error for OpenError {}

/// Windows error codes that mean "the object is not there".
const ERROR_FILE_NOT_FOUND: u32 = 2;
const ERROR_PATH_NOT_FOUND: u32 = 3;

fn open_error(code: u32) -> OpenError {
    match code {
        ERROR_FILE_NOT_FOUND | ERROR_PATH_NOT_FOUND => OpenError::NotRunning,
        other => OpenError::Os(other),
    }
}

/// Whether the simulator's memory map exists. Nothing is mapped or read: this is a hint that
/// the simulator is running.
pub fn sim_mapping_exists() -> bool {
    sys::mapping_exists(MEM_MAP_NAME)
}

/// The live data of the simulator: a sampler over the memory map and the event that announces
/// new rows.
#[derive(Debug)]
pub struct Live {
    sampler: Sampler<Arc<MapView>>,
    event: Event,
}

impl Live {
    /// Opens the simulator's real map and event.
    pub fn open() -> Result<Live, OpenError> {
        Live::open_named(MEM_MAP_NAME, DATA_VALID_EVENT_NAME)
    }

    /// Opens a map and an event by name (test mappings), read-only like [`Live::open`].
    pub fn open_named(map: &str, event: &str) -> Result<Live, OpenError> {
        let view = MapView::open(map).map_err(open_error)?;
        let event = Event::open(event).map_err(open_error)?;
        let sampler = Sampler::attach(Arc::new(view)).map_err(OpenError::Layout)?;
        Ok(Live { sampler, event })
    }

    /// Waits for the event for at most `timeout`, then polls. The poll runs whether the event
    /// was signalled or the wait timed out: the event is a hint, the tick count is the truth.
    pub fn wait(&mut self, timeout: Duration) -> Poll {
        self.event.wait(timeout);
        self.sampler.poll()
    }

    /// The sampler, for the row, the variables and the counters.
    pub fn sampler(&self) -> &Sampler<Arc<MapView>> {
        &self.sampler
    }

    /// The sampler, to poll without waiting.
    pub fn sampler_mut(&mut self) -> &mut Sampler<Arc<MapView>> {
        &mut self.sampler
    }

    /// The mapped view, to read the session text from another thread.
    pub fn view(&self) -> Arc<MapView> {
        Arc::clone(self.sampler.view())
    }
}
