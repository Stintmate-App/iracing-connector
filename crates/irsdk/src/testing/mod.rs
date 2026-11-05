//! Synthetic data for tests: layouts, rows and whole live images.
//!
//! The builders panic on misuse (an unknown variable name, a wrong type): they are test tools.

use crate::defs::{MAX_BUFS, MAX_DESC, MAX_STRING, STATUS_CONNECTED, VER, VarType};
use crate::header::{HEADER_LEN, VAR_HEADER_LEN};
use crate::vars::VarValue;

/// One variable of a [`Layout`].
#[derive(Clone, Debug)]
pub struct LayoutVar {
    /// The name.
    pub name: String,
    /// The type.
    pub ty: VarType,
    /// Array length.
    pub count: usize,
    /// The unit.
    pub unit: String,
    /// The description.
    pub desc: String,
    /// Offset in the row, in bytes.
    pub offset: usize,
}

/// Builds a [`Layout`]: variables packed in order, 4-aligned (8 for doubles).
#[derive(Clone, Debug)]
pub struct LayoutBuilder {
    tick_rate: i32,
    num_buf: usize,
    vars: Vec<LayoutVar>,
    next: usize,
}

impl Default for LayoutBuilder {
    fn default() -> Self {
        LayoutBuilder::new()
    }
}

impl LayoutBuilder {
    /// An empty layout: 60 ticks per second, 3 buffers.
    pub fn new() -> LayoutBuilder {
        LayoutBuilder {
            tick_rate: 60,
            num_buf: 3,
            vars: Vec::new(),
            next: 0,
        }
    }

    /// Sets the tick rate.
    pub fn tick_rate(mut self, rate: i32) -> LayoutBuilder {
        self.tick_rate = rate;
        self
    }

    /// Sets the number of row buffers (1 to 4).
    pub fn num_buf(mut self, n: usize) -> LayoutBuilder {
        assert!((1..=MAX_BUFS).contains(&n), "num_buf must be 1 to 4");
        self.num_buf = n;
        self
    }

    /// Adds a variable at the next aligned offset.
    pub fn var(mut self, name: &str, ty: VarType, count: usize, unit: &str, desc: &str) -> Self {
        let align = if ty == VarType::Double { 8 } else { 4 };
        let offset = self.next.next_multiple_of(align);
        self.next = offset + ty.size() * count;
        self.vars.push(LayoutVar {
            name: name.to_owned(),
            ty,
            count,
            unit: unit.to_owned(),
            desc: desc.to_owned(),
            offset,
        });
        self
    }

    /// The finished layout; the row length is the total rounded up to 8 bytes.
    pub fn build(self) -> Layout {
        Layout {
            tick_rate: self.tick_rate,
            num_buf: self.num_buf,
            buf_len: self.next.next_multiple_of(8).max(8),
            vars: self.vars,
        }
    }
}

/// A synthetic layout: the variables and the sizes, with the bytes of its headers.
#[derive(Clone, Debug)]
pub struct Layout {
    tick_rate: i32,
    num_buf: usize,
    buf_len: usize,
    vars: Vec<LayoutVar>,
}

impl Layout {
    /// Length of one row in bytes.
    pub fn buf_len(&self) -> usize {
        self.buf_len
    }

    /// Number of row buffers.
    pub fn num_buf(&self) -> usize {
        self.num_buf
    }

    /// Ticks per second.
    pub fn tick_rate(&self) -> i32 {
        self.tick_rate
    }

    /// The variables, in order.
    pub fn vars(&self) -> &[LayoutVar] {
        &self.vars
    }

    /// The variable called `name`.
    pub fn var(&self, name: &str) -> Option<&LayoutVar> {
        self.vars.iter().find(|v| v.name == name)
    }

    /// The bytes of all the variable headers, 144 bytes each.
    pub fn var_header_bytes(&self) -> Vec<u8> {
        let mut out = vec![0u8; self.vars.len() * VAR_HEADER_LEN];
        for (i, v) in self.vars.iter().enumerate() {
            let raw = &mut out[i * VAR_HEADER_LEN..(i + 1) * VAR_HEADER_LEN];
            put_i32(raw, 0, v.ty as i32);
            put_i32(raw, 4, v.offset as i32);
            put_i32(raw, 8, v.count as i32);
            put_text(&mut raw[16..16 + MAX_STRING], &v.name);
            put_text(&mut raw[48..48 + MAX_DESC], &v.desc);
            put_text(&mut raw[112..112 + MAX_STRING], &v.unit);
        }
        out
    }

    /// The 112 bytes of the header for the given YAML position and buffers
    /// (`(tick_count, buf_offset)` for each of the first `num_buf` slots; others stay zero).
    pub fn header_bytes(
        &self,
        session_info_update: i32,
        session_info: (usize, usize),
        var_header_offset: usize,
        bufs: &[(i32, usize)],
    ) -> Vec<u8> {
        let mut h = vec![0u8; HEADER_LEN];
        put_i32(&mut h, 0, VER);
        put_i32(&mut h, 4, STATUS_CONNECTED);
        put_i32(&mut h, 8, self.tick_rate);
        put_i32(&mut h, 12, session_info_update);
        put_i32(&mut h, 16, session_info.1 as i32);
        put_i32(&mut h, 20, session_info.0 as i32);
        put_i32(&mut h, 24, self.vars.len() as i32);
        put_i32(&mut h, 28, var_header_offset as i32);
        put_i32(&mut h, 32, self.num_buf as i32);
        put_i32(&mut h, 36, self.buf_len as i32);
        for (i, (tick, off)) in bufs.iter().take(MAX_BUFS).enumerate() {
            put_i32(&mut h, 48 + 16 * i, *tick);
            put_i32(&mut h, 52 + 16 * i, *off as i32);
        }
        h
    }
}

