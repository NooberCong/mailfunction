//! How fast mail with remote images opens, in the real window and the real WebView2, against a
//! web that takes its time. Opens a window but injects no input, and needs no servers.
//! Run with `cargo test -p mailfunction --test images`.

use std::cell::{Cell, RefCell};
use std::future::poll_fn;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::{Poll, Waker};
use std::time::{Duration, Instant};

use mailfunction::notify::{Notice, Notifier};
use mailfunction::webview::Webview;
use mailfunction::{App, Controller, ImageBar, Pane, Theme};
use mailfunction_core::remote::{Logo, Network};
use mailfunction_core::settings::Settings;
use mailfunction_core::store::{Account, AuthKind, LabelSpec, NewMessage, Reader, Role, Security, Store};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use slint::{ComponentHandle, Model};
use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::Storage::Xps::{PRINT_WINDOW_FLAGS, PrintWindow};
use windows::Win32::UI::WindowsAndMessaging::GetClientRect;

const RED_DOT: &[u8] = b"GIF89a\x01\0\x01\0\x80\0\0\xff\0\0\xff\0\0,\0\0\0\0\x01\0\x01\0\0\x02\x02D\x01\0;";
const IMAGE_TIME: Duration = Duration::from_millis(1500);
const IMAGES: usize = 24;
/// In the time one image takes, a page that waited for them could not be up.
const OPENS_WITHIN: Duration = Duration::from_millis(500);

/// A web where every image takes [`IMAGE_TIME`], and a URL with "gone" in it never arrives.
#[derive(Default)]
struct SlowWeb {
    asked: Mutex<Vec<String>>,
    running: AtomicUsize,
    peak: AtomicUsize,
}

impl SlowWeb {
    fn asked(&self) -> usize {
        self.asked.lock().unwrap().len()
    }
}

impl Network for SlowWeb {
    fn image(&self, url: &str) -> Option<Vec<u8>> {
        self.asked.lock().unwrap().push(url.to_string());
        let running = self.running.fetch_add(1, Ordering::SeqCst) + 1;
        self.peak.fetch_max(running, Ordering::SeqCst);
        std::thread::sleep(IMAGE_TIME);
        self.running.fetch_sub(1, Ordering::SeqCst);
        (!url.contains("gone")).then(|| RED_DOT.to_vec())
    }

    fn logo(&self, _: &str) -> Logo {
        Logo::Unknown
    }
}

struct Silent;

impl Notifier for Silent {
    fn show(&self, _: &Notice) {}
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

/// Waits for something, and says how long it took.
async fn until(what: &str, mut ready: impl FnMut() -> bool) -> Duration {
    let started = Instant::now();
    while !ready() {
        assert!(started.elapsed() < Duration::from_secs(30), "timed out waiting for {what}");
        sleep(5).await;
    }
    println!("  {what}: {} ms", started.elapsed().as_millis());
    started.elapsed()
}

/// The window's whole client area, webview included, as blue, green, red and a spare byte.
fn client_pixels(app: &App) -> (i32, i32, Vec<u8>) {
    let handle = app.window().window_handle();
    let RawWindowHandle::Win32(handle) = handle.window_handle().unwrap().as_raw() else { panic!("not Win32") };
    let hwnd = HWND(handle.hwnd.get() as _);
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
        (w, h, pixels)
    }
}

/// How much of the window is the red of [`RED_DOT`], in pixels.
fn red_pixels(app: &App) -> usize {
    client_pixels(app).2.chunks_exact(4).filter(|p| p[2] > 240 && p[1] < 20 && p[0] < 20).count()
}

/// Keeps a picture of the window in `target/ui`.
fn save(app: &App, name: &str) {
    let (w, h, pixels) = client_pixels(app);
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/ui");
    std::fs::create_dir_all(&dir).unwrap();
    let file = std::fs::File::create(dir.join(format!("{name}.png"))).unwrap();
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), w as u32, h as u32);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let rgba: Vec<u8> = pixels.chunks_exact(4).flat_map(|p| [p[2], p[1], p[0], 255]).collect();
    encoder.write_header().unwrap().write_image_data(&rgba).unwrap();
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
        date: 1_793_000_000 - i64::from(uid) * 3600,
        size: 100,
        seen: true,
        flagged: false,
        labels: vec![],
    }
}

/// A newsletter whose images are at `path`, which puts them in a row of their own in the cache.
fn newsletter(path: &str) -> Vec<u8> {
    let images: String = (0..IMAGES)
        .map(|n| format!("<img alt=\"Picture\" width=\"100\" height=\"100\" src=\"https://images.example/{path}/{n}.gif\">"))
        .collect();
    format!(
        "From: news@brand.example\r\nTo: me@example.com\r\nSubject: x\r\nContent-Type: text/html; charset=utf-8\r\n\r\n\
         <h1>News</h1>{images}\r\n"
    )
    .into_bytes()
}

/// From its style sheet, which the sanitiser leaves alone, asks for a page image and for a server.
fn sneaky(port: u16) -> Vec<u8> {
    format!(
        "From: news@brand.example\r\nTo: me@example.com\r\nSubject: x\r\nContent-Type: text/html; charset=utf-8\r\n\r\n\
         <style>h1{{background:url(https://message.invalid/image/0)}}\
         p{{background:url(http://127.0.0.1:{port}/pixel.gif)}}</style><h1>News</h1><p>Text</p>\
         <img src=\"https://images.example/sneaky.gif\">\r\n"
    )
    .into_bytes()
}

