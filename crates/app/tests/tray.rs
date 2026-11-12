//! The real executable, from the outside: what closing its window does, what starting it again
//! does, and what its tray icon does. Everything goes through window messages to the app itself,
//! the ones Windows would send, so no input reaches the desktop. Links reach it through Windows'
//! own association, as from another app. Opens a window, so plain `cargo test` leaves it out. Run
//! with `scripts/test-imap.ps1`.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus};
use std::time::{Duration, Instant};

use mailfunction::startup;
use mailfunction::tray::{CALLBACK, ICON_ID, NIN_SELECT};
use mailfunction_core::settings::Settings;
use mailfunction_core::store::{Account, AuthKind, LabelSpec, NewMessage, Role, Security, Store};
use windows::Win32::Foundation::{CloseHandle, HANDLE, HWND, LPARAM, RECT, WAIT_OBJECT_0, WPARAM};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx};
use windows::Win32::System::Threading::{
    GetExitCodeProcess, GetProcessId, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, TerminateProcess, WaitForSingleObject,
};
use windows::Win32::UI::HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext};
use windows::Win32::UI::Shell::{
    IEnumAssocHandlers, NIM_DELETE, NOTIFYICONDATAW, NOTIFYICONIDENTIFIER, SEE_MASK_CLASSNAME, SEE_MASK_NOCLOSEPROCESS,
    SHAssocEnumHandlersForProtocolByApplication, SHELLEXECUTEINFOW, Shell_NotifyIconGetRect, Shell_NotifyIconW, ShellExecuteExW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetClassNameW, GetWindowRect, GetWindowTextW, GetWindowThreadProcessId, IsIconic, IsWindowVisible, PostMessageW,
    RegisterWindowMessageW, SW_SHOWNORMAL, WM_CLOSE, WM_CONTEXTMENU, WM_KEYDOWN,
};
use windows::core::{BOOL, HSTRING, PCWSTR, PWSTR, w};

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

#[link(name = "kernel32")]
unsafe extern "system" {
    fn QueryProcessCycleTime(process: HANDLE, cycles: *mut u64) -> i32;
}

fn cycles(pid: u32) -> u64 {
    let mut cycles = 0;
    unsafe {
        let process: HANDLE = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).unwrap();
        assert_ne!(QueryProcessCycleTime(process, &raw mut cycles), 0);
        CloseHandle(process).unwrap();
    }
    cycles
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

const RUN_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";
/// What the app declares to be offered for `mailto:` links.
const LINK_KEYS: [&str; 2] = [r"HKCU\Software\Classes\mailfunction.mailto", r"HKCU\Software\mailfunction"];
const REGISTERED_APPS: &str = r"HKCU\Software\RegisteredApplications";
/// Asks for more than the composer takes, which it should leave out.
const LINK: &str =
    "mailto:ann@example.com?subject=From%20another%20app&body=Written%20from%20a%20link&bcc=evil@x.example&attach=C:%5Csecret.txt";

fn start(dir: &Path) -> Child {
    Command::new(env!("CARGO_BIN_EXE_mailfunction")).env("MAILFUNCTION_DATA_DIR", dir).spawn().unwrap()
}

/// A second process for a scenario, which goes with it however the scenario ends. Left behind, it
/// would answer for the app in whatever runs next.
struct Running(Child);

impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// A value in the registry, or with no name the key's own.
fn reg_value(key: &str, name: Option<&str>) -> Option<String> {
    let which = name.map_or(vec!["/ve"], |name| vec!["/v", name]);
    let output = Command::new("reg").args(["query", key]).args(which).output().unwrap();
    let text = String::from_utf8_lossy(&output.stdout).into_owned();
    output.status.success().then(|| text.split("REG_SZ").nth(1).unwrap().trim().to_string())
}

fn set_reg_value(key: &str, name: &str, value: Option<&str>) {
    let mut command = Command::new("reg");
    match value {
        Some(value) => command.args(["add", key, "/v", name, "/d", value, "/f"]),
        None => command.args(["delete", key, "/v", name, "/f"]),
    };
    let _ = command.output();
}

/// What Windows would start at login, as the app wrote it.
fn run_entry() -> Option<String> {
    reg_value(RUN_KEY, Some("mailfunction"))
}

fn set_run_entry(value: Option<&str>) {
    set_reg_value(RUN_KEY, "mailfunction", value);
}

