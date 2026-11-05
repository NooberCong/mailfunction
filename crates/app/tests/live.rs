//! The real window, real WebView2, real sync against Dovecot and real sending through mailpit,
//! driven through the controller.
//! It opens a window, but injects no input: the one message it sends about the pointer goes to
//! its own window, and moves nothing on the desktop. Run with `scripts/test-imap.ps1`.

use std::cell::Cell;
use std::collections::HashSet;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use mailfunction::notify::{ACTIVATOR, APP_ID, Toasts};
use mailfunction::webview::Webview;
use mailfunction::{App, Controller, Pane, Theme, Verb, chrome};
use mailfunction_core::remote::Offline;
use mailfunction_core::render;
use mailfunction_core::secrets;
use mailfunction_core::store::{Account, AuthKind, Reader, Security, Store};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use slint::{ComponentHandle, Model, PhysicalSize};
use webview2_com::Microsoft::Web::WebView2::Win32::*;
use webview2_com::{CapturePreviewCompletedHandler, take_pwstr};
use windows::UI::Notifications::ToastNotificationManager;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::Storage::Xps::{PRINT_WINDOW_FLAGS, PrintWindow};
use windows::Win32::System::Com::{
    CLSCTX_LOCAL_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, STATFLAG_NONAME, STGC_DEFAULT,
    STREAM_SEEK_SET,
};
use windows::Win32::UI::Notifications::INotificationActivationCallback;
use windows::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SHCreateMemStream, SetWindowSubclass};
use windows::Win32::UI::WindowsAndMessaging::{
    GetClientRect, GetCursor, HTCLIENT, IDC_HAND, IsWindowVisible, LoadCursorW, PostMessageW, SendMessageW, WM_CLOSE,
    WM_MOUSEMOVE, WM_SETCURSOR,
};
use windows::core::{HSTRING, PCWSTR, PWSTR};
use wry::WebViewExtWindows;

/// Another session may be using the usual port, so it can be moved.
fn dovecot_port() -> u16 {
    std::env::var("MAILFUNCTION_IMAP_PORT").map_or(14143, |port| port.parse().unwrap())
}

fn sleep(ms: u64) -> impl Future<Output = ()> {
    let (tx, rx) = mpsc::channel::<()>();
    let done = Rc::new(Cell::new(false));
    let flag = done.clone();
    slint::Timer::single_shot(Duration::from_millis(ms), move || {
        flag.set(true);
        let _ = tx.send(());
    });
    Wait { done, rx }
}

/// Polls from the Slint executor, which wakes on timers.
struct Wait {
    done: Rc<Cell<bool>>,
    rx: mpsc::Receiver<()>,
}

impl Future for Wait {
    type Output = ();
    fn poll(self: std::pin::Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> std::task::Poll<()> {
        if self.done.get() {
            let _ = self.rx.try_recv();
            return std::task::Poll::Ready(());
        }
        let waker = cx.waker().clone();
        slint::Timer::single_shot(Duration::from_millis(10), move || waker.wake());
        std::task::Poll::Pending
    }
}

async fn until(what: &str, mut ready: impl FnMut() -> bool) {
    let started = Instant::now();
    while !ready() {
        assert!(started.elapsed() < Duration::from_secs(30), "timed out waiting for {what}");
        sleep(25).await;
    }
    println!("  {what}: {} ms", started.elapsed().as_millis());
}

/// Counts every connection made to it. Any hit at all means mail reached the network.
fn canary() -> (u16, Arc<AtomicU32>) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    let hits = Arc::new(AtomicU32::new(0));
    let count = hits.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            count.fetch_add(1, Ordering::SeqCst);
            let mut stream = stream;
            let mut line = String::new();
            let _ = stream.set_read_timeout(Some(Duration::from_millis(500)));
            let _ = BufReader::new(&stream).read_line(&mut line);
            println!("  canary hit: {:?}", line.trim());
            let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
        }
    });
    (port, hits)
}

/// Deliberately not sanitised: this attacks the webview layers directly.
fn hostile_page(port: u16) -> String {
    let base = format!("http://127.0.0.1:{port}");
    format!(
        r#"<!doctype html><html><head><title>clean</title>
        <meta http-equiv="refresh" content="0;url={base}/refresh">
        <link rel="stylesheet" href="{base}/link.css">
        <style>@import url({base}/import.css); body {{ background: url({base}/bg.png) }}
        @font-face {{ font-family: x; src: url({base}/font.woff) }} p {{ font-family: x }}</style>
        <script src="{base}/script.js"></script>
        <script>document.title = 'PWNED'; fetch('{base}/fetch'); location.href = '{base}/js-nav';</script>
        </head><body onload="document.title='PWNED'"><p>hostile</p>
        <img src="{base}/pixel.gif"><svg><image href="{base}/svg.png"/></svg>
        <iframe src="{base}/frame.html"></iframe><video src="{base}/v.mp4"></video>
        </body></html>"#
    )
}

