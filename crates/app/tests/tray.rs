//! The real executable, from the outside: what closing its window does, what starting it again
//! does, and what its tray icon does. Everything goes through window messages to the app itself,
//! the ones Windows would send, so no input reaches the desktop. Opens a window, so plain
//! `cargo test` leaves it out. Run with `scripts/test-imap.ps1`.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus};
use std::time::{Duration, Instant};

use mailfunction::tray::{CALLBACK, ICON_ID, NIN_SELECT};
use mailfunction_core::store::{Account, AuthKind, LabelSpec, NewMessage, Role, Security, Store};
use windows::Win32::Foundation::{FILETIME, HANDLE, HWND, LPARAM, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::Threading::{GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
use windows::Win32::UI::HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext};
use windows::Win32::UI::Shell::{NIM_DELETE, NOTIFYICONDATAW, NOTIFYICONIDENTIFIER, Shell_NotifyIconGetRect, Shell_NotifyIconW};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetClassNameW, GetWindowRect, GetWindowTextW, GetWindowThreadProcessId, IsWindowVisible, PostMessageW,
    RegisterWindowMessageW, WM_CLOSE, WM_CONTEXTMENU, WM_KEYDOWN,
};
use windows::core::{BOOL, w};

const VK_DOWN: usize = 0x28;
const VK_ESCAPE: usize = 0x1B;
const VK_RETURN: usize = 0x0D;
const PATIENCE: Duration = Duration::from_secs(20);
/// Where the menu is asked to appear, clear of the taskbar.
const MENU_AT: (i16, i16) = (420, 420);

fn until(what: &str, mut ready: impl FnMut() -> bool) {
    let started = Instant::now();
    while !ready() {
        assert!(started.elapsed() < PATIENCE, "timed out waiting for: {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
    println!("  {what}: {} ms", started.elapsed().as_millis());
}

/// The process's top-level windows, with their class names and titles.
fn windows_of(pid: u32) -> Vec<(HWND, String, String)> {
    unsafe extern "system" fn add(hwnd: HWND, found: LPARAM) -> BOOL {
        unsafe { (*(found.0 as *mut Vec<HWND>)).push(hwnd) };
        true.into()
    }
    let mut all: Vec<HWND> = Vec::new();
    unsafe { EnumWindows(Some(add), LPARAM(&raw mut all as isize)) }.unwrap();
    let text = |hwnd, read: unsafe fn(HWND, &mut [u16]) -> i32| {
        let mut buffer = [0; 128];
        let length = unsafe { read(hwnd, &mut buffer) };
        String::from_utf16_lossy(&buffer[..length as usize])
    };
    let owned_by = |&hwnd: &HWND| {
        let mut owner = 0;
        unsafe { GetWindowThreadProcessId(hwnd, Some(&raw mut owner)) };
        owner == pid
    };
    all.into_iter()
        .filter(owned_by)
        .map(|hwnd| (hwnd, text(hwnd, |h, b| unsafe { GetClassNameW(h, b) }), text(hwnd, |h, b| unsafe { GetWindowTextW(h, b) })))
        .collect()
}

fn main_window(pid: u32) -> Option<HWND> {
    windows_of(pid).into_iter().find(|(_, _, title)| title == "mailfunction").map(|(hwnd, ..)| hwnd)
}

fn menu_of(pid: u32) -> Option<HWND> {
    let visible = |hwnd: &HWND| unsafe { IsWindowVisible(*hwnd) }.as_bool();
    windows_of(pid).into_iter().find(|(_, class, _)| class == "#32768").map(|(hwnd, ..)| hwnd).filter(visible)
}

fn showing(hwnd: HWND) -> bool {
    unsafe { IsWindowVisible(hwnd) }.as_bool()
}

fn post(hwnd: HWND, message: u32, wparam: usize, lparam: isize) {
    unsafe { PostMessageW(Some(hwnd), message, WPARAM(wparam), LPARAM(lparam)) }.unwrap();
}

/// Whether Windows holds the icon.
fn has_icon(hwnd: HWND) -> bool {
    let icon = NOTIFYICONIDENTIFIER {
        cbSize: size_of::<NOTIFYICONIDENTIFIER>() as u32,
        hWnd: hwnd,
        uID: ICON_ID,
        ..Default::default()
    };
    unsafe { Shell_NotifyIconGetRect(&raw const icon) }.is_ok()
}

fn cpu_time(pid: u32) -> Duration {
    let ticks = |time: FILETIME| u64::from(time.dwHighDateTime) << 32 | u64::from(time.dwLowDateTime);
    let (mut start, mut end, mut kernel, mut user) = Default::default();
    unsafe {
        let process: HANDLE = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).unwrap();
        GetProcessTimes(process, &raw mut start, &raw mut end, &raw mut kernel, &raw mut user).unwrap();
    }
    Duration::from_nanos((ticks(kernel) + ticks(user)) * 100)
}

/// Saves the pixels of a window on screen, and only those: the rest of the desktop is private.
fn capture(hwnd: HWND, name: &str) {
    let mut rect = RECT::default();
    unsafe { GetWindowRect(hwnd, &raw mut rect) }.unwrap();
    let (width, height) = (rect.right - rect.left, rect.bottom - rect.top);
    let mut bgra = vec![0u8; (width * height * 4) as usize];
    unsafe {
        let screen = GetDC(None);
        let memory = CreateCompatibleDC(Some(screen));
        let bitmap = CreateCompatibleBitmap(screen, width, height);
        let previous = SelectObject(memory, bitmap.into());
        BitBlt(memory, 0, 0, width, height, Some(screen), rect.left, rect.top, SRCCOPY).unwrap();
        SelectObject(memory, previous);
        let mut info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width,
                biHeight: -height,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        GetDIBits(memory, bitmap, 0, height as u32, Some(bgra.as_mut_ptr().cast()), &raw mut info, DIB_RGB_COLORS);
        let _ = DeleteObject(bitmap.into());
        let _ = DeleteDC(memory);
        ReleaseDC(None, screen);
    }
    for pixel in bgra.chunks_exact_mut(4) {
        pixel.swap(0, 2);
        pixel[3] = 255;
    }
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/ui");
    std::fs::create_dir_all(&dir).unwrap();
    let file = std::fs::File::create(dir.join(format!("{name}.png"))).unwrap();
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), width as u32, height as u32);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.write_header().unwrap().write_image_data(&bgra).unwrap();
}

