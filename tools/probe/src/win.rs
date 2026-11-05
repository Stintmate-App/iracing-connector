//! The Windows calls of the probe: the Ctrl+C handler and the read-only listing of the
//! simulator's windows. Nothing here writes to another process or sends it a message.

#![allow(unsafe_code)]

use std::sync::atomic::{AtomicBool, Ordering};

type Handle = isize;

const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
const SIM_IMAGE: &str = "iracingsim64dx11.exe";

#[repr(C)]
struct Rect {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}

type EnumProc = unsafe extern "system" fn(Handle, isize) -> i32;
type CtrlHandler = unsafe extern "system" fn(u32) -> i32;

#[link(name = "user32")]
#[link(name = "kernel32")]
unsafe extern "system" {
    fn EnumWindows(proc_: Option<EnumProc>, lparam: isize) -> i32;
    fn IsWindowVisible(hwnd: Handle) -> i32;
    fn IsIconic(hwnd: Handle) -> i32;
    fn GetWindowThreadProcessId(hwnd: Handle, pid: *mut u32) -> u32;
    fn GetClassNameW(hwnd: Handle, buf: *mut u16, max: i32) -> i32;
    fn GetWindowTextW(hwnd: Handle, buf: *mut u16, max: i32) -> i32;
    fn GetClientRect(hwnd: Handle, rect: *mut Rect) -> i32;
    fn OpenProcess(access: u32, inherit: i32, pid: u32) -> Handle;
    fn QueryFullProcessImageNameW(h: Handle, flags: u32, buf: *mut u16, size: *mut u32) -> i32;
    fn CloseHandle(h: Handle) -> i32;
    fn SetConsoleCtrlHandler(handler: Option<CtrlHandler>, add: i32) -> i32;
}

/// One top-level window of the simulator.
#[derive(Clone, Debug)]
pub struct WinInfo {
    /// The window handle value.
    pub hwnd: isize,
    /// The owning process id.
    pub pid: u32,
    /// The window class name.
    pub class: String,
    /// The title.
    pub title: String,
    /// Client area width.
    pub width: i32,
    /// Client area height.
    pub height: i32,
    /// Whether the window is minimized.
    pub minimized: bool,
}

static STOP: AtomicBool = AtomicBool::new(false);

unsafe extern "system" fn on_ctrl(_kind: u32) -> i32 {
    STOP.store(true, Ordering::SeqCst);
    1
}

/// Makes Ctrl+C, Ctrl+Break and closing the console set the returned flag.
pub fn install_ctrl_c() -> &'static AtomicBool {
    // SAFETY: registers a handler that only stores to a static atomic; it stays valid for the
    // life of the process.
    unsafe { SetConsoleCtrlHandler(Some(on_ctrl), 1) };
    &STOP
}

fn text(buf: &[u16], n: i32) -> String {
    let n = usize::try_from(n).unwrap_or(0).min(buf.len());
    String::from_utf16_lossy(&buf[..n])
}

/// The file name (lower case) of the executable of process `pid`, if it can be queried.
fn image_name(pid: u32) -> Option<String> {
    // SAFETY: a query-only access right; the handle is closed below.
    let h = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if h == 0 {
        return None;
    }
    let mut buf = [0u16; 1024];
    let mut size = buf.len() as u32;
    // SAFETY: `buf` holds `size` UTF-16 units and both pointers live through the call.
    let ok = unsafe { QueryFullProcessImageNameW(h, 0, buf.as_mut_ptr(), &mut size) };
    // SAFETY: `h` was opened above and is closed once.
    unsafe { CloseHandle(h) };
    if ok == 0 {
        return None;
    }
    let path = text(&buf, size as i32);
    let name = path.rsplit(['\\', '/']).next().unwrap_or("");
    Some(name.to_ascii_lowercase())
}

unsafe extern "system" fn each_window(hwnd: Handle, lparam: isize) -> i32 {
    // SAFETY: `lparam` is the address of the vector `sim_windows` passed, alive for the whole
    // EnumWindows call, and used by this thread only.
    let out = unsafe { &mut *(lparam as *mut Vec<WinInfo>) };
    // SAFETY: plain queries on a window handle EnumWindows just gave; no message is sent.
    unsafe {
        if IsWindowVisible(hwnd) == 0 {
            return 1;
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, &mut pid);
        if image_name(pid).as_deref() != Some(SIM_IMAGE) {
            return 1;
        }
        let mut class = [0u16; 256];
        let n = GetClassNameW(hwnd, class.as_mut_ptr(), class.len() as i32);
        let class = text(&class, n);
        let mut title = [0u16; 512];
        let n = GetWindowTextW(hwnd, title.as_mut_ptr(), title.len() as i32);
        let title = text(&title, n);
        let mut rect = Rect {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        };
        GetClientRect(hwnd, &mut rect);
        out.push(WinInfo {
            hwnd,
            pid,
            class,
            title,
            width: rect.right - rect.left,
            height: rect.bottom - rect.top,
            minimized: IsIconic(hwnd) != 0,
        });
    }
    1
}

/// The visible top-level windows of the simulator's process.
pub fn sim_windows() -> Vec<WinInfo> {
    let mut out: Vec<WinInfo> = Vec::new();
    // SAFETY: the callback only reads window properties and pushes into `out`, which outlives
    // the synchronous call.
    unsafe { EnumWindows(Some(each_window), &mut out as *mut Vec<WinInfo> as isize) };
    out
}