fn title(view: &Webview) -> String {
    view.with_view(|v| unsafe {
        let mut p = PWSTR::null();
        v.webview().DocumentTitle(&mut p).unwrap();
        take_pwstr(p)
    })
    .unwrap()
}

async fn attack(app: &App, view: &Webview, port: u16, hits: &AtomicU32, label: &str) {
    println!("{label}");
    let before = view.activity.loaded.get();
    view.load(app.window(), (400.0, 80.0, 600.0, 400.0), hostile_page(port)).unwrap();
    until("hostile page loaded", || view.activity.loaded.get() > before).await;
    sleep(1500).await;
    assert_eq!(hits.load(Ordering::SeqCst), 0, "{label}: mail reached the network");
    assert_eq!(title(view), "clean", "{label}: a script ran");
}

async fn capture(view: &Webview, name: &str) {
    let stream = unsafe { SHCreateMemStream(None) }.unwrap();
    let finished = Rc::new(Cell::new(false));
    let done = finished.clone();
    view.with_view(|v| unsafe {
        v.webview()
            .CapturePreview(
                COREWEBVIEW2_CAPTURE_PREVIEW_IMAGE_FORMAT_PNG,
                &stream,
                &CapturePreviewCompletedHandler::create(Box::new(move |_| {
                    done.set(true);
                    Ok(())
                })),
            )
            .unwrap();
    });
    until("capture", || finished.get()).await;
    unsafe {
        stream.Commit(STGC_DEFAULT).unwrap();
        let mut stat = Default::default();
        stream.Stat(&mut stat, STATFLAG_NONAME).unwrap();
        stream.Seek(0, STREAM_SEEK_SET, None).unwrap();
        let mut bytes = vec![0u8; stat.cbSize as usize];
        let mut read = 0;
        stream.Read(bytes.as_mut_ptr().cast(), bytes.len() as u32, Some(&mut read)).ok().unwrap();
        std::fs::write(shot_dir().join(name), &bytes[..read as usize]).unwrap();
    }
}

fn shot_dir() -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/ui");
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// The child window's rectangle in physical pixels, which wry reports relative to the parent.
fn bounds(view: &Webview) -> (i32, i32, u32, u32) {
    view.with_view(|v| {
        let r = v.bounds().unwrap();
        let position = r.position.to_physical::<i32>(1.0);
        let size = r.size.to_physical::<u32>(1.0);
        (position.x, position.y, size.width, size.height)
    })
    .unwrap()
}

fn assert_on_slot(app: &App, view: &Webview) {
    let s = app.window().scale_factor();
    let (x, y, w, h) = bounds(view);
    let near = |a: f32, b: f32| (a - b).abs() <= 1.5;
    assert!(near(x as f32, app.get_slot_x() * s), "left {x} vs {}", app.get_slot_x() * s);
    assert!(near(y as f32, app.get_slot_y() * s), "top {y} vs {}", app.get_slot_y() * s);
    assert!(near(w as f32, app.get_slot_w() * s), "width {w} vs {}", app.get_slot_w() * s);
    assert!(near(h as f32, app.get_slot_h() * s), "height {h} vs {}", app.get_slot_h() * s);
}

const NEWSLETTER: &str = r##"<html><head><style>.h{font:600 26px Georgia,serif;color:#123}.b{background:#2f62d9;color:#fff;padding:10px 18px;border-radius:6px;text-decoration:none}</style></head>
<body><table width="100%" cellpadding="0" cellspacing="0" bgcolor="#f4f4f0"><tr><td align="center" style="padding:24px">
<table width="560" cellpadding="24" bgcolor="#ffffff"><tr><td>
<p class="h">Bản tin tuần này</p><p>Chào bạn, đây là những tin đáng đọc nhất tuần qua.</p>
<img src="https://tracker.example/open.gif" width="1" height="1"><img src="cid:logo" alt="logo" width="48">
<p><a class="b" href="https://example.com/read">Đọc thêm</a></p></td></tr></table></td></tr></table></body></html>"##;

fn newsletter_message() -> Vec<u8> {
    const PNG_1X1: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==";
    format!(
        "From: =?UTF-8?Q?B=E1=BA=A3n_tin?= <news@example.vn>\r\nTo: me@example.com\r\nSubject: =?UTF-8?Q?B=E1=BA=A3n_tin_tu=E1=BA=A7n?=\r\n\
         Message-ID: <news@example.vn>\r\nDate: Wed, 04 Nov 2026 10:00:00 +0700\r\nContent-Type: multipart/related; boundary=r\r\n\r\n\
         --r\r\nContent-Type: text/html; charset=utf-8\r\n\r\n{NEWSLETTER}\r\n\
         --r\r\nContent-Type: image/png\r\nContent-Transfer-Encoding: base64\r\nContent-ID: <logo>\r\nContent-Disposition: inline\r\n\r\n{PNG_1X1}\r\n--r--\r\n"
    )
    .into_bytes()
}

