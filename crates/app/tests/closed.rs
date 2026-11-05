//! What the app does with its window closed: nothing, whatever happens to the mail. Windows sends
//! a hidden window no paint message, so a redraw asked for there is never served, and Slint keeps
//! asking at the display's refresh rate until the window comes back. Works in the real window
//! with the real WebView2, and measures the thread itself, as a frame count would see nothing.
//! Opens a window but injects no input, and needs no servers.
//! Run with `cargo test --release -p mailfunction --test closed`.

mod common;

use std::cell::{Cell, RefCell};
use std::future::poll_fn;
use std::rc::Rc;
use std::sync::Arc;
use std::task::{Poll, Waker};
use std::time::{Duration, Instant};

use mailfunction::webview::Webview;
use mailfunction::{App, Controller, Pane, Theme};
use mailfunction_core::engine::Event;
use mailfunction_core::remote::Offline;
use mailfunction_core::settings::Settings;
use mailfunction_core::store::{Account, AuthKind, Folder, LabelSpec, NewMessage, Reader, Role, Security, Store};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use slint::platform::WindowEvent;
use slint::{ComponentHandle, Model};
use windows::Win32::Foundation::{HWND, LPARAM, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::Storage::Xps::{PRINT_WINDOW_FLAGS, PrintWindow};
use windows::Win32::UI::WindowsAndMessaging::{GetClientRect, IsWindowVisible, PostMessageW, SW_MINIMIZE, SW_RESTORE, ShowWindow, WM_CLOSE};

/// An idle thread measures about 0.2, and up to 4 with the desktop busy. One asking for redraws
/// measures 14 or more.
const IDLE_MCYCLES: f64 = 8.0;
const SETTLE: u64 = 1200;
const SAMPLE: u64 = 3000;

#[link(name = "kernel32")]
unsafe extern "system" {
    fn QueryThreadCycleTime(thread: isize, cycles: *mut u64) -> i32;
    fn GetCurrentThread() -> isize;
}

fn cycles() -> u64 {
    let mut cycles = 0;
    unsafe { QueryThreadCycleTime(GetCurrentThread(), &raw mut cycles) };
    cycles
}

fn sleep(ms: u64) -> impl Future<Output = ()> {
    let done = Rc::new(Cell::new(false));
    let waker: Rc<RefCell<Option<Waker>>> = Rc::default();
    let (flag, waiting) = (done.clone(), waker.clone());
    slint::Timer::single_shot(Duration::from_millis(ms), move || {
        flag.set(true);
        if let Some(waker) = waiting.take() {
            waker.wake();
        }
    });
    poll_fn(move |context| {
        if done.get() {
            return Poll::Ready(());
        }
        waker.replace(Some(context.waker().clone()));
        Poll::Pending
    })
}

async fn until(what: &str, mut ready: impl FnMut() -> bool) {
    let started = Instant::now();
    while !ready() {
        assert!(started.elapsed() < Duration::from_secs(20), "timed out waiting for {what}");
        sleep(5).await;
    }
}

fn window(app: &App) -> HWND {
    let handle = app.window().window_handle();
    let RawWindowHandle::Win32(handle) = handle.window_handle().unwrap().as_raw() else { panic!("not Win32") };
    HWND(handle.hwnd.get() as _)
}

fn showing(app: &App) -> bool {
    unsafe { IsWindowVisible(window(app)) }.as_bool()
}

/// The window's client area, webview included.
fn pixels(app: &App) -> Vec<u8> {
    let hwnd = window(app);
    unsafe {
        let mut client = RECT::default();
        GetClientRect(hwnd, &mut client).unwrap();
        let (w, h) = (client.right, client.bottom);
        let dc = GetDC(Some(hwnd));
        let memory = CreateCompatibleDC(Some(dc));
        let bitmap = CreateCompatibleBitmap(dc, w, h);
        let previous = SelectObject(memory, bitmap.into());
        // PW_CLIENTONLY | PW_RENDERFULLCONTENT: the second is what includes the webview.
        let _ = PrintWindow(hwnd, memory, PRINT_WINDOW_FLAGS(3));
        let mut info = BITMAPINFO::default();
        info.bmiHeader.biSize = size_of::<BITMAPINFOHEADER>() as u32;
        info.bmiHeader.biWidth = w;
        info.bmiHeader.biHeight = -h;
        info.bmiHeader.biPlanes = 1;
        info.bmiHeader.biBitCount = 32;
        let mut pixels = vec![0u8; (w * h * 4) as usize];
        GetDIBits(memory, bitmap, 0, h as u32, Some(pixels.as_mut_ptr().cast()), &mut info, DIB_RGB_COLORS);
        SelectObject(memory, previous);
        let _ = DeleteObject(bitmap.into());
        let _ = DeleteDC(memory);
        ReleaseDC(Some(hwnd), dc);
        pixels
    }
}

/// The share of the window that differs between two pictures of it.
fn changed(before: &[u8], after: &[u8]) -> f64 {
    let differ = before.chunks_exact(4).zip(after.chunks_exact(4)).filter(|(a, b)| a[..3] != b[..3]).count();
    differ as f64 / (before.len() / 4) as f64
}

fn row(app: &App, subject: &str) -> usize {
    let rows = app.get_rows();
    (0..rows.row_count()).find(|&i| rows.row_data(i).unwrap().subject == subject).unwrap()
}

fn mail(uid: u32, subject: &str) -> NewMessage {
    NewMessage {
        uid,
        remote_id: format!("m{uid}"),
        gm_thread_id: Some(u64::from(uid)),
        mid: Some(format!("m{uid}@x")),
        references: vec![],
        subject: subject.into(),
        from_name: "Brand".into(),
        from_addr: "news@brand.example".into(),
        verified: true,
        date: 1_793_000_000 - i64::from(uid) * 3600,
        size: 100,
        seen: true,
        flagged: false,
        labels: vec![],
    }
}

struct Stage {
    app: App,
    controller: Rc<Controller>,
    account: i64,
    failed: Cell<usize>,
}

impl Stage {
    /// The message Windows sends for the close button, so that it arrives as a real one does.
    async fn close(&self) {
        unsafe { PostMessageW(Some(window(&self.app)), WM_CLOSE, WPARAM(0), LPARAM(0)) }.unwrap();
        until("the window to close", || !showing(&self.app)).await;
    }

    async fn reopen(&self) {
        self.controller.reveal();
        until("the window to open", || showing(&self.app)).await;
        until("the page", || self.app.get_pane() != Pane::Loading).await;
    }

    /// What the UI thread costs for a while, in millions of cycles a second.
    async fn cost(&self) -> f64 {
        sleep(SETTLE).await;
        let (spent, started) = (cycles(), Instant::now());
        sleep(SAMPLE).await;
        (cycles() - spent) as f64 / started.elapsed().as_secs_f64() / 1e6
    }

    /// Fails the test if the thread is busy, for a window nothing on the desktop can reach.
    async fn check(&self, what: &str) {
        assert!(!showing(&self.app), "the window is open for: {what}");
        let rate = self.cost().await;
        let verdict = if rate < IDLE_MCYCLES { "ok" } else { "BUSY" };
        println!("  {verdict:<5}{rate:>6.1} Mcycles/s  {what}");
        if rate >= IDLE_MCYCLES {
            self.failed.set(self.failed.get() + 1);
        }
    }

    /// Only says what it finds, for a window the desktop can reach: the pointer passing over it
    /// costs cycles too, and anyone can bring back a minimized one.
    async fn note(&self, what: &str) {
        println!("       {:>6.1} Mcycles/s  {what}", self.cost().await);
    }

    fn event(&self, event: Event) {
        self.controller.on_event(self.account, event);
    }
}

async fn scenario(stage: &Stage, store: &Store, inbox: &Folder) {
    let (app, controller) = (&stage.app, &stage.controller);
    sleep(800).await;
    stage.note("open, nothing selected").await;
    controller.select_row(row(app, "Page"));
    until("the page", || app.get_pane() == Pane::Html).await;
    stage.note("open, showing a page").await;

    stage.close().await;
    stage.check("closed").await;

    stage.event(Event::Syncing);
    stage.check("closed, a sync under way").await;
    stage.event(Event::UpToDate);
    stage.check("closed, the sync over").await;
    stage.reopen().await;
    stage.note("reopened").await;

    let before = pixels(app);
    stage.close().await;
    store.save_messages(inbox, &[mail(50, "Arrived while closed")]).unwrap();
    stage.event(Event::Changed);
    sleep(600).await;
    stage.check("closed, new mail").await;
    stage.reopen().await;
    sleep(600).await;
    let seen = changed(&before, &pixels(app));
    println!("  {:.1}% of the window changed for the new mail", seen * 100.0);
    assert!(app.get_rows().iter().any(|r| r.subject == "Arrived while closed"), "the new mail is not in the list");
    assert!(seen > 0.001, "the window still shows what it did before the mail came");
    stage.note("reopened").await;

    stage.close().await;
    stage.event(Event::BodyReady(controller.selected_message().unwrap()));
    stage.check("closed, a body arrived").await;
    stage.reopen().await;

    let theme = app.global::<Theme>();
    stage.close().await;
    let mut settings = Settings::from(theme.get_preferences());
    settings.compact_list = !settings.compact_list;
    theme.set_preferences(settings.into());
    stage.check("closed, a setting changed").await;
    stage.reopen().await;

    // Minimized is not closed: the window is still there, and the taskbar shows it.
    unsafe {
        let _ = ShowWindow(window(app), SW_MINIMIZE);
    }
    sleep(500).await;
    stage.note("minimized").await;
    // The spinner costs what it does in an open window, and ends with the sync.
    stage.event(Event::Syncing);
    sleep(300).await;
    stage.event(Event::UpToDate);
    store.save_messages(inbox, &[mail(51, "Arrived while minimized")]).unwrap();
    stage.event(Event::Changed);
    sleep(600).await;
    stage.note("minimized, new mail").await;
    unsafe {
        let _ = ShowWindow(window(app), SW_RESTORE);
    }
    sleep(600).await;
    stage.note("restored").await;

    // A focused field blinks its cursor from a timer of its own.
    let press = |key: &str| {
        app.window().dispatch_event(WindowEvent::KeyPressed { text: key.into() });
        app.window().dispatch_event(WindowEvent::KeyReleased { text: key.into() });
    };
    press("/");
    sleep(300).await;
    assert!(app.get_searching(), "the search field did not open");
    stage.close().await;
    stage.check("closed with the search field focused").await;
    stage.reopen().await;
    app.invoke_search_closed();
    sleep(300).await;
    controller.select_row(row(app, "Page"));
    until("the page", || app.get_pane() == Pane::Html).await;

    // Reopening animates too, so a quick close catches its frames.
    for wait in [0, 5, 20, 60] {
        stage.close().await;
        stage.reopen().await;
        sleep(wait).await;
        stage.close().await;
        stage.check(&format!("closed {wait} ms after reopening")).await;
        stage.reopen().await;
    }

    // Back within the moment before Slint hears of it.
    stage.close().await;
    stage.reopen().await;
    sleep(200).await;
    assert!(showing(app), "the window went away again");
    stage.note("reopened at once").await;

    // At different points of the page's load and of the animations that go with it.
    for wait in [0, 8, 40, 120, 300] {
        controller.select_row(row(app, "Other"));
        sleep(wait).await;
        stage.close().await;
        stage.check(&format!("closed {wait} ms into opening a page")).await;
        stage.reopen().await;
        controller.select_row(row(app, "Page"));
        until("the page", || app.get_pane() == Pane::Html).await;
    }

    controller.select_row(row(app, "Large"));
    sleep(1).await;
    stage.close().await;
    stage.check("closed while a large message was rendering").await;
    stage.reopen().await;
    stage.note("reopened").await;

    controller.select_row(row(app, "Page"));
    until("the page", || app.get_pane() == Pane::Html).await;
    sleep(600).await;
    // Windows drops what a hidden window showed, so all of it has to be drawn again, whether or
    // not Slint had heard of the hide by the time it came back.
    let before = pixels(app);
    for away in [0, 30, 100, 1000] {
        stage.close().await;
        sleep(away).await;
        stage.reopen().await;
        sleep(600).await;
        let seen = changed(&before, &pixels(app));
        println!("  {:.1}% of the window changed from closing it for {away} ms", seen * 100.0);
        assert!(seen < 0.01, "the window came back different after {away} ms");
    }
}

fn main() {
    if std::env::args().any(|a| a == "--list") {
        return;
    }
    let dir = std::env::temp_dir().join(format!("mailfunction-closed-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let store = Store::open(&dir).unwrap();
    let account = store
        .add_account(&Account {
            id: 0,
            email: "me@example.com".into(),
            host: "localhost".into(),
            port: 993,
            security: Security::Tls,
            auth: AuthKind::Google,
            smtp_host: "localhost".into(),
            smtp_port: 25,
        })
        .unwrap();
    let inbox = store.folder(account, "INBOX", &LabelSpec::system(Role::Inbox)).unwrap();
    let page = |text: &str| format!("From: a@example.com\r\nSubject: x\r\nContent-Type: text/html\r\n\r\n<p>{text}</p>\r\n").into_bytes();
    let large = format!("From: a@example.com\r\nSubject: x\r\nContent-Type: text/html\r\n\r\n{}\r\n", "<div><b>Filler</b> text</div>".repeat(20_000)).into_bytes();
    let bodies = [("Page", page("One")), ("Other", page("Two")), ("Large", large)];
    let messages: Vec<NewMessage> = bodies.iter().zip(1..).map(|((subject, _), uid)| mail(uid, subject)).collect();
    store.save_messages(&inbox, &messages).unwrap();
    let reader = Reader::open(&dir).unwrap();
    for row in reader.messages(&[inbox.label_id], 0, 20).unwrap() {
        let (_, raw) = bodies.iter().find(|(subject, _)| *subject == row.subject).unwrap();
        store.save_body(row.id, raw).unwrap();
    }

    let app = App::new().unwrap();
    let view = Rc::new(Webview::new(dir.join("webview")));
    let editor = Box::new(common::RecordingEditor::default());
    let notifier = Box::new(common::RecordingNotifier::default());
    let controller = Controller::new(app.clone_strong(), &dir, None, Box::new(view), editor, notifier, Arc::new(Offline)).unwrap();
    controller.load_accounts().unwrap();
    app.window().set_size(slint::LogicalSize::new(1100.0, 760.0));
    app.show().unwrap();

    let stage = Stage { app: app.clone_strong(), controller, account, failed: Cell::new(0) };
    let result = Rc::new(Cell::new(None));
    let outcome = result.clone();
    slint::spawn_local(async move {
        scenario(&stage, &store, &inbox).await;
        outcome.set(Some(stage.failed.get()));
        slint::quit_event_loop().unwrap();
    })
    .unwrap();
    // Closing the window must not end the loop, as in the app.
    slint::run_event_loop_until_quit().unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    match result.take() {
        Some(0) => println!("all closed-window checks passed"),
        Some(failed) => {
            println!("{failed} checks found the thread busy");
            std::process::exit(1);
        }
        None => std::process::exit(2),
    }
    std::process::exit(0);
}
