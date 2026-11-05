//! The Windows calls that open the simulator's memory map and event, read-only.
//!
//! The map is opened with `FILE_MAP_READ` only and the event with `SYNCHRONIZE` only. No slice is
//! ever formed over the view: the simulator writes it while it is read, so the bytes leave it
//! through atomic loads (the tick counts) and raw copies (the rows).

#![allow(unsafe_code)]

use std::ffi::c_void;
use std::sync::atomic::{AtomicI32, Ordering, fence};
use std::time::Duration;

use crate::view::View;

type Handle = isize;

const FILE_MAP_READ: u32 = 0x0004;
const SYNCHRONIZE: u32 = 0x0010_0000;
const WAIT_OBJECT_0: u32 = 0;

#[repr(C)]
struct MemoryBasicInformation {
    base_address: *mut c_void,
    allocation_base: *mut c_void,
    allocation_protect: u32,
    partition_id: u16,
    region_size: usize,
    state: u32,
    protect: u32,
    kind: u32,
}

unsafe extern "system" {
    fn OpenFileMappingW(access: u32, inherit: i32, name: *const u16) -> Handle;
    fn MapViewOfFile(map: Handle, access: u32, hi: u32, lo: u32, bytes: usize) -> *mut c_void;
    fn UnmapViewOfFile(base: *const c_void) -> i32;
    fn VirtualQuery(addr: *const c_void, info: *mut MemoryBasicInformation, len: usize) -> usize;
    fn OpenEventW(access: u32, inherit: i32, name: *const u16) -> Handle;
    fn WaitForSingleObject(handle: Handle, millis: u32) -> u32;
    fn CloseHandle(handle: Handle) -> i32;
    fn GetLastError() -> u32;
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

fn last_error() -> u32 {
    // SAFETY: GetLastError has no preconditions; it reads the calling thread's error code.
    unsafe { GetLastError() }
}

/// Whether a mapping called `name` exists. Nothing is mapped or read.
pub fn mapping_exists(name: &str) -> bool {
    let name = wide(name);
    // SAFETY: `name` is a NUL-terminated UTF-16 string that outlives the call.
    let h = unsafe { OpenFileMappingW(FILE_MAP_READ, 0, name.as_ptr()) };
    if h == 0 {
        return false;
    }
    // SAFETY: `h` is a handle this function opened and closes once.
    unsafe { CloseHandle(h) };
    true
}

/// A read-only view of a named file mapping. Dropping it unmaps the view and closes the handle.
#[derive(Debug)]
pub struct MapView {
    handle: Handle,
    base: *mut u8,
    len: usize,
}

// SAFETY: the view is only read, through atomic loads and raw copies; the handle and the base
// address are plain values that any thread may use until the drop.
unsafe impl Send for MapView {}
// SAFETY: as above; no method takes `&self` and writes.
unsafe impl Sync for MapView {}

impl MapView {
    /// Opens the mapping `name` for reading and maps all of it. The error is the Windows error
    /// code (2, file not found, when the mapping does not exist).
    pub fn open(name: &str) -> Result<MapView, u32> {
        let name = wide(name);
        // SAFETY: `name` is a NUL-terminated UTF-16 string that outlives the call; the access
        // is read-only.
        let handle = unsafe { OpenFileMappingW(FILE_MAP_READ, 0, name.as_ptr()) };
        if handle == 0 {
            return Err(last_error());
        }
        // SAFETY: `handle` is a mapping handle opened with FILE_MAP_READ; offset 0 and size 0
        // map the whole object, read-only.
        let base = unsafe { MapViewOfFile(handle, FILE_MAP_READ, 0, 0, 0) };
        if base.is_null() {
            let err = last_error();
            // SAFETY: `handle` was opened above and is closed once.
            unsafe { CloseHandle(handle) };
            return Err(err);
        }
        let mut info = MemoryBasicInformation {
            base_address: std::ptr::null_mut(),
            allocation_base: std::ptr::null_mut(),
            allocation_protect: 0,
            partition_id: 0,
            region_size: 0,
            state: 0,
            protect: 0,
            kind: 0,
        };
        // SAFETY: `base` is the start of the view just mapped and `info` is a writable
        // structure of the size passed.
        let got = unsafe { VirtualQuery(base, &mut info, size_of::<MemoryBasicInformation>()) };
        if got == 0 || info.region_size == 0 {
            let err = last_error();
            // SAFETY: `base` came from MapViewOfFile and `handle` from OpenFileMappingW; each
            // is released once.
            unsafe {
                UnmapViewOfFile(base);
                CloseHandle(handle);
            }
            return Err(err);
        }
        Ok(MapView {
            handle,
            base: base.cast(),
            len: info.region_size,
        })
    }
}

impl View for MapView {
    fn len(&self) -> usize {
        self.len
    }

    fn load_i32(&self, off: usize) -> Option<i32> {
        if !off.is_multiple_of(4) || off.checked_add(4)? > self.len {
            return None;
        }
        // SAFETY: `off + 4 <= len`, so the address is inside the view; the base is page
        // aligned and `off` is a multiple of 4, so the pointer is aligned for an i32. The
        // atomic load is a plain read of memory the simulator writes concurrently.
        let value =
            unsafe { AtomicI32::from_ptr(self.base.add(off).cast()).load(Ordering::Acquire) };
        Some(i32::from_le(value))
    }

    fn copy_out(&self, off: usize, dst: &mut [u8]) -> bool {
        match off.checked_add(dst.len()) {
            Some(end) if end <= self.len => {}
            _ => return false,
        }
        // SAFETY: the source range lies inside the view (checked above), `dst` is a distinct
        // writable slice of the same length, and the two cannot overlap. The simulator may
        // write the source during the copy; the caller checks the tick count afterwards and
        // discards a copy that changed.
        unsafe { std::ptr::copy_nonoverlapping(self.base.add(off), dst.as_mut_ptr(), dst.len()) };
        fence(Ordering::Acquire);
        true
    }
}

impl Drop for MapView {
    fn drop(&mut self) {
        // SAFETY: `base` and `handle` were obtained in `open`, are not used after this, and are
        // released once.
        unsafe {
            UnmapViewOfFile(self.base.cast());
            CloseHandle(self.handle);
        }
    }
}

/// A named event opened with `SYNCHRONIZE` access: it can be waited on and nothing else.
#[derive(Debug)]
pub struct Event {
    handle: Handle,
}

impl Event {
    /// Opens the event `name`. The error is the Windows error code.
    pub fn open(name: &str) -> Result<Event, u32> {
        let name = wide(name);
        // SAFETY: `name` is a NUL-terminated UTF-16 string that outlives the call; the access
        // right only allows waiting.
        let handle = unsafe { OpenEventW(SYNCHRONIZE, 0, name.as_ptr()) };
        if handle == 0 {
            return Err(last_error());
        }
        Ok(Event { handle })
    }

    /// Waits for the event for at most `timeout`; true when it was signalled.
    pub fn wait(&self, timeout: Duration) -> bool {
        // 0xFFFF_FFFF means "forever": stay below it.
        let millis = u32::try_from(timeout.as_millis()).unwrap_or(u32::MAX - 1);
        // SAFETY: `handle` is an open event handle owned by `self`.
        unsafe { WaitForSingleObject(self.handle, millis) == WAIT_OBJECT_0 }
    }
}

impl Drop for Event {
    fn drop(&mut self) {
        // SAFETY: `handle` was opened in `open` and is closed once.
        unsafe { CloseHandle(self.handle) };
    }
}