fn plain_message() -> Vec<u8> {
    b"From: An <an@example.com>\r\nTo: me@example.com\r\nSubject: Plain\r\nMessage-ID: <plain@x>\r\n\
      Date: Wed, 04 Nov 2026 09:00:00 +0700\r\nContent-Type: text/plain; charset=utf-8\r\n\r\nJust words.\r\n"
        .to_vec()
}

/// A full-height block of one colour, so that a frame shows at a glance whose it is.
fn colour_message(name: &str, colour: &str) -> Vec<u8> {
    format!(
        "From: {name} <{name}@example.com>\r\nTo: me@example.com\r\nSubject: {name}\r\nMessage-ID: <{name}@x>\r\n\
         Date: Mon, 02 Nov 2026 09:00:00 +0700\r\nContent-Type: text/html; charset=utf-8\r\n\r\n\
         <div style=\"height:3000px;background:{colour}\">{name}</div>\r\n"
    )
    .into_bytes()
}

fn window(app: &App) -> isize {
    let handle = app.window().window_handle();
    let RawWindowHandle::Win32(handle) = handle.window_handle().unwrap().as_raw() else { panic!("not Win32") };
    handle.hwnd.get()
}

type Frame = Vec<u8>;

/// A coarse grid of colours over part of a window, webview included. Read-only. Colour matters:
/// a page fading in from white passes through the brightness of any darker page.
fn grab(hwnd: isize, (x, y, width, height): (i32, i32, i32, i32)) -> Option<Frame> {
    const GRID: i32 = 24;
    let hwnd = HWND(hwnd as _);
    unsafe {
        let mut client = RECT::default();
        GetClientRect(hwnd, &mut client).ok()?;
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
        let mut frame = Vec::with_capacity((GRID * GRID * 3) as usize);
        for row in 0..GRID {
            for column in 0..GRID {
                let px = (x + width * column / GRID).min(w - 1);
                let py = (y + height * row / GRID).min(h - 1);
                let at = ((py * w + px) * 4) as usize;
                frame.extend_from_slice(&pixels[at..at + 3]);
            }
        }
        Some(frame)
    }
}

fn looks_like(a: &[u8], b: &[u8]) -> bool {
    let difference: u32 = a.iter().zip(b).map(|(x, y)| u32::from(x.abs_diff(*y))).sum();
    (difference as f32 / a.len() as f32) < 2.0
}

/// Records what the reading pane shows, as often as the window can be read.
struct Film {
    frames: Arc<Mutex<Vec<(Instant, Frame)>>>,
    stop: Arc<AtomicBool>,
}

impl Film {
    fn start(app: &App) -> Self {
        let s = app.window().scale_factor();
        let slot =
            ((app.get_slot_x() * s) as i32, (app.get_slot_y() * s) as i32, (app.get_slot_w() * s) as i32, (app.get_slot_h() * s) as i32);
        let hwnd = window(app);
        let frames: Arc<Mutex<Vec<(Instant, Frame)>>> = Arc::default();
        let stop = Arc::new(AtomicBool::new(false));
        let (recording, stopping) = (frames.clone(), stop.clone());
        std::thread::spawn(move || {
            while !stopping.load(Ordering::Relaxed) {
                // Stamped before the grab, which takes a while: a frame counts from when it began.
                let began = Instant::now();
                if let Some(frame) = grab(hwnd, slot) {
                    recording.lock().unwrap().push((began, frame));
                }
                std::thread::sleep(Duration::from_millis(4));
            }
        });
        Self { frames, stop }
    }

    fn latest(&self) -> Frame {
        self.frames.lock().unwrap().last().expect("no frames yet").1.clone()
    }
}