/// Counts the connections made to it. Any at all means a page reached the network.
fn canary() -> (u16, Arc<AtomicUsize>) {
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    let hits = Arc::new(AtomicUsize::new(0));
    let count = hits.clone();
    std::thread::spawn(move || {
        for _ in listener.incoming() {
            count.fetch_add(1, Ordering::SeqCst);
        }
    });
    (port, hits)
}

fn row(app: &App, subject: &str) -> usize {
    let rows = app.get_rows();
    (0..rows.row_count()).find(|&i| rows.row_data(i).unwrap().subject == subject).unwrap()
}

async fn scenario(app: App, controller: Rc<Controller>, view: Rc<Webview>, web: Arc<SlowWeb>, hits: Arc<AtomicUsize>) {
    let open = async |subject: &str, what: &str| {
        controller.select_row(row(&app, subject));
        until(what, || app.get_pane() == Pane::Html).await
    };

    println!("the first page, with the browser still to start");
    open("Warm up", "page up").await;

    println!("a newsletter never seen before");
    let page = open("Fresh", "page up").await;
    assert!(page < OPENS_WITHIN, "the page waited: {page:?}");
    assert_eq!(red_pixels(&app), 0, "the images cannot have come yet");
    sleep(250).await;
    save(&app, "images-1-page-up");
    until("first image on the page", || red_pixels(&app) > 0).await;
    sleep(100).await;
    save(&app, "images-2-coming-in");
    let scale = f64::from(app.window().scale_factor());
    // All of them, short of the pixels a fractional scale loses at their edges.
    let whole = (100.0 * 100.0 * IMAGES as f64 * scale * scale * 0.9) as usize;
    until("all of them", || red_pixels(&app) > whole).await;
    let (asked, peak) = (web.asked(), web.peak.load(Ordering::SeqCst));
    println!("  {asked} fetched, up to {peak} at a time");
    assert!(peak >= 8, "only {peak} at a time");

    println!("opened again");
    open("Plain page", "another page").await;
    let page = open("Fresh", "page up").await;
    assert!(page < OPENS_WITHIN, "the page waited: {page:?}");
    let images = until("all of them", || red_pixels(&app) > whole).await;
    assert!(images < IMAGE_TIME, "they came from the web: {images:?}");
    assert_eq!(web.asked(), asked, "held images were fetched again");

    println!("left before the images came");
    open("Left", "page up").await;
    open("Plain page", "another page").await;
    sleep(u64::try_from(IMAGE_TIME.as_millis()).unwrap() * 2).await;
    let fetched = web.asked() - asked;
    println!("  {fetched} of {IMAGES} fetched");
    assert!(fetched <= 8, "a page nobody is reading went on fetching: {fetched}");

    println!("nothing comes at all");
    let asked = web.asked();
    open("Gone", "page up").await;
    assert_eq!(app.get_image_bar(), ImageBar::Hidden);
    until("the offer to try again", || app.get_image_bar() == ImageBar::Failed).await;
    sleep(200).await;
    save(&app, "images-3-none-came");
    assert_eq!(web.asked() - asked, IMAGES);

    println!("a page that may not load images cannot make it happen");
    let theme = app.global::<Theme>();
    let mut settings = Settings::from(theme.get_preferences());
    settings.show_all_images = false;
    theme.set_preferences(settings.into());
    sleep(50).await;
    let (asked, images_before) = (web.asked(), view.activity.image_requests.get());
    open("Sneaky", "page up").await;
    assert_eq!(app.get_image_bar(), ImageBar::Blocked);
    sleep(300).await;
    assert_eq!(view.activity.image_requests.get(), images_before + 1, "the page's own request never came");
    assert_eq!(web.asked(), asked, "a blocked page reached the web");
    assert_eq!(hits.load(Ordering::SeqCst), 0, "a page reached the network");
}

fn main() {
    let dir = std::env::temp_dir().join(format!("mailfunction-images-{}", std::process::id()));
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
    let (port, hits) = canary();
    let plain = b"From: a@example.com\r\nSubject: x\r\nContent-Type: text/html\r\n\r\n<p>Plain</p>\r\n".to_vec();
    let bodies =
        [("Warm up", plain.clone()), ("Plain page", plain), ("Fresh", newsletter("fresh")), ("Left", newsletter("left")), ("Gone", newsletter("gone")), ("Sneaky", sneaky(port))];
    let messages: Vec<NewMessage> = bodies.iter().zip(1..).map(|((subject, _), uid)| mail(uid, subject)).collect();
    store.save_messages(&inbox, &messages).unwrap();
    let reader = Reader::open(&dir).unwrap();
    for row in reader.messages(inbox.label_id, 0, 10).unwrap() {
        let (_, raw) = bodies.iter().find(|(subject, _)| *subject == row.subject).unwrap();
        store.save_body(row.id, raw).unwrap();
    }

    let app = App::new().unwrap();
    let web = Arc::new(SlowWeb::default());
    let view = Rc::new(Webview::new(dir.join("webview")));
    let controller =
        Controller::new(app.clone_strong(), &dir, None, Box::new(view.clone()), Box::new(Silent), web.clone()).unwrap();
    controller.load_accounts().unwrap();
    let theme = app.global::<Theme>();
    let mut settings = Settings::from(theme.get_preferences());
    settings.show_all_images = true;
    theme.set_preferences(settings.into());
    app.window().set_size(slint::LogicalSize::new(1280.0, 900.0));
    app.show().unwrap();

    let driver = app.clone_strong();
    slint::spawn_local(async move {
        scenario(driver, controller, view, web, hits).await;
        slint::quit_event_loop().unwrap();
    })
    .unwrap();
    slint::run_event_loop().unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    std::process::exit(0);
}