fn put_i32(b: &mut [u8], off: usize, v: i32) {
    b[off..off + 4].copy_from_slice(&v.to_le_bytes());
}

/// Writes `text` NUL-terminated into `dst`, cut to fit; characters above U+00FF are written
/// as `?`, the others as their cp1252 byte.
fn put_text(dst: &mut [u8], text: &str) {
    let room = dst.len() - 1;
    for (slot, c) in dst.iter_mut().zip(text.chars().take(room)) {
        *slot = u8::try_from(u32::from(c)).unwrap_or(b'?');
    }
}

/// Writes values into one row of a [`Layout`] by variable name.
#[derive(Clone, Debug)]
pub struct RowBuilder<'a> {
    layout: &'a Layout,
    bytes: Vec<u8>,
}

impl<'a> RowBuilder<'a> {
    /// A zeroed row for `layout`.
    pub fn new(layout: &'a Layout) -> RowBuilder<'a> {
        RowBuilder {
            layout,
            bytes: vec![0; layout.buf_len()],
        }
    }

    fn put<T: VarValue>(&mut self, name: &str, i: usize, v: T) -> &mut Self {
        let var = self
            .layout
            .var(name)
            .unwrap_or_else(|| panic!("no variable {name}"));
        assert!(T::accepts(var.ty), "variable {name} has type {:?}", var.ty);
        assert!(
            i < var.count,
            "index {i} past the {} values of {name}",
            var.count
        );
        let at = var.offset + i * T::SIZE;
        v.write_le(&mut self.bytes[at..at + T::SIZE]);
        self
    }

    /// Sets a float variable.
    pub fn set_f32(&mut self, name: &str, v: f32) -> &mut Self {
        self.put(name, 0, v)
    }
    /// Sets a double variable.
    pub fn set_f64(&mut self, name: &str, v: f64) -> &mut Self {
        self.put(name, 0, v)
    }
    /// Sets an int or bit field variable.
    pub fn set_i32(&mut self, name: &str, v: i32) -> &mut Self {
        self.put(name, 0, v)
    }
    /// Sets a bit field or int variable.
    pub fn set_u32(&mut self, name: &str, v: u32) -> &mut Self {
        self.put(name, 0, v)
    }
    /// Sets a boolean variable.
    pub fn set_bool(&mut self, name: &str, v: bool) -> &mut Self {
        self.put(name, 0, v)
    }
    /// Sets a character variable.
    pub fn set_u8(&mut self, name: &str, v: u8) -> &mut Self {
        self.put(name, 0, v)
    }

    /// Sets element `i` of a float array.
    pub fn set_f32_at(&mut self, name: &str, i: usize, v: f32) -> &mut Self {
        self.put(name, i, v)
    }
    /// Sets element `i` of a double array.
    pub fn set_f64_at(&mut self, name: &str, i: usize, v: f64) -> &mut Self {
        self.put(name, i, v)
    }
    /// Sets element `i` of an int array.
    pub fn set_i32_at(&mut self, name: &str, i: usize, v: i32) -> &mut Self {
        self.put(name, i, v)
    }
    /// Sets element `i` of a bit field array.
    pub fn set_u32_at(&mut self, name: &str, i: usize, v: u32) -> &mut Self {
        self.put(name, i, v)
    }
    /// Sets element `i` of a boolean array.
    pub fn set_bool_at(&mut self, name: &str, i: usize, v: bool) -> &mut Self {
        self.put(name, i, v)
    }
    /// Sets element `i` of a character array.
    pub fn set_u8_at(&mut self, name: &str, i: usize, v: u8) -> &mut Self {
        self.put(name, i, v)
    }

    /// The row bytes.
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// The row bytes, by value.
    pub fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }
}

/// A whole live map image: header, variable headers, session YAML, then the row buffers.
///
/// Row `k` of `rows` goes to slot `k % num_buf` (later rows overwrite earlier ones, as the
/// simulator's ring does); slots no row reaches stay zeroed with tick 0. The session update
/// counter is 1 and the status says connected. Every row must be `layout.buf_len()` bytes.
pub fn live_image(layout: &Layout, yaml: impl AsRef<[u8]>, rows: &[(i32, Vec<u8>)]) -> Vec<u8> {
    let yaml = yaml.as_ref();
    let var_off = HEADER_LEN;
    let yaml_off = var_off + layout.vars().len() * VAR_HEADER_LEN;
    let first_buf = (yaml_off + yaml.len()).next_multiple_of(16);
    let stride = layout.buf_len().next_multiple_of(16);
    let slots: Vec<usize> = (0..layout.num_buf())
        .map(|i| first_buf + i * stride)
        .collect();

    let mut ticks = vec![0i32; layout.num_buf()];
    let mut image = vec![0u8; first_buf + layout.num_buf() * stride];
    for (k, (tick, row)) in rows.iter().enumerate() {
        assert_eq!(row.len(), layout.buf_len(), "row {k} has the wrong length");
        let slot = k % layout.num_buf();
        ticks[slot] = *tick;
        image[slots[slot]..slots[slot] + row.len()].copy_from_slice(row);
    }
    let bufs: Vec<(i32, usize)> = ticks.iter().copied().zip(slots.iter().copied()).collect();
    let header = layout.header_bytes(1, (yaml_off, yaml.len()), var_off, &bufs);
    image[..HEADER_LEN].copy_from_slice(&header);
    image[var_off..yaml_off].copy_from_slice(&layout.var_header_bytes());
    image[yaml_off..yaml_off + yaml.len()].copy_from_slice(yaml);
    image
}