fn start(dir: &Path) -> Child {
    Command::new(env!("CARGO_BIN_EXE_mailfunction")).env("MAILFUNCTION_DATA_DIR", dir).spawn().unwrap()
}

fn exits(child: &mut Child) -> ExitStatus {
    let mut status = None;
    until("process exit", || {
        status = child.try_wait().unwrap();
        status.is_some()
    });
    status.unwrap()
}

/// Starts the app again, which should leave only the first one running.
fn start_again(dir: &Path) {
    assert!(exits(&mut start(dir)).success());
}

/// An account no server answers for, with mail waiting, so that the icon has something to show.
fn seed(dir: &Path) {
    let store = Store::open(dir).unwrap();
    let account = Account {
        id: 0,
        email: "tray@example.com".into(),
        host: "127.0.0.1".into(),
        port: 1,
        security: Security::Plain,
        auth: AuthKind::Password,
    };
    let account = store.add_account(&account).unwrap();
    let inbox = store.folder(account, "INBOX", &LabelSpec::system(Role::Inbox)).unwrap();
    let mail = |uid: u32| NewMessage {
        uid,
        remote_id: format!("tray-{uid}"),
        gm_thread_id: None,
        subject: "Waiting".into(),
        from_name: "Ann".into(),
        from_addr: "ann@example.com".into(),
        date: 1_800_000_000,
        size: 1,
        seen: false,
        flagged: false,
        labels: vec![],
    };
    store.save_messages(&inbox, &[mail(1), mail(2), mail(3)]).unwrap();
}