impl Drop for Film {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

/// Opens a message and waits for it to settle on screen.
async fn open_settled(app: &App, controller: &Rc<Controller>, film: &Film, row: usize) -> Frame {
    controller.select_row(row);
    until("message on screen", || matches!(app.get_pane(), Pane::Html | Pane::Text)).await;
    sleep(500).await;
    film.latest()
}

/// The message on screen goes with the click, even while the next one is still loading.
async fn moving_between_messages(app: &App, controller: &Rc<Controller>, film: &Film, html: (usize, usize), text: usize) {
    let first = open_settled(app, controller, film, html.0).await;
    let second = open_settled(app, controller, film, html.1).await;
    let plain = open_settled(app, controller, film, text).await;
    assert!(!looks_like(&first, &second), "the two messages should look different");
    // Only the coloured pages can be told from a blank pane, so they are what must never linger.
    let coloured = [(html.0, &first), (html.1, &second)];
    let sheets = [(html.0, &first), (html.1, &second), (text, &plain)];

    for (to, seen_last) in [(html.0, text), (html.1, html.0), (html.0, html.1), (text, html.0), (html.1, text), (html.0, html.1)] {
        let wanted = sheets.iter().find(|s| s.0 == to).unwrap().1;
        let started = Instant::now();
        film.frames.lock().unwrap().clear();
        controller.select_row(to);
        sleep(700).await;
        let frames = film.frames.lock().unwrap().clone();
        let arrived = frames.iter().find(|(_, frame)| looks_like(frame, wanted)).map(|(t, _)| t.duration_since(started));
        let arrived = arrived.expect("the message never showed");
        // The pixels change with the next paint after the click, so allow that one frame.
        let grace = started + Duration::from_millis(20);
        for (at, frame) in &frames {
            for (row, sheet) in coloured.iter().filter(|s| s.0 != to) {
                assert!(
                    *at < grace || !looks_like(frame, sheet),
                    "moving {seen_last} -> {to}: row {row} was on screen {} ms after the click",
                    at.duration_since(started).as_millis()
                );
            }
        }
        println!("  {seen_last} -> {to}: shown after {} ms, {} frames, none stale", arrived.as_millis(), frames.len());
    }
}

static LAST_CURSOR: AtomicIsize = AtomicIsize::new(0);
const WM_MOUSELEAVE: u32 = 0x02A3;

/// Marks a pointer move as this test's own.
const OURS: usize = 1 << 31;

/// Notes the cursor the window has just set in answer to WM_SETCURSOR. Windows keeps reporting
/// the real pointer, wherever it is, and those reports would undo the test's hover.
unsafe extern "system" fn note_cursor(hwnd: HWND, message: u32, w: WPARAM, l: LPARAM, _: usize, _: usize) -> LRESULT {
    if message == WM_MOUSELEAVE || (message == WM_MOUSEMOVE && w.0 & OURS == 0) {
        return LRESULT(0);
    }
    let w = if message == WM_MOUSEMOVE { WPARAM(0) } else { w };
    let result = unsafe { DefSubclassProc(hwnd, message, w, l) };
    if message == WM_SETCURSOR {
        LAST_CURSOR.store(unsafe { GetCursor() }.0 as isize, Ordering::SeqCst);
    }
    result
}

/// The cursors Windows is given at a point of the window, asked for the way the desktop does:
/// a posted pointer move, as real input arrives, and then WM_SETCURSOR. Both go to this window
/// alone, so nothing happens on the desktop.
async fn cursors_at(app: &App, x: f32, y: f32) -> HashSet<isize> {
    let s = app.window().scale_factor();
    let hwnd = HWND(window(app) as _);
    let mut seen = HashSet::new();
    for nudge in 0..4 {
        let (px, py) = ((x * s) as isize + nudge % 2, (y * s) as isize);
        unsafe { PostMessageW(Some(hwnd), WM_MOUSEMOVE, WPARAM(OURS), LPARAM(py << 16 | px)) }.unwrap();
        sleep(60).await;
        let hit = (WM_MOUSEMOVE as isize) << 16 | HTCLIENT as isize;
        unsafe { SendMessageW(hwnd, WM_SETCURSOR, Some(WPARAM(hwnd.0 as usize)), Some(LPARAM(hit))) };
        seen.insert(LAST_CURSOR.load(Ordering::SeqCst));
    }
    seen
}

async fn clickable_things_show_the_hand(app: &App) {
    unsafe { SetWindowSubclass(HWND(window(app) as _), Some(note_cursor), 1, 0) }.ok().unwrap();
    let hand = unsafe { LoadCursorW(None, IDC_HAND) }.unwrap().0 as isize;
    let height = app.window().size().to_logical(app.window().scale_factor()).height as f32;
    let accounts = app.get_accounts().row_count() as f32;
    let add = height - 8.0 - 18.0;
    let account = add - 18.0 - accounts * 46.0 + 22.0;
    for (name, x, y) in
        [("a sidebar label", 34.0, 64.0), ("a message", 300.0, 100.0), ("an account", 34.0, account), ("add account", 34.0, add)]
    {
        assert_eq!(cursors_at(app, x, y).await, HashSet::from([hand]), "the cursor over {name}");
    }
    assert!(!cursors_at(app, 300.0, 34.0).await.contains(&hand), "a hand over the list's heading");
    unsafe { RemoveWindowSubclass(HWND(window(app) as _), Some(note_cursor), 1) }.ok().unwrap();
    println!("  every clickable thing shows the hand, and the heading does not");
}

/// Delivers mail the way another client would, from its own thread and runtime.
fn deliver(user: String, messages: Vec<Vec<u8>>) {
    std::thread::spawn(move || {
        tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
            let tcp = tokio::net::TcpStream::connect(("127.0.0.1", dovecot_port()))
                .await
                .expect("start Dovecot: scripts/test-imap.ps1");
            let mut client = async_imap::Client::new(tcp);
            client.read_response().await.unwrap();
            let mut session = client.login(&user, "pass").await.map_err(|(e, _)| e).unwrap();
            for message in messages {
                session.append("INBOX", None, None, &message).await.unwrap();
            }
        });
    })
    .join()
    .unwrap();
}

