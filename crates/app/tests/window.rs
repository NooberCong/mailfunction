//! The real window and the real save dialog: where the window opens, what it remembers, and what a
//! file saved under a new name is called. Everything is done to the test's own windows, so nothing
//! reaches the desktop, and no servers are needed. Run with `cargo test -p mailfunction --test window`.

mod common;

use std::path::{Path, PathBuf};
use std::process::Command;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use common::RecordingNotifier;
use common::server::{sleep, until};
use mailfunction::editor::EditorView;
use mailfunction::webview::Webview;
use mailfunction::{App, Controller, save_dialog};
use mailfunction_core::remote::Offline;
use mailfunction_core::settings::Placement;
use mailfunction_core::store::{Reader, Store};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use slint::ComponentHandle;
use windows::Win32::Foundation::{HWND, LPARAM, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow};
use windows::Win32::System::Threading::GetCurrentProcessId;
use windows::Win32::UI::WindowsAndMessaging::{
    EnumChildWindows, EnumWindows, GetClassNameW, GetDlgCtrlID, GetWindowRect, GetWindowThreadProcessId, IDOK,
    IsWindowVisible, IsZoomed, PostMessageW, SHOW_WINDOW_CMD, SW_MAXIMIZE, SW_MINIMIZE, SW_RESTORE, SWP_NOACTIVATE, SWP_NOZORDER,
    SendMessageW, SetWindowPos, ShowWindow, WM_COMMAND, WM_SETTEXT,
};
use windows::core::{BOOL, HSTRING};

/// Tells the test to be the process that opens the app with nothing remembered.
const FIRST_OPENING: &str = "--first-opening";
/// The dialog's box for the file's name, whatever the version of Windows around it.
const NAME_BOX: i32 = 1001;
/// Longer than the app waits for a window to come to rest.
const AT_REST: u64 = 900;

const LEFT: Placement = Placement { left: 120, top: 70, width: 1700, height: 1000, maximized: true };

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("mailfunction-window-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn window(app: &App) -> HWND {
    let handle = app.window().window_handle();
    let RawWindowHandle::Win32(handle) = handle.window_handle().unwrap().as_raw() else { panic!("not Win32") };
    HWND(handle.hwnd.get() as *mut _)
}

fn frame(hwnd: HWND) -> RECT {
    let mut rect = RECT::default();
    unsafe { GetWindowRect(hwnd, &raw mut rect) }.unwrap();
    rect
}

fn as_rect(placement: Placement) -> RECT {
    RECT {
        left: placement.left,
        top: placement.top,
        right: placement.left + placement.width,
        bottom: placement.top + placement.height,
    }
}

fn is_zoomed(hwnd: HWND) -> bool {
    unsafe { IsZoomed(hwnd) }.as_bool()
}

fn show(hwnd: HWND, how: SHOW_WINDOW_CMD) {
    let _ = unsafe { ShowWindow(hwnd, how) };
}

/// Opens the app as `main` does.
fn open(dir: &Path, drive: impl AsyncFnOnce(App, Rc<Controller>) + 'static) {
    let app = App::new().unwrap();
    let pane = Rc::new(Webview::new(dir.join("webview")));
    let editor = Rc::new(EditorView::new(dir.join("webview")));
    let notifier = Box::new(RecordingNotifier::default());
    let controller = Controller::new(app.clone_strong(), dir, None, Box::new(pane), Box::new(editor), notifier, Arc::new(Offline)).unwrap();
    controller.load_accounts().unwrap();
    controller.place_window();
    mailfunction::chrome::watch_window(&app);
    app.show().unwrap();
    let driver = app.clone_strong();
    slint::spawn_local(async move {
        drive(driver, controller).await;
        slint::quit_event_loop().unwrap();
    })
    .unwrap();
    slint::run_event_loop().unwrap();
}

/// With nothing remembered, all of the window is on the screen.
fn first_opening() {
    let dir = scratch("first");
    open(&dir, async |app, _| {
        until("the window on screen", || unsafe { IsWindowVisible(window(&app)) }.as_bool()).await;
        let hwnd = window(&app);
        let mut monitor = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
        unsafe { GetMonitorInfoW(MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST), &raw mut monitor) }.unwrap();
        let (own, work) = (frame(hwnd), monitor.rcWork);
        println!("first opening: {own:?} on {work:?}");
        assert!(own.left >= work.left && own.top >= work.top, "{own:?} on {work:?}");
        assert!(own.right <= work.right && own.bottom <= work.bottom, "{own:?} on {work:?}");
        assert!(!is_zoomed(hwnd));
    });
    let _ = std::fs::remove_dir_all(&dir);
}