fn scenario(dir: &Path, app: &mut Child) {
    let pid = app.id();
    println!("starting");
    let mut found = None;
    until("window", || {
        found = main_window(pid).filter(|hwnd| showing(*hwnd));
        found.is_some()
    });
    let window = found.unwrap();
    until("tray icon", || has_icon(window));

    println!("starting it a second time");
    start_again(dir);
    assert!(app.try_wait().unwrap().is_none());

    println!("closing the window");
    post(window, WM_CLOSE, 0, 0);
    until("window gone", || !showing(window));
    std::thread::sleep(Duration::from_millis(500));
    assert!(app.try_wait().unwrap().is_none(), "closing the window ended the app");
    assert!(has_icon(window), "the icon went with the window");

    let before = cpu_time(pid);
    std::thread::sleep(Duration::from_secs(5));
    let spent = cpu_time(pid) - before;
    println!("  {} ms of CPU in 5 s with the window closed", spent.as_millis());
    assert!(spent < Duration::from_millis(250), "the closed app keeps working: {spent:?}");

    println!("starting it with the window closed");
    start_again(dir);
    until("window back", || showing(window));

    println!("clicking the icon");
    post(window, WM_CLOSE, 0, 0);
    until("window gone", || !showing(window));
    post(window, CALLBACK, 0, NIN_SELECT as isize);
    until("window back", || showing(window));

    println!("the taskbar restarting");
    let icon = NOTIFYICONDATAW { cbSize: size_of::<NOTIFYICONDATAW>() as u32, hWnd: window, uID: ICON_ID, ..Default::default() };
    assert!(unsafe { Shell_NotifyIconW(NIM_DELETE, &raw const icon) }.as_bool());
    assert!(!has_icon(window));
    post(window, unsafe { RegisterWindowMessageW(w!("TaskbarCreated")) }, 0, 0);
    until("icon back", || has_icon(window));

    let open_menu = || {
        post(window, CALLBACK, MENU_AT.0 as usize | (MENU_AT.1 as usize) << 16, WM_CONTEXTMENU as isize);
        let mut found = None;
        until("menu", || {
            found = menu_of(pid);
            found.is_some()
        });
        found.unwrap()
    };

    println!("the menu: dismissing it");
    post(window, WM_CLOSE, 0, 0);
    until("window gone", || !showing(window));
    let menu = open_menu();
    std::thread::sleep(Duration::from_millis(400)); // its fade
    capture(menu, "tray-menu");
    post(menu, WM_KEYDOWN, VK_ESCAPE, 0);
    until("menu gone", || menu_of(pid).is_none());
    assert!(!showing(window), "dismissing the menu opened the window");

    println!("the menu: the app carries on behind it");
    open_menu();
    start_again(dir);
    until("window back", || showing(window));
    if let Some(menu) = menu_of(pid) {
        post(menu, WM_KEYDOWN, VK_ESCAPE, 0);
    }
    until("menu gone", || menu_of(pid).is_none());
    post(window, WM_CLOSE, 0, 0);
    until("window gone", || !showing(window));

    println!("the menu: Open");
    let menu = open_menu();
    post(menu, WM_KEYDOWN, VK_DOWN, 0);
    post(menu, WM_KEYDOWN, VK_RETURN, 0);
    until("window back", || showing(window));

    println!("the menu: Quit");
    let menu = open_menu();
    post(menu, WM_KEYDOWN, VK_DOWN, 0);
    post(menu, WM_KEYDOWN, VK_DOWN, 0);
    post(menu, WM_KEYDOWN, VK_RETURN, 0);
    assert!(exits(app).success());
    assert!(!has_icon(window), "the icon outlived the app");

    println!("starting it after quitting");
    let mut again = start(dir);
    let pid = again.id();
    until("a new window", || main_window(pid).is_some_and(showing));
    again.kill().unwrap();
    again.wait().unwrap();
}

fn main() {
    if std::env::args().any(|a| a == "--list") {
        return;
    }
    // Otherwise windows report scaled positions, which name the wrong pixels on screen.
    unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) }.unwrap();
    let dir: PathBuf = std::env::temp_dir().join(format!("mailfunction-tray-{}", std::process::id()));
    seed(&dir);
    let mut app = start(&dir);
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| scenario(&dir, &mut app)));
    let _ = app.kill();
    let _ = app.wait();
    let _ = std::fs::remove_dir_all(&dir);
    if let Err(failure) = result {
        std::panic::resume_unwind(failure);
    }
    println!("all tray checks passed");
}