/// How many messages a folder holds on the server, and how many of them are read.
fn on_server(user: &str, folder: &str) -> (u32, usize) {
    let (user, folder) = (user.to_string(), folder.to_string());
    std::thread::spawn(move || {
        tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
            let tcp = tokio::net::TcpStream::connect(("127.0.0.1", dovecot_port())).await.unwrap();
            let mut client = async_imap::Client::new(tcp);
            client.read_response().await.unwrap();
            let mut session = client.login(&user, "pass").await.map_err(|(e, _)| e).unwrap();
            let exists = session.select(&folder).await.unwrap().exists;
            (exists, session.search("SEEN").await.unwrap().len())
        })
    })
    .join()
    .unwrap()
}

fn create_folders(user: String) {
    std::thread::spawn(move || {
        tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
            let tcp = tokio::net::TcpStream::connect(("127.0.0.1", dovecot_port())).await.unwrap();
            let mut client = async_imap::Client::new(tcp);
            client.read_response().await.unwrap();
            let mut session = client.login(&user, "pass").await.map_err(|(e, _)| e).unwrap();
            for folder in ["Trash", "Sent", "Drafts"] {
                session.create(folder).await.unwrap();
            }
        });
    })
    .join()
    .unwrap();
}

fn delivered_to_mailpit(subject: &str) -> usize {
    let url = format!("http://127.0.0.1:18025/api/v1/search?query=subject:{subject}");
    let mut response = ureq::get(&url).call().expect("start mailpit: scripts/test-imap.ps1");
    let body = response.body_mut().read_to_string().unwrap();
    body.split("\"ID\"").count() - 1
}

/// A day's actions from the real window, each one checked on the real server.
async fn daily_use(app: &App, controller: &Rc<Controller>, user: &str) {
    println!("daily use");
    let before = app.get_rows().row_count();
    deliver(user.to_string(), vec![colour_message("fresh", "#a818a8")]);
    until("fresh mail", || app.get_rows().row_count() == before + 1).await;
    let rows = before + 1;
    let (total, seen) = on_server(user, "INBOX");
    assert_eq!(total as usize, rows);
    assert!(app.get_rows().row_data(0).unwrap().unread);
    controller.select_row(0);
    until("reading reaches the server", || on_server(user, "INBOX").1 == seen + 1).await;

    app.invoke_verb(Verb::Trash);
    assert_eq!(app.get_rows().row_count(), rows - 1, "gone from the list at once");
    until("the move reaches the server", || on_server(user, "Trash").0 == 1).await;
    assert_eq!(on_server(user, "INBOX").0, total - 1);

    app.invoke_verb(Verb::Undo);
    assert_eq!(app.get_rows().row_count(), rows);
    until("undo reaches the server", || on_server(user, "INBOX").0 == total).await;
    assert_eq!(on_server(user, "Trash").0, 0);

    // Written and sent from the real composer. It waits out its undo, then goes through real
    // SMTP, and files itself on the server as one copy.
    let subject = format!("liveword{}", std::process::id());
    app.invoke_verb(Verb::Compose);
    assert!(app.get_composing());
    app.set_draft_to("someone@example.com".into());
    app.set_draft_subject(subject.as_str().into());
    app.set_draft_body("Chào bạn".into());
    app.invoke_send_draft();
    assert!(!app.get_composing());
    assert_eq!(app.get_notice(), "Sending");
    sleep(1000).await;
    assert_eq!(delivered_to_mailpit(&subject), 0, "still in its undo window");
    until("it goes out through SMTP", || delivered_to_mailpit(&subject) == 1).await;
    until("it is filed in Sent", || on_server(user, "Sent").0 == 1).await;
    assert_eq!(on_server(user, "Sent").1, 1, "and read");
    until("the notice", || app.get_notice() == "Sent").await;
    assert_eq!(delivered_to_mailpit(&subject), 1, "once");
}

/// What Windows holds for the app, as each notification's XML.
fn delivered_toasts() -> Vec<String> {
    let history = ToastNotificationManager::History().unwrap();
    let toasts = history.GetHistoryWithId(&HSTRING::from(APP_ID)).unwrap();
    toasts.into_iter().map(|toast| toast.Content().unwrap().GetXml().unwrap().to_string()).collect()
}

