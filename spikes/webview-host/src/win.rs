//! Win32 probes: process-tree memory, pixels, window hit tests and guarded input injection.

use std::collections::HashSet;
use std::mem::size_of;

use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::Diagnostics::ToolHelp::*;
use windows::Win32::System::ProcessStatus::*;
use windows::Win32::System::Threading::*;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;

pub fn hwnd_of(window: &slint::Window) -> HWND {
    let handle = window.window_handle();
    match handle.window_handle().expect("window handle").as_raw() {
        RawWindowHandle::Win32(h) => HWND(h.hwnd.get() as *mut _),
        other => panic!("not a Win32 window: {other:?}"),
    }
}

#[derive(Debug, Clone, Copy)]
pub struct TreeMem {
    pub processes: usize,
    pub self_mb: f64,
    pub total_mb: f64,
    /// Processes we could not read, which would make `total_mb` an undercount.
    pub unreadable: usize,
}

/// This process and all its descendants.
fn process_tree() -> HashSet<u32> {
    let mut edges = Vec::new();
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0).expect("snapshot");
        let mut e = PROCESSENTRY32W { dwSize: size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
        if Process32FirstW(snap, &mut e).is_ok() {
            loop {
                edges.push((e.th32ParentProcessID, e.th32ProcessID));
                if Process32NextW(snap, &mut e).is_err() {
                    break;
                }
            }
        }
        let _ = CloseHandle(snap);
    }
    let mut tree: HashSet<u32> = HashSet::from([std::process::id()]);
    loop {
        let before = tree.len();
        for (parent, pid) in &edges {
            if tree.contains(parent) {
                tree.insert(*pid);
            }
        }
        if tree.len() == before {
            return tree;
        }
    }
}

/// Who owns a window: us, our webview, or some other application on the desktop. Class names
/// alone cannot say, because every Chromium-based app uses the same ones.
fn owner(hwnd: HWND) -> &'static str {
    let mut pid = 0u32;
    unsafe {
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
    }
    if pid == std::process::id() {
        "self"
    } else if process_tree().contains(&pid) {
        "webview"
    } else {
        "OTHER_APP"
    }
}

/// Private bytes of this process plus all descendants (the report's measuring rule).
pub fn tree_mem() -> TreeMem {
    let me = std::process::id();
    let tree = process_tree();
    let (mut self_b, mut total_b, mut unreadable) = (0usize, 0usize, 0usize);
    for pid in &tree {
        let Some(private) = private_bytes(*pid) else {
            unreadable += 1;
            continue;
        };
        total_b += private;
        if *pid == me {
            self_b = private;
        }
    }
    let mb = |b: usize| b as f64 / (1024.0 * 1024.0);
    TreeMem { processes: tree.len(), self_mb: mb(self_b), total_mb: mb(total_b), unreadable }
}

fn private_bytes(pid: u32) -> Option<usize> {
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut c = PROCESS_MEMORY_COUNTERS_EX { cb: size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32, ..Default::default() };
        let ok = GetProcessMemoryInfo(h, &mut c as *mut _ as *mut _, c.cb).is_ok();
        let _ = CloseHandle(h);
        ok.then_some(c.PrivateUsage)
    }
}

pub fn client_to_screen(hwnd: HWND, x: i32, y: i32) -> (i32, i32) {
    let mut p = POINT { x, y };
    unsafe {
        let _ = ClientToScreen(hwnd, &mut p);
    }
    (p.x, p.y)
}

pub fn pixel(x: i32, y: i32) -> (u8, u8, u8) {
    unsafe {
        let dc = GetDC(None);
        let c = GetPixel(dc, x, y).0;
        ReleaseDC(None, dc);
        (c as u8, (c >> 8) as u8, (c >> 16) as u8)
    }
}

/// Owner of the top window at a screen point, to see who really has those pixels.
pub fn owner_at(x: i32, y: i32) -> &'static str {
    owner(unsafe { WindowFromPoint(POINT { x, y }) })
}

/// Owner of the window that really holds keyboard focus, straight from the OS.
pub fn focus_owner() -> &'static str {
    unsafe {
        let mut info = GUITHREADINFO { cbSize: size_of::<GUITHREADINFO>() as u32, ..Default::default() };
        if GetGUIThreadInfo(0, &mut info).is_err() || info.hwndFocus.is_invalid() {
            return "none";
        }
        owner(info.hwndFocus)
    }
}

