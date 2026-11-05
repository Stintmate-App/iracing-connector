//! A writer of a named memory mapping that looks like the simulator's, for tests.
//!
//! It refuses the simulator's real names and any name containing `IRSDK`, so a test never
//! writes where the game would.

#![allow(unsafe_code)]

use std::ffi::c_void;
use std::io;
use std::sync::atomic::{AtomicI32, Ordering};

use super::Layout;
use crate::defs::{DATA_VALID_EVENT_NAME, MEM_MAP_NAME, STATUS_CONNECTED};
use crate::header::{HEADER_LEN, VAR_HEADER_LEN};

type Handle = isize;

const INVALID_HANDLE_VALUE: Handle = -1;
const PAGE_READWRITE: u32 = 0x04;
const FILE_MAP_ALL_ACCESS: u32 = 0x000F_001F;
/// Room kept for the session text in the mapping, in bytes (more when the first text is longer).
const YAML_ROOM: usize = 64 * 1024;

unsafe extern "system" {
    fn CreateFileMappingW(
        file: Handle,
        attributes: *const c_void,
        protect: u32,
        size_hi: u32,
        size_lo: u32,
        name: *const u16,
    ) -> Handle;
    fn MapViewOfFile(map: Handle, access: u32, hi: u32, lo: u32, bytes: usize) -> *mut c_void;
    fn UnmapViewOfFile(base: *const c_void) -> i32;
    fn CreateEventW(
        attributes: *const c_void,
        manual_reset: i32,
        initial: i32,
        name: *const u16,
    ) -> Handle;
    fn SetEvent(handle: Handle) -> i32;
    fn CloseHandle(handle: Handle) -> i32;
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

fn refuse_real_name(name: &str) -> io::Result<()> {
    let upper = name.to_ascii_uppercase();
    if name == MEM_MAP_NAME || name == DATA_VALID_EVENT_NAME || upper.contains("IRSDK") {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "a test mapping must not use the simulator's names",
        ));
    }
    Ok(())
}

/// Where the parts of a laid-out mapping are.
#[derive(Clone, Debug)]
struct Plan {
    yaml_off: usize,
    yaml_room: usize,
    slots: Vec<usize>,
    total: usize,
}

fn plan(layout: &Layout, yaml_len: usize) -> Plan {
    let yaml_off = HEADER_LEN + layout.vars().len() * VAR_HEADER_LEN;
    let yaml_room = YAML_ROOM.max(yaml_len);
    let first = (yaml_off + yaml_room).next_multiple_of(16);
    let stride = layout.buf_len().next_multiple_of(16);
    let slots = (0..layout.num_buf()).map(|i| first + i * stride).collect();
    Plan {
        yaml_off,
        yaml_room,
        slots,
        total: first + layout.num_buf() * stride,
    }
}

/// A named mapping and event written like the simulator writes its own.
#[derive(Debug)]
pub struct TestMap {
    map: Handle,
    event: Handle,
    base: *mut u8,
    size: usize,
    plan: Plan,
    num_buf: usize,
    buf_len: usize,
    var_count: usize,
    tick_rate: i32,
    next_slot: usize,
    tick: i32,
    update: i32,
}

// SAFETY: the raw pointer is the base of a view this value owns; every access goes through
// `&self` or `&mut self` of the one owner, and the handles are plain values.
unsafe impl Send for TestMap {}