/// The window comes back as it was left, and leaves word of each thing done to it.
fn remembering() {
    let dir = scratch("left");
    let store = Store::open(&dir).unwrap();
    store.save_placement(&LEFT).unwrap();
    let reader = Reader::open(&dir).unwrap();
    open(&dir, async move |app, _| {
        until("the window on screen", || unsafe { IsWindowVisible(window(&app)) }.as_bool()).await;
        let hwnd = window(&app);
        sleep(300).await;
        assert!(is_zoomed(hwnd), "maximized when left, so maximized now");

        show(hwnd, SW_RESTORE);
        sleep(300).await;
        assert_eq!(frame(hwnd), as_rect(LEFT), "coming back from maximized lands where it was left");

        unsafe { SetWindowPos(hwnd, None, 200, 110, 1600, 950, SWP_NOZORDER | SWP_NOACTIVATE) }.unwrap();
        sleep(AT_REST).await;
        let moved = Placement { left: 200, top: 110, width: 1600, height: 950, maximized: false };
        assert_eq!(reader.placement(), Some(moved), "a move is remembered");

        show(hwnd, SW_MINIMIZE);
        sleep(AT_REST).await;
        assert_eq!(reader.placement(), Some(moved), "nobody wants it to open minimized");

        show(hwnd, SW_RESTORE);
        sleep(300).await;
        show(hwnd, SW_MAXIMIZE);
        sleep(AT_REST).await;
        assert_eq!(reader.placement(), Some(Placement { maximized: true, ..moved }), "the size to come back to stays");
    });
    let _ = std::fs::remove_dir_all(&dir);
}

unsafe extern "system" fn find_dialog(hwnd: HWND, found: LPARAM) -> BOOL {
    let mut process = 0;
    let mut class = [0; 16];
    unsafe {
        GetWindowThreadProcessId(hwnd, Some(&raw mut process));
        let length = GetClassNameW(hwnd, &mut class) as usize;
        if process == GetCurrentProcessId() && String::from_utf16_lossy(&class[..length]) == "#32770" {
            *(found.0 as *mut Option<HWND>) = Some(hwnd);
            return false.into();
        }
    }
    true.into()
}

unsafe extern "system" fn find_name_box(hwnd: HWND, found: LPARAM) -> BOOL {
    unsafe {
        if GetDlgCtrlID(hwnd) == NAME_BOX {
            let mut class = [0; 8];
            let length = GetClassNameW(hwnd, &mut class) as usize;
            if String::from_utf16_lossy(&class[..length]) == "Edit" {
                *(found.0 as *mut Option<HWND>) = Some(hwnd);
                return false.into();
            }
        }
    }
    true.into()
}

/// What the dialog answers for a file of one name when another is typed into it.
fn saved_as(dir: &Path, name: &str, typed: &str) -> PathBuf {
    let (name, folder) = (name.to_string(), dir.to_path_buf());
    let chosen = std::thread::spawn(move || save_dialog(&name).set_directory(folder).save_file());
    let started = Instant::now();
    let (mut dialog, mut name_box) = (None::<HWND>, None::<HWND>);
    while name_box.is_none() {
        assert!(started.elapsed() < Duration::from_secs(20), "the dialog never came");
        std::thread::sleep(Duration::from_millis(100));
        unsafe {
            let _ = EnumWindows(Some(find_dialog), LPARAM(&raw mut dialog as isize));
            if let Some(dialog) = dialog {
                let _ = EnumChildWindows(Some(dialog), Some(find_name_box), LPARAM(&raw mut name_box as isize));
            }
        }
    }
    let typed = HSTRING::from(typed);
    unsafe {
        SendMessageW(name_box.unwrap(), WM_SETTEXT, None, Some(LPARAM(typed.as_ptr() as isize)));
        PostMessageW(dialog, WM_COMMAND, WPARAM(IDOK.0 as usize), LPARAM(0)).unwrap();
    }
    chosen.join().unwrap().expect("the dialog gave a path")
}

/// A file keeps its extension when it is given a new name that leaves it out.
fn saving_under_a_new_name() {
    let dir = scratch("save");
    let file_name = |path: PathBuf| path.file_name().unwrap().to_string_lossy().into_owned();
    assert_eq!(file_name(saved_as(&dir, "20261104_실시간.xlsx", "분기 보고서")), "분기 보고서.xlsx");
    assert_eq!(file_name(saved_as(&dir, "notes.pdf", "notes v1.2")), "notes v1.2.pdf");
    assert_eq!(file_name(saved_as(&dir, "notes.pdf", "other.txt")), "other.txt", "one typed on purpose stands");
    assert_eq!(file_name(saved_as(&dir, "Attachment", "plain")), "plain");
    let _ = std::fs::remove_dir_all(&dir);
}

fn main() {
    if std::env::args().any(|argument| argument == FIRST_OPENING) {
        first_opening();
        std::process::exit(0);
    }
    saving_under_a_new_name();
    println!("the save dialog keeps extensions");
    // A process gets one event loop, so the other opening needs a process of its own.
    let exe = std::env::current_exe().unwrap();
    assert!(Command::new(exe).arg(FIRST_OPENING).status().unwrap().success());
    remembering();
    println!("all window checks passed");
    std::process::exit(0);
}
