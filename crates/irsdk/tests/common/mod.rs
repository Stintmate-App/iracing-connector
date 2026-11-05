//! A scripted view for the sampler tests: an image that the test changes between calls.

#![allow(dead_code)]

use std::cell::{Cell, RefCell};

use irsdk::defs::VarType;
use irsdk::testing::{Layout, LayoutBuilder, RowBuilder, live_image};
use irsdk::view::View;

/// A layout with `Speed` (float) and `Lap` (int).
pub fn layout(tick_rate: i32, num_buf: usize) -> Layout {
    LayoutBuilder::new()
        .tick_rate(tick_rate)
        .num_buf(num_buf)
        .var("Speed", VarType::Float, 1, "m/s", "")
        .var("Lap", VarType::Int, 1, "", "")
        .build()
}

/// One row of `layout()` with the given values.
pub fn row(layout: &Layout, speed: f32, lap: i32) -> Vec<u8> {
    let mut b = RowBuilder::new(layout);
    b.set_f32("Speed", speed).set_i32("Lap", lap);
    b.into_bytes()
}

/// Where the tick count of `slot` and its buffer offset are in the header.
fn tick_off(slot: usize) -> usize {
    48 + 16 * slot
}

/// An image the test edits, with an optional edit applied after every `copy_out`.
pub struct Script {
    image: RefCell<Vec<u8>>,
    /// When set, the tick of the slot last copied moves by one after each copy.
    pub tear: Cell<bool>,
    last_slot: Cell<usize>,
}

impl Script {
    pub fn new(image: Vec<u8>) -> Script {
        Script {
            image: RefCell::new(image),
            tear: Cell::new(false),
            last_slot: Cell::new(0),
        }
    }

    pub fn from_rows(layout: &Layout, rows: &[(i32, Vec<u8>)]) -> Script {
        Script::new(live_image(layout, "---\n", rows))
    }

    pub fn set_i32(&self, off: usize, v: i32) {
        self.image.borrow_mut()[off..off + 4].copy_from_slice(&v.to_le_bytes());
    }

    pub fn get_i32(&self, off: usize) -> i32 {
        let img = self.image.borrow();
        i32::from_le_bytes(img[off..off + 4].try_into().unwrap())
    }

    pub fn set_tick(&self, slot: usize, tick: i32) {
        self.set_i32(tick_off(slot), tick);
    }

    pub fn set_status(&self, status: i32) {
        self.set_i32(4, status);
    }

    /// Writes `row` into the buffer of `slot`.
    pub fn set_row(&self, slot: usize, row: &[u8]) {
        let off = self.get_i32(tick_off(slot) + 4) as usize;
        self.image.borrow_mut()[off..off + row.len()].copy_from_slice(row);
    }

    pub fn replace(&self, image: Vec<u8>) {
        *self.image.borrow_mut() = image;
    }
}

impl View for Script {
    fn len(&self) -> usize {
        self.image.borrow().len()
    }

    fn load_i32(&self, off: usize) -> Option<i32> {
        let img = self.image.borrow();
        if !off.is_multiple_of(4) {
            return None;
        }
        let raw: [u8; 4] = img.get(off..off + 4)?.try_into().ok()?;
        Some(i32::from_le_bytes(raw))
    }

    fn copy_out(&self, off: usize, dst: &mut [u8]) -> bool {
        let ok = {
            let img = self.image.borrow();
            match img.get(off..off + dst.len()) {
                Some(src) => {
                    dst.copy_from_slice(src);
                    true
                }
                None => false,
            }
        };
        if ok && self.tear.get() {
            // A row copy: find the slot whose buffer starts at `off` and move its tick.
            for slot in 0..4 {
                if self.get_i32(tick_off(slot) + 4) as usize == off {
                    self.last_slot.set(slot);
                    let t = self.get_i32(tick_off(slot));
                    self.set_tick(slot, t + 1);
                }
            }
        }
        ok
    }
}
