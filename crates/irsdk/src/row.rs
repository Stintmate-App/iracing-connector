//! The preallocated copy of one row of telemetry.

/// One row: the bytes of a buffer, copied out of the live map or read from a file, with its tick.
///
/// The allocation is made once; [`Row::bytes_mut`] and [`Row::set_tick`] refill it.
#[derive(Clone, Debug)]
pub struct Row {
    bytes: Vec<u8>,
    tick: i32,
    generation: u64,
}

impl Row {
    /// A zeroed row of `len` bytes, for the variable table of generation `generation`.
    pub fn new(len: usize, generation: u64) -> Row {
        Row {
            bytes: vec![0; len],
            tick: 0,
            generation,
        }
    }

    /// A row holding a copy of `bytes`, generation 0, tick 0.
    pub fn from_slice(bytes: &[u8]) -> Row {
        Row {
            bytes: bytes.to_vec(),
            tick: 0,
            generation: 0,
        }
    }

    /// The row bytes.
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// The row bytes, to be refilled.
    pub fn bytes_mut(&mut self) -> &mut [u8] {
        &mut self.bytes
    }

    /// Length of the row in bytes.
    pub fn len(&self) -> usize {
        self.bytes.len()
    }

    /// Whether the row has no bytes.
    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }

    /// The tick count the row was copied at.
    pub fn tick(&self) -> i32 {
        self.tick
    }

    /// Sets the tick count.
    pub fn set_tick(&mut self, tick: i32) {
        self.tick = tick;
    }

    /// The generation of the variable table this row belongs to.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Sets the generation, after the variable table was resolved again.
    pub fn set_generation(&mut self, generation: u64) {
        self.generation = generation;
    }
}