/// The copy under test offers itself for `mailto:` links as it starts, over the user's own copy
/// of the app. This puts back whatever was there, however the test ends.
struct LinkRegistration {
    keys: Vec<(&'static str, Option<PathBuf>)>,
    registered: Option<String>,
}

impl LinkRegistration {
    fn save(dir: &Path) -> Self {
        let keys = LINK_KEYS
            .iter()
            .enumerate()
            .map(|(n, &key)| {
                let file = dir.join(format!("saved-{n}.reg"));
                let saved = Command::new("reg").args(["export", key]).arg(&file).arg("/y").output().unwrap();
                (key, saved.status.success().then_some(file))
            })
            .collect();
        Self { keys, registered: reg_value(REGISTERED_APPS, Some("mailfunction")) }
    }
}

impl Drop for LinkRegistration {
    fn drop(&mut self) {
        for (key, saved) in &self.keys {
            let _ = Command::new("reg").args(["delete", key, "/f"]).output();
            if let Some(file) = saved {
                let _ = Command::new("reg").arg("import").arg(file).output();
            }
        }
        set_reg_value(REGISTERED_APPS, "mailfunction", self.registered.as_deref());
    }
}

/// The apps Windows offers for `mailto:` links, by the name it shows and the program it runs.
fn offered_for_links() -> Vec<(String, String)> {
    let handlers: IEnumAssocHandlers = unsafe { SHAssocEnumHandlersForProtocolByApplication(w!("mailto")) }.unwrap();
    let text = |read: windows::core::Result<PWSTR>| read.map(|text| unsafe { text.to_string() }.unwrap()).unwrap_or_default();
    let mut offered = Vec::new();
    loop {
        let mut next = [None];
        let mut fetched = 0;
        if unsafe { handlers.Next(&mut next, Some(&raw mut fetched)) }.is_err() || fetched == 0 {
            return offered;
        }
        let handler = next[0].take().unwrap();
        offered.push((text(unsafe { handler.GetUIName() }), text(unsafe { handler.GetName() })));
    }
}

/// Opens a link as Windows does once the user has chosen the app for it, through what the app
/// declared, and returns the process Windows started for it.
fn open_as_windows_would(link: &str) -> HANDLE {
    let (verb, file, class) = (HSTRING::from("open"), HSTRING::from(link), HSTRING::from("mailfunction.mailto"));
    let mut info = SHELLEXECUTEINFOW {
        cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_CLASSNAME | SEE_MASK_NOCLOSEPROCESS,
        lpVerb: PCWSTR(verb.as_ptr()),
        lpFile: PCWSTR(file.as_ptr()),
        lpClass: PCWSTR(class.as_ptr()),
        nShow: SW_SHOWNORMAL.0,
        ..Default::default()
    };
    unsafe { ShellExecuteExW(&raw mut info) }.unwrap();
    assert!(!info.hProcess.is_invalid(), "Windows started nothing");
    info.hProcess
}

fn exit_code(process: HANDLE) -> u32 {
    assert_eq!(unsafe { WaitForSingleObject(process, PATIENCE.as_millis() as u32) }, WAIT_OBJECT_0, "it kept running");
    let mut code = 0;
    unsafe {
        GetExitCodeProcess(process, &raw mut code).unwrap();
        CloseHandle(process).unwrap();
    }
    code
}

/// Windows offers the app for `mailto:` links, and one comes up as a message over a closed window.
fn link_while_running(window: HWND) {
    println!("a mailto: link, through Windows, with the window closed");
    let exe = std::path::absolute(env!("CARGO_BIN_EXE_mailfunction")).unwrap().display().to_string();
    let command = reg_value(r"HKCU\Software\Classes\mailfunction.mailto\shell\open\command", None);
    assert_eq!(command, Some(format!(r#""{exe}" "%1""#)));
    let offered = offered_for_links();
    println!("  offered: {offered:?}");
    assert!(
        offered.iter().any(|(name, program)| name == "mailfunction" && program.eq_ignore_ascii_case(&exe)),
        "Windows doesn't offer the app for mailto: links"
    );
    post(window, WM_CLOSE, 0, 0);
    until("window gone", || !showing(window));
    assert_eq!(exit_code(open_as_windows_would(LINK)), 0, "the link's own process should pass it on and end");
    until("window back", || showing(window));
    std::thread::sleep(Duration::from_millis(1500));
    capture(window, "mailto-running");
}

/// A link starts the app when it isn't running, and comes up as a message.
fn link_from_cold() {
    println!("a mailto: link, through Windows, with the app not running");
    let process = open_as_windows_would(LINK);
    let pid = unsafe { GetProcessId(process) };
    let mut found = None;
    until("a new window", || {
        found = main_window(pid).filter(|hwnd| showing(*hwnd));
        found.is_some()
    });
    std::thread::sleep(Duration::from_millis(1500));
    capture(found.unwrap(), "mailto-cold");
    unsafe {
        TerminateProcess(process, 0).unwrap();
        WaitForSingleObject(process, PATIENCE.as_millis() as u32);
        CloseHandle(process).unwrap();
    }
}

/// Started the way Windows starts it at login, the window must never reach the screen. It would
/// be there for a moment at most, so this watches as closely as it can.
fn in_the_background(dir: &Path) {
    println!("starting in the background");
    let app = Command::new(env!("CARGO_BIN_EXE_mailfunction")).arg("--background").env("MAILFUNCTION_DATA_DIR", dir).spawn().unwrap();
    let app = Running(app);
    let pid = app.0.id();
    let (started, mut found, mut seen) = (Instant::now(), None, false);
    while started.elapsed() < Duration::from_secs(4) {
        found = main_window(pid).or(found);
        seen |= found.is_some_and(|hwnd| showing(hwnd) && !unsafe { IsIconic(hwnd) }.as_bool());
    }
    let window = found.expect("a window to own the icon");
    assert!(!seen, "the window reached the screen");
    assert!(!showing(window));
    assert!(has_icon(window), "no icon to find the app by");

    println!("  and coming up from there");
    start_again(dir);
    until("window", || showing(window));
    assert!(!unsafe { IsIconic(window) }.as_bool(), "it came back minimised");
    std::thread::sleep(Duration::from_millis(600));
    capture(window, "background-revealed");
}

/// The settings that only mean something to Windows.
fn with_settings(dir: &Path) {
    println!("settings: quitting when closed, and starting with Windows");
    let saved = run_entry();
    set_run_entry(None);
    let store = Store::open(dir).unwrap();
    store.save_settings(&Settings { keep_running: false, start_with_windows: true, ..Settings::default() }).unwrap();
    drop(store);
    let mut app = Running(start(dir));
    let pid = app.0.id();
    until("window", || main_window(pid).is_some_and(showing));
    let exe = std::path::absolute(env!("CARGO_BIN_EXE_mailfunction")).unwrap();
    let entry = run_entry();
    startup::set(false);
    let turned_off = run_entry();
    set_run_entry(saved.as_deref());
    assert_eq!(entry, Some(format!(r#""{}" --background"#, exe.display())));
    assert_eq!(turned_off, None);
    post(main_window(pid).unwrap(), WM_CLOSE, 0, 0);
    assert!(exits(&mut app.0).success(), "closing the window should have ended the app");
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
        smtp_host: "localhost".into(),
        smtp_port: 25,
    };
    let account = store.add_account(&account).unwrap();
    let inbox = store.folder(account, "INBOX", &LabelSpec::system(Role::Inbox)).unwrap();
    let mail = |uid: u32| NewMessage {
        uid,
        remote_id: format!("tray-{uid}"),
        gm_thread_id: None,
        mid: None,
        references: vec![],
        subject: "Waiting".into(),
        from_name: "Ann".into(),
        from_addr: "ann@example.com".into(),
        verified: true,
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

    // The account here cannot connect, so its status keeps changing behind the closed window. A
    // thread asking for redraws spends 15 million cycles a second, and one that is idle next to
    // none. Counted in cycles because Windows samples CPU time, and can miss this much altogether.
    let before = cycles(pid);
    std::thread::sleep(Duration::from_secs(10));
    let spent = (cycles(pid) - before) / 1_000_000;
    println!("  {spent} million cycles in 10 s with the window closed");
    assert!(spent < 30, "the closed app keeps working: {spent} million cycles");

    println!("starting it with the window closed");
    start_again(dir);
    until("window back", || showing(window));

    link_while_running(window);

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

    link_from_cold();
}

fn main() {
    if std::env::args().any(|a| a == "--list") {
        return;
    }
    // Otherwise windows report scaled positions, which name the wrong pixels on screen.
    unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) }.unwrap();
    let dir: PathBuf = std::env::temp_dir().join(format!("mailfunction-tray-{}", std::process::id()));
    // What Windows starts for a link inherits this process's environment, and without this would
    // open the user's own mail.
    unsafe { std::env::set_var("MAILFUNCTION_DATA_DIR", &dir) };
    unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }.unwrap();
    seed(&dir);
    let registration = LinkRegistration::save(&dir);
    let mut app = start(&dir);
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        scenario(&dir, &mut app);
        in_the_background(&dir);
        with_settings(&dir);
    }));
    let _ = app.kill();
    let _ = app.wait();
    drop(registration);
    let _ = std::fs::remove_dir_all(&dir);
    if let Err(failure) = result {
        std::panic::resume_unwind(failure);
    }
    println!("all tray checks passed");
}