fn clear_toasts() {
    ToastNotificationManager::History().unwrap().ClearWithId(&HSTRING::from(APP_ID)).unwrap();
}

/// Does what the shell does for a click on a notification, from outside the app's process.
fn click_toast(launch: &str) {
    unsafe {
        CoInitializeEx(None, COINIT_MULTITHREADED).ok().unwrap();
        let activator: INotificationActivationCallback = CoCreateInstance(&ACTIVATOR, None, CLSCTX_LOCAL_SERVER).unwrap();
        let (app_id, launch) = (HSTRING::from(APP_ID), HSTRING::from(launch));
        activator.Activate(PCWSTR(app_id.as_ptr()), PCWSTR(launch.as_ptr()), &[]).unwrap();
    }
}

async fn scenario(app: App, dir: PathBuf, user: String, controller: Rc<Controller>, view: Rc<Webview>) {
    let (port, hits) = canary();

    // Without the CSP, the request hook alone must still stop everything.
    let bare = Webview::with_policy(dir.join("bare"), "default-src * 'unsafe-inline'");
    attack(&app, &bare, port, &hits, "hook alone, CSP loosened").await;
    let refused = bare.activity.refused.borrow().clone();
    assert!(refused.iter().any(|u| u.ends_with("/pixel.gif")), "the hook saw nothing: {refused:?}");
    let cancelled = bare.activity.cancelled_navigations.borrow().clone();
    assert!(cancelled.iter().any(|u| u.ends_with("/refresh")), "the redirect was not stopped: {cancelled:?}");
    println!("  refused {} requests, cancelled {} navigations", refused.len(), cancelled.len());
    bare.close();

    attack(&app, &view, port, &hits, "full lockdown").await;

    println!("oversized document");
    let before = view.activity.loaded.get();
    let big = format!("<!doctype html><title>big</title><p>{}</p>", "x".repeat(3 * 1024 * 1024));
    view.load(app.window(), (400.0, 80.0, 600.0, 400.0), big).unwrap();
    until("3 MB page loaded", || view.activity.loaded.get() > before).await;
    assert_eq!(title(&view), "big");
    view.close();
    assert!(!view.is_open());

    println!("end to end");
    controller.start_engine();
    until("sync finished", || app.get_rows().row_count() == 2).await;
    assert_eq!(app.get_rows().row_data(0).unwrap().subject, "Bản tin tuần");

    let opened = Instant::now();
    controller.select_row(0);
    until("HTML message shown", || app.get_pane() == Pane::Html).await;
    println!("  first HTML open including fetch: {} ms", opened.elapsed().as_millis());
    sleep(300).await;
    assert_on_slot(&app, &view);
    assert_eq!(hits.load(Ordering::SeqCst), 0);
    capture(&view, "live-newsletter.png").await;

    // Dark mode: the page follows the theme, and the title bar follows the app.
    app.global::<Theme>().set_dark(true);
    view.with_view(|v| v.set_theme(wry::Theme::Dark).unwrap());
    controller.select_row(0);
    until("HTML message shown in dark mode", || app.get_pane() == Pane::Html).await;
    sleep(1500).await;
    capture(&view, "live-newsletter-dark.png").await;
    app.global::<Theme>().set_dark(false);
    view.with_view(|v| v.set_theme(wry::Theme::Light).unwrap());
    sleep(1500).await;

    for (w, h) in [(1000, 640), (1500, 900)] {
        app.window().set_size(PhysicalSize::new(w, h));
        sleep(300).await;
        assert_on_slot(&app, &view);
    }

    // Plain text needs no webview, so the pane hides it.
    until("second body prefetched or fetched", || {
        controller.select_row(1);
        app.get_pane() == Pane::Text
    })
    .await;
    assert!(!view.is_on_screen());

    // Back to HTML, which reuses the warm webview.
    let opened = Instant::now();
    controller.select_row(0);
    until("HTML message shown again", || app.get_pane() == Pane::Html).await;
    println!("  warm HTML open: {} ms", opened.elapsed().as_millis());
    assert_on_slot(&app, &view);

    println!("moving between messages");
    deliver(user.clone(), vec![colour_message("red", "#c81818"), colour_message("blue", "#1838c8")]);
    until("new mail pushed through IDLE", || app.get_rows().row_count() == 4).await;
    let film = Film::start(&app);
    sleep(300).await;
    // Newest first by arrival: blue, red, then the two from the start.
    assert_eq!(app.get_rows().row_data(0).unwrap().subject, "blue");
    moving_between_messages(&app, &controller, &film, (0, 1), 3).await;
    drop(film);

    daily_use(&app, &controller, &user).await;

    println!("a second account");
    let second = format!("second-{user}");
    secrets::save(&second, "pass").unwrap();
    deliver(second.clone(), vec![plain_message()]);
    let first_rows = app.get_rows().row_count();
    let local = |email: &str| Account {
        id: 0,
        email: email.into(),
        host: "127.0.0.1".into(),
        port: dovecot_port(),
        security: Security::Plain,
        auth: AuthKind::Password,
        smtp_host: "localhost".into(),
        smtp_port: 25,
    };
    controller.add_account(local(&second)).unwrap();
    assert_eq!(app.get_active_account(), 1);
    assert_eq!(app.get_pane(), Pane::Empty);
    assert!(!view.is_on_screen(), "the first account's message must not stay on screen");
    until("the new account's first sync", || app.get_rows().row_count() == 1).await;
    assert_eq!(app.get_rows().row_data(0).unwrap().subject, "Plain");

    // New mail for the account on screen shows, and for the one behind it only moves a count.
    controller.select_account(0);
    assert_eq!(app.get_rows().row_count(), first_rows);
    sleep(500).await;
    let unread = app.get_accounts().row_data(1).unwrap().unread;
    deliver(second.clone(), vec![colour_message("late", "#18a838")]);
    until("the background account's count", || app.get_accounts().row_data(1).unwrap().unread != unread).await;
    assert_eq!(app.get_rows().row_count(), first_rows, "nothing else moves");

    controller.select_account(1);
    until("the background account's list", || app.get_rows().row_count() == 2).await;
    let opened = Instant::now();
    controller.select_row(0);
    until("a message from the second account", || app.get_pane() == Pane::Html).await;
    println!("  opened in {} ms", opened.elapsed().as_millis());

    println!("the pointer");
    controller.select_account(0);
    clickable_things_show_the_hand(&app).await;

    println!("removing an account");
    let reader = Reader::open(&dir).unwrap();
    let id = reader.accounts().unwrap()[1].id;
    let inbox = reader.labels(id).unwrap().into_iter().next().unwrap().id;
    let message = reader.messages(inbox, 0, 1).unwrap()[0].id;
    controller.remove_account(1);
    assert_eq!(app.get_accounts().row_count(), 1);
    assert_eq!(app.get_rows().row_count(), first_rows);
    until("its mail deleted", || {
        reader.accounts().unwrap().len() == 1 && reader.body(message).unwrap().is_none() && secrets::load(&second).is_err()
    })
    .await;

    println!("notifications");
    clear_toasts();
    let mail = b"From: Linh Tran <linh@gmail.com>\r\nTo: me@example.com\r\nSubject: Lunch on Friday?\r\n\
                 Message-ID: <lunch@x>\r\nDate: Wed, 04 Nov 2026 12:00:00 +0700\r\n\
                 Content-Type: text/plain; charset=utf-8\r\n\r\nThe usual place.\r\n";
    deliver(user.clone(), vec![mail.to_vec()]);
    until("IDLE to notification", || !delivered_toasts().is_empty()).await;
    let toasts = delivered_toasts();
    assert_eq!(toasts.len(), 1, "{toasts:?}");
    assert!(toasts[0].contains("Linh Tran") && toasts[0].contains("Lunch on Friday?"), "{}", toasts[0]);
    let picture = toasts[0].split(r#"src=""#).nth(1).unwrap().split('"').next().unwrap();
    assert!(std::path::Path::new(picture).exists(), "the picture Windows was given: {picture}");

    let inbox = reader.labels(reader.accounts().unwrap()[0].id).unwrap()[0].id;
    let lunch = reader.messages(inbox, 0, 100).unwrap().into_iter().find(|m| m.subject == "Lunch on Friday?").unwrap().id;
    controller.select_label(1);
    assert_ne!(controller.selected_message(), Some(lunch));
    let launch = toasts[0].split(r#"launch=""#).nth(1).unwrap().split('"').next().unwrap();
    let mut click = std::process::Command::new(std::env::current_exe().unwrap()).args(["--click", launch]).spawn().unwrap();
    until("click to message", || controller.selected_message() == Some(lunch)).await;
    assert!(click.wait().unwrap().success());
    assert_eq!(app.get_active_label(), 0);
    until("message on screen", || app.get_pane() == Pane::Text).await;
    assert_eq!(app.get_message().subject, "Lunch on Friday?");
    clear_toasts();

    println!("the window closed");
    let hwnd = window(&app);
    let showing = || unsafe { IsWindowVisible(HWND(hwnd as _)) }.as_bool();
    let close = || unsafe { PostMessageW(Some(HWND(hwnd as _)), WM_CLOSE, WPARAM(0), LPARAM(0)) }.unwrap();
    let scale = app.window().scale_factor();
    let width = (app.get_slot_x() * scale) as i32;
    let height = ((app.get_slot_y() + app.get_slot_h()) * scale) as i32;
    let list = (0, 0, width, height);
    let page = ((app.get_slot_x() * scale) as i32, (app.get_slot_y() * scale) as i32, (app.get_slot_w() * scale) as i32, (app.get_slot_h() * scale) as i32);

    // A page lets go of its browser while nobody can see it, and comes back with the window.
    let messages = reader.messages(inbox, 0, 100).unwrap();
    let newsletter = messages.iter().find(|m| m.subject.starts_with("Bản tin")).unwrap().id;
    controller.select_row(reader.position(inbox, newsletter).unwrap().unwrap());
    until("page on screen", || app.get_pane() == Pane::Html).await;
    sleep(500).await;
    let on_screen = grab(hwnd, page).unwrap();
    close();
    until("window gone", || !showing()).await;
    assert!(bounds(&view).0 < 0, "the page stayed up behind a closed window");
    sleep(800).await;
    controller.reveal();
    assert!(showing());
    until("page back", || app.get_pane() == Pane::Html).await;
    assert_eq!(controller.selected_message(), Some(newsletter));
    assert_on_slot(&app, &view);
    until("page painted", || grab(hwnd, page).is_some_and(|now| looks_like(&now, &on_screen))).await;

    // Mail still arrives, is still announced, and is on screen when the window returns.
    let list_before = grab(hwnd, list).unwrap();
    close();
    until("window gone", || !showing()).await;
    let mail = b"From: Minh Pham <minh@gmail.com>\r\nTo: me@example.com\r\nSubject: Keys are at reception\r\n\
                 Message-ID: <keys@x>\r\nDate: Wed, 04 Nov 2026 13:00:00 +0700\r\n\
                 Content-Type: text/plain; charset=utf-8\r\n\r\nSee you there.\r\n";
    deliver(user.clone(), vec![mail.to_vec()]);
    until("IDLE to notification, window closed", || !delivered_toasts().is_empty()).await;
    assert!(!showing(), "new mail opened the window");
    let toast = delivered_toasts().remove(0);
    assert!(toast.contains("Keys are at reception"), "{toast}");
    let keys = reader.messages(inbox, 0, 100).unwrap().into_iter().find(|m| m.subject.starts_with("Keys")).unwrap().id;
    let launch = toast.split(r#"launch=""#).nth(1).unwrap().split('"').next().unwrap();
    let mut click = std::process::Command::new(std::env::current_exe().unwrap()).args(["--click", launch]).spawn().unwrap();
    until("click to message, window closed", || showing() && controller.selected_message() == Some(keys)).await;
    assert!(click.wait().unwrap().success());
    until("message on screen", || app.get_pane() == Pane::Text).await;
    assert_eq!(app.get_message().subject, "Keys are at reception");
    until("list repainted", || grab(hwnd, list).is_some_and(|now| now != list_before)).await;
    clear_toasts();

    secrets::delete(&user).unwrap();
    println!("all live checks passed");

    // Lets a process monitor read steady-state memory with one webview open.
    if let Some(ms) = std::env::var("LIVE_HOLD_MS").ok().and_then(|v| v.parse().ok()) {
        sleep(ms).await;
    }
    view.close();
    drop(controller);
    let _ = std::fs::remove_dir_all(&dir);
}

fn main() {
    let args: Vec<_> = std::env::args().collect();
    if args.iter().any(|a| a == "--list") {
        return;
    }
    if let [_, flag, launch] = args.as_slice()
        && flag == "--click"
    {
        return click_toast(launch);
    }
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    let user = format!("live-{nanos}@example.com");
    let dir = std::env::temp_dir().join(format!("mailfunction-{user}"));
    assert!(render::message(&newsletter_message()).is_some());

    deliver(user.clone(), vec![plain_message(), newsletter_message()]);
    create_folders(user.clone());
    {
        let store = Store::open(&dir).unwrap();
        store
            .add_account(&Account {
                id: 0,
                email: user.clone(),
                host: "127.0.0.1".into(),
                port: dovecot_port(),
                security: Security::Plain,
                auth: AuthKind::Password,
                smtp_host: "127.0.0.1".into(),
                smtp_port: 11025,
            })
            .unwrap();
    }
    secrets::save(&user, "pass").unwrap();

    let app = App::new().unwrap();
    let view = Rc::new(Webview::new(dir.join("webview")));
    let toasts = Box::new(Toasts::new(&dir));
    let controller = Controller::new(app.clone_strong(), &dir, None, Box::new(view.clone()), toasts, Arc::new(Offline)).unwrap();
    controller.load_accounts().unwrap();
    app.show().unwrap();
    chrome::set_icon(&app);
    chrome::paint_title_bar(&app);

    let driver = app.clone_strong();
    let dir_for_scenario = dir.clone();
    slint::spawn_local(async move {
        scenario(driver, dir_for_scenario, user, controller, view).await;
        slint::quit_event_loop().unwrap();
    })
    .unwrap();
    slint::run_event_loop().unwrap();
    std::process::exit(0);
}