impl TestMap {
    /// Creates the mapping `map_name` of `size` bytes and the auto-reset event `event_name`,
    /// laid out as `layout` says, with `yaml` as the session text. The mapping starts connected,
    /// with no row written. `ErrorKind::InvalidInput` for the simulator's names or a `size`
    /// that is too small.
    pub fn create(
        map_name: &str,
        event_name: &str,
        layout: &Layout,
        yaml: &[u8],
        size: usize,
    ) -> io::Result<TestMap> {
        refuse_real_name(map_name)?;
        refuse_real_name(event_name)?;
        let plan = plan(layout, yaml.len());
        if size < plan.total {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "size {size} is below the {} bytes the layout needs",
                    plan.total
                ),
            ));
        }
        let wide_map = wide(map_name);
        let wide_event = wide(event_name);
        // SAFETY: an anonymous page-file-backed mapping (INVALID_HANDLE_VALUE) with a name
        // that is not the simulator's (checked above); the string is NUL-terminated and lives
        // through the call.
        let map = unsafe {
            CreateFileMappingW(
                INVALID_HANDLE_VALUE,
                std::ptr::null(),
                PAGE_READWRITE,
                (size as u64 >> 32) as u32,
                size as u32,
                wide_map.as_ptr(),
            )
        };
        if map == 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: `map` is the mapping handle created above; offset 0 and size 0 map all of it.
        let base = unsafe { MapViewOfFile(map, FILE_MAP_ALL_ACCESS, 0, 0, 0) };
        if base.is_null() {
            let err = io::Error::last_os_error();
            // SAFETY: `map` was created above and is closed once.
            unsafe { CloseHandle(map) };
            return Err(err);
        }
        // SAFETY: auto-reset (manual_reset 0), initially not signalled, a NUL-terminated name.
        let event = unsafe { CreateEventW(std::ptr::null(), 0, 0, wide_event.as_ptr()) };
        if event == 0 {
            let err = io::Error::last_os_error();
            // SAFETY: `base` and `map` were obtained above and are released once.
            unsafe {
                UnmapViewOfFile(base);
                CloseHandle(map);
            }
            return Err(err);
        }
        let mut me = TestMap {
            map,
            event,
            base: base.cast(),
            size,
            plan,
            num_buf: layout.num_buf(),
            buf_len: layout.buf_len(),
            var_count: layout.vars().len(),
            tick_rate: layout.tick_rate(),
            next_slot: 0,
            tick: 0,
            update: 1,
        };
        me.lay_out(layout, yaml);
        Ok(me)
    }

    /// Writes the headers, the variable headers and the text for `layout`, with every tick at 0.
    fn lay_out(&mut self, layout: &Layout, yaml: &[u8]) {
        self.plan = plan(layout, yaml.len());
        assert!(
            self.plan.total <= self.size,
            "the mapping is too small for the layout"
        );
        self.num_buf = layout.num_buf();
        self.buf_len = layout.buf_len();
        self.var_count = layout.vars().len();
        self.tick_rate = layout.tick_rate();
        self.next_slot = 0;
        self.tick = 0;
        let bufs: Vec<(i32, usize)> = self.plan.slots.iter().map(|&o| (0, o)).collect();
        let header = layout.header_bytes(
            self.update,
            (self.plan.yaml_off, yaml.len()),
            HEADER_LEN,
            &bufs,
        );
        self.write_bytes(HEADER_LEN, &layout.var_header_bytes());
        self.write_bytes(self.plan.yaml_off, yaml);
        self.write_bytes(0, &header);
    }

    fn write_bytes(&self, off: usize, bytes: &[u8]) {
        assert!(off + bytes.len() <= self.size, "write past the mapping");
        // SAFETY: the destination range lies inside the view this value owns (asserted), the
        // source is a distinct slice, and only this owner writes the mapping.
        unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), self.base.add(off), bytes.len()) };
    }

    fn store_i32(&self, off: usize, value: i32) {
        assert!(
            off.is_multiple_of(4) && off + 4 <= self.size,
            "store past the mapping"
        );
        // SAFETY: the address is inside the view (asserted), page-aligned base plus a multiple
        // of 4 is aligned for an i32, and the view stays mapped while `self` lives.
        unsafe { AtomicI32::from_ptr(self.base.add(off).cast()).store(value, Ordering::Release) };
    }

    fn load_i32(&self, off: usize) -> i32 {
        assert!(
            off.is_multiple_of(4) && off + 4 <= self.size,
            "load past the mapping"
        );
        // SAFETY: as in `store_i32`.
        unsafe { AtomicI32::from_ptr(self.base.add(off).cast()).load(Ordering::Acquire) }
    }

    fn signal(&self) {
        // SAFETY: `event` is an open event handle owned by `self`.
        unsafe { SetEvent(self.event) };
    }

    /// Sets or clears the "connected" status bit.
    pub fn set_connected(&self, on: bool) {
        let status = if on { STATUS_CONNECTED } else { 0 };
        self.store_i32(4, status);
    }

    /// Writes `row` into the next slot, then that slot's tick count, then signals the event.
    /// Ticks start at 1 and rise by `tick_rate / 60` (at least 1) per row. `row` must be the
    /// layout's row length.
    pub fn push_row(&mut self, row: &[u8]) {
        assert_eq!(
            row.len(),
            self.buf_len,
            "row length differs from the layout"
        );
        let step = (self.tick_rate / 60).max(1);
        self.tick = if self.tick == 0 { 1 } else { self.tick + step };
        let slot = self.next_slot;
        self.write_bytes(self.plan.slots[slot], row);
        self.store_i32(48 + 16 * slot, self.tick);
        self.next_slot = (slot + 1) % self.num_buf;
        self.signal();
    }

    /// Replaces the session text, then moves the update counter. The text must fit the room
    /// kept in the mapping (64 KiB, or the first text if longer).
    pub fn set_yaml(&mut self, yaml: &[u8]) {
        assert!(yaml.len() <= self.plan.yaml_room, "session text too long");
        self.write_bytes(self.plan.yaml_off, yaml);
        self.store_i32(16, yaml.len() as i32);
        self.update += 1;
        self.store_i32(12, self.update);
    }

    /// Starts a new session as the simulator does: a new layout, every tick back at 0, the
    /// update counter moved. A reader sees a reset.
    pub fn restart(&mut self, layout: &Layout, yaml: &[u8]) {
        self.update += 1;
        // Clear the old buffers so no stale row survives the new layout.
        let zeros = vec![0u8; self.size - HEADER_LEN];
        self.write_bytes(HEADER_LEN, &zeros);
        self.lay_out(layout, yaml);
        self.signal();
    }

    /// Sets the tick rate field of the header. A reader takes it at its next reset.
    pub fn set_tick_rate(&mut self, hz: i32) {
        self.tick_rate = hz;
        self.store_i32(8, hz);
    }

    /// The number of variable headers written.
    pub fn var_count(&self) -> usize {
        self.var_count
    }

    /// The tick count of the slot `slot` as written.
    pub fn slot_tick(&self, slot: usize) -> i32 {
        self.load_i32(48 + 16 * slot)
    }
}

impl Drop for TestMap {
    fn drop(&mut self) {
        // SAFETY: the view and the two handles were created in `create`, are not used after
        // this, and are released once.
        unsafe {
            UnmapViewOfFile(self.base.cast());
            CloseHandle(self.map);
            CloseHandle(self.event);
        }
    }
}

impl TestMap {
    /// The mapping size, in bytes, that holds `layout` and a session text of `yaml_len` bytes.
    pub fn required_size(layout: &Layout, yaml_len: usize) -> usize {
        plan(layout, yaml_len).total
    }
}
