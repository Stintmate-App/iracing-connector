//! Replays the rows of an `.ibt` image into a [`TestMap`] at a chosen rate.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use super::map::TestMap;
use crate::ibt::Ibt;

/// The layout the replay map needs for `ibt`: its variables at their offsets, 3 buffers.
pub fn layout_of(ibt: &Ibt) -> super::Layout {
    super::Layout::from_table(
        ibt.vars(),
        ibt.header().tick_rate,
        3,
        ibt.header().buf_len as usize,
    )
}

/// Sleeps until `deadline`: a coarse sleep, then a short spin for the last stretch.
fn sleep_until(deadline: Instant) {
    const SPIN: Duration = Duration::from_millis(2);
    loop {
        let now = Instant::now();
        if now >= deadline {
            return;
        }
        let left = deadline - now;
        if left > SPIN {
            std::thread::sleep(left - SPIN);
        } else {
            std::hint::spin_loop();
        }
    }
}

/// Writes the session text of `ibt` to `map`, then pushes its rows in order at `hz` rows per
/// second until the end or until `stop` is set. Each push advances the map's tick count;
/// writing the session text advanced its update counter. Returns the number of rows pushed.
///
/// `map` must have been created with [`layout_of`]`(ibt)` (or a layout with the same row
/// length).
pub fn replay(ibt: &Ibt, map: &mut TestMap, hz: f64, stop: &AtomicBool) -> usize {
    assert!(hz.is_finite() && hz > 0.0, "hz must be above 0");
    map.set_yaml(ibt.yaml_raw());
    let step = Duration::from_secs_f64(1.0 / hz);
    let start = Instant::now();
    let mut pushed = 0;
    for row in ibt.rows() {
        if stop.load(Ordering::Relaxed) {
            break;
        }
        sleep_until(start + step.mul_f64(pushed as f64));
        map.push_row(row);
        pushed += 1;
    }
    pushed
}