/// Screen rectangle of the first direct child window, which is wry's container.
pub fn first_child_rect(parent: HWND) -> Option<RECT> {
    unsafe {
        let child = GetWindow(parent, GW_CHILD).ok()?;
        let mut r = RECT::default();
        GetWindowRect(child, &mut r).ok()?;
        Some(r)
    }
}

pub fn bring_to_front(hwnd: HWND) -> bool {
    unsafe {
        let _ = SetForegroundWindow(hwnd);
    }
    is_foreground(hwnd)
}

pub fn is_foreground(hwnd: HWND) -> bool {
    unsafe { GetForegroundWindow() == hwnd }
}

pub fn cursor() -> (i32, i32) {
    let mut p = POINT::default();
    unsafe {
        let _ = GetCursorPos(&mut p);
    }
    (p.x, p.y)
}

pub fn move_cursor(x: i32, y: i32) {
    unsafe {
        let _ = SetCursorPos(x, y);
    }
}

fn send(inputs: &[INPUT]) {
    unsafe {
        SendInput(inputs, size_of::<INPUT>() as i32);
    }
}

/// True only if input can land on this window: foreground, not minimized.
fn safe_target(window: HWND) -> bool {
    is_foreground(window) && !unsafe { IsIconic(window) }.as_bool()
}

/// Refuses unless the point is inside our window and our process tree owns the pixels there.
/// Foreground alone is not enough: a minimized window is still foreground, its coordinates
/// are near -32000, and Windows clamps the cursor to the edge of the desktop, so the click
/// lands on whatever the user has there. Topmost windows from other apps can also cover us.
pub fn click(window: HWND, x: i32, y: i32) -> bool {
    let mut r = RECT::default();
    let inside = unsafe { GetWindowRect(window, &mut r) }.is_ok()
        && x >= r.left && x < r.right && y >= r.top && y < r.bottom;
    if !safe_target(window) || !inside || !matches!(owner_at(x, y), "self" | "webview") {
        return false;
    }
    move_cursor(x, y);
    let m = |flags| INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 { mi: MOUSEINPUT { dwFlags: flags, ..Default::default() } },
    };
    send(&[m(MOUSEEVENTF_LEFTDOWN), m(MOUSEEVENTF_LEFTUP)]);
    true
}

pub fn key(window: HWND, vk: u16) -> bool {
    if !safe_target(window) {
        return false;
    }
    let k = |flags| INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 { ki: KEYBDINPUT { wVk: VIRTUAL_KEY(vk), dwFlags: flags, ..Default::default() } },
    };
    send(&[k(KEYBD_EVENT_FLAGS(0)), k(KEYEVENTF_KEYUP)]);
    true
}

/// Writes a 32-bit BMP of the window's screen rectangle as evidence.
pub fn screenshot(hwnd: HWND, path: &str) {
    unsafe {
        let mut r = RECT::default();
        let _ = GetWindowRect(hwnd, &mut r);
        let (w, h) = (r.right - r.left, r.bottom - r.top);
        let screen = GetDC(None);
        let mem = CreateCompatibleDC(Some(screen));
        let bmp = CreateCompatibleBitmap(screen, w, h);
        let old = SelectObject(mem, bmp.into());
        let _ = BitBlt(mem, 0, 0, w, h, Some(screen), r.left, r.top, SRCCOPY);
        let mut info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w,
                biHeight: h,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut px = vec![0u8; (w * h * 4) as usize];
        GetDIBits(mem, bmp, 0, h as u32, Some(px.as_mut_ptr() as *mut _), &mut info, DIB_RGB_COLORS);
        SelectObject(mem, old);
        let _ = DeleteObject(bmp.into());
        let _ = DeleteDC(mem);
        ReleaseDC(None, screen);

        let mut out = Vec::with_capacity(54 + px.len());
        out.extend_from_slice(b"BM");
        out.extend_from_slice(&(54 + px.len() as u32).to_le_bytes());
        out.extend_from_slice(&[0; 4]);
        out.extend_from_slice(&54u32.to_le_bytes());
        out.extend_from_slice(std::slice::from_raw_parts(&info.bmiHeader as *const _ as *const u8, 40));
        out.extend_from_slice(&px);
        let _ = std::fs::write(path, out);
    }
}
