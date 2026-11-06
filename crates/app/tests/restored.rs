//! A window brought back from the taskbar shows all of itself, whatever happened while it was down.
//! Windows drops what a minimized window showed, and Slint puts back only what it thinks changed,
//! so anything it drew in between left the rest of the window see-through. Reads the window's own
//! surface, as a picture of the screen would depend on what is behind it. Shows and restores
//! without activating, so it takes no focus, injects no input and needs no servers.
//! Run with `cargo test -p mailfunction --test restored`.

mod common;

use std::cell::{Cell, RefCell};
use std::future::poll_fn;
use std::rc::Rc;
use std::sync::Arc;
use std::task::{Poll, Waker};
use std::time::Duration;

use mailfunction::webview::Webview;
use mailfunction::{App, Controller};
use mailfunction_core::engine::Event;
use mailfunction_core::remote::Offline;
use mailfunction_core::store::{Account, AuthKind, Folder, LabelSpec, NewMessage, Role, Security, Store};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use slint::ComponentHandle;
use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::WindowsAndMessaging::{GetClientRect, SW_SHOWMINNOACTIVE, SW_SHOWNOACTIVATE, ShowWindow};

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

fn window(app: &App) -> HWND {
    let handle = app.window().window_handle();
    let RawWindowHandle::Win32(handle) = handle.window_handle().unwrap().as_raw() else { panic!("not Win32") };
    HWND(handle.hwnd.get() as _)
}

/// The share of the window with nothing on it, which is where the desktop shows through.
fn missing(app: &App) -> f64 {
    let hwnd = window(app);
    unsafe {
        let mut client = RECT::default();
        GetClientRect(hwnd, &mut client).unwrap();
        let (w, h) = (client.right, client.bottom);
        let dc = GetDC(Some(hwnd));
        let memory = CreateCompatibleDC(Some(dc));
        let mut info = BITMAPINFO::default();
        info.bmiHeader.biSize = size_of::<BITMAPINFOHEADER>() as u32;
        info.bmiHeader.biWidth = w;
        info.bmiHeader.biHeight = -h;
        info.bmiHeader.biPlanes = 1;
        info.bmiHeader.biBitCount = 32;
        let mut bits = std::ptr::null_mut();
        let bitmap = CreateDIBSection(Some(memory), &raw const info, DIB_RGB_COLORS, &raw mut bits, None, 0).unwrap();
        let previous = SelectObject(memory, bitmap.into());
        BitBlt(memory, 0, 0, w, h, Some(dc), 0, 0, SRCCOPY).unwrap();
        let _ = GdiFlush();
        let pixels = std::slice::from_raw_parts(bits.cast::<u32>(), (w * h) as usize);
        let empty = pixels.iter().filter(|&&pixel| pixel == 0).count();
        let share = empty as f64 / pixels.len() as f64;
        SelectObject(memory, previous);
        let _ = DeleteObject(bitmap.into());
        let _ = DeleteDC(memory);
        ReleaseDC(Some(hwnd), dc);
        share
    }
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
    /// Minimizes the window, lets `meanwhile` happen, brings it back and looks at what is there.
    async fn round(&self, what: &str, meanwhile: impl AsyncFnOnce()) {
        let hwnd = window(&self.app);
        let _ = unsafe { ShowWindow(hwnd, SW_SHOWMINNOACTIVE) };
        sleep(500).await;
        meanwhile().await;
        let _ = unsafe { ShowWindow(hwnd, SW_SHOWNOACTIVATE) };
        sleep(700).await;
        let share = missing(&self.app);
        let verdict = if share < 0.001 { "ok" } else { "HOLES" };
        println!("  {verdict:<6}{:>5.1}% of the window missing  {what}", share * 100.0);
        if share >= 0.001 {
            self.failed.set(self.failed.get() + 1);
        }
    }
}

async fn scenario(stage: &Stage, store: &Store, inbox: &Folder) {
    sleep(1000).await;
    println!("  open: {:.1}% missing", missing(&stage.app) * 100.0);

    stage.round("nothing happened", async || {}).await;
    stage
        .round("new mail arrived", async || {
            store.save_messages(inbox, &[mail(50, "Arrived while minimized")]).unwrap();
            stage.controller.on_event(stage.account, Event::Changed);
            sleep(600).await;
        })
        .await;
    stage
        .round("a sync ran", async || {
            stage.controller.on_event(stage.account, Event::Syncing);
            sleep(300).await;
            stage.controller.on_event(stage.account, Event::UpToDate);
            sleep(600).await;
        })
        .await;
    stage.round("nothing happened, again", async || {}).await;
    stage
        .round("more mail arrived", async || {
            store.save_messages(inbox, &[mail(51, "Also while minimized")]).unwrap();
            stage.controller.on_event(stage.account, Event::Changed);
            sleep(600).await;
        })
        .await;
}

fn main() {
    if std::env::args().any(|a| a == "--list") {
        return;
    }
    let dir = std::env::temp_dir().join(format!("mailfunction-restored-{}", std::process::id()));
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
    store.save_messages(&inbox, &[mail(1, "Already here")]).unwrap();

    let app = App::new().unwrap();
    let view = Rc::new(Webview::new(dir.join("webview")));
    let editor = Box::new(common::RecordingEditor::default());
    let notifier = Box::new(common::RecordingNotifier::default());
    let controller = Controller::new(app.clone_strong(), &dir, None, Box::new(view), editor, notifier, Arc::new(Offline)).unwrap();
    controller.load_accounts().unwrap();
    app.window().set_size(slint::LogicalSize::new(1100.0, 760.0));
    mailfunction::chrome::watch_window(&app);
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
    slint::run_event_loop_until_quit().unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    match result.take() {
        Some(0) => println!("all restored-window checks passed"),
        Some(failed) => {
            println!("{failed} rounds came back with holes");
            std::process::exit(1);
        }
        None => std::process::exit(2),
    }
    std::process::exit(0);
}
