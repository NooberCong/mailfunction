//! The composer in the real window beside the real WebView2: who has the keyboard, and what the
//! editor uncovers when it moves. Clicks are posted to the test's own window, so nothing reaches
//! the desktop, and no servers are needed. Run with `cargo test -p mailfunction --test composer`.

mod common;

use std::rc::Rc;
use std::sync::Arc;
use std::sync::mpsc;

use common::{RecordingNotifier, typed};
use common::server::{client_pixels, sleep, until};
use mailfunction::editor::EditorView;
use mailfunction::webview::Webview;
use mailfunction::{App, Controller, Pane, Verb, chrome};
use mailfunction_core::remote::Offline;
use mailfunction_core::store::{Account, AuthKind, LabelSpec, NewMessage, Role, Security, Store};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::Input::KeyboardAndMouse::GetFocus;
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE};

const LEFT_BUTTON: usize = 1;

fn window(app: &App) -> HWND {
    let handle = app.window().window_handle();
    let RawWindowHandle::Win32(handle) = handle.window_handle().unwrap().as_raw() else { panic!("not Win32") };
    HWND(handle.hwnd.get() as *mut _)
}

async fn body_has_focus(editor: &EditorView) -> bool {
    let (tx, rx) = mpsc::channel();
    editor.with_view(|view| view.evaluate_script_with_callback("document.hasFocus()", move |result| drop(tx.send(result)))).unwrap().unwrap();
    loop {
        if let Ok(result) = rx.try_recv() {
            return result == "true";
        }
        sleep(10).await;
    }
}

/// A click on the subject field, as Windows delivers one.
async fn click_subject(app: &App) {
    let scale = app.window().scale_factor();
    let (x, y) = ((app.get_editor_x() + 120.0) * scale, (app.get_editor_y() - 20.0) * scale);
    let place = LPARAM(((y as isize) << 16) | x as isize);
    let hwnd = Some(window(app));
    unsafe {
        PostMessageW(hwnd, WM_MOUSEMOVE, WPARAM(0), place).unwrap();
        PostMessageW(hwnd, WM_LBUTTONDOWN, WPARAM(LEFT_BUTTON), place).unwrap();
        PostMessageW(hwnd, WM_LBUTTONUP, WPARAM(0), place).unwrap();
    }
    sleep(300).await;
}

/// Whether a band of the composer, between two heights, is all one colour. It starts past the
/// words of a short error, and the line down the side of the pane.
fn is_clean(app: &App, top: f32, bottom: f32) -> bool {
    let scale = app.window().scale_factor();
    let (width, _, pixels) = client_pixels(window(app).0 as isize).unwrap();
    let left = ((app.get_editor_x() + 200.0) * scale) as usize;
    let right = ((app.get_editor_x() + app.get_editor_w()) * scale) as usize - 1;
    let pixel = |x: usize, y: usize| &pixels[(y * width as usize + x) * 4..][..3];
    let (top, bottom) = ((top * scale).ceil() as usize, (bottom * scale) as usize);
    let first = pixel(left, top);
    (top..bottom).all(|y| (left..right).all(|x| pixel(x, y) == first))
}

/// The mail on screen when writing began, and a closed popup, leave nothing behind them in what
/// the editor covers, for the rows that push the editor down to uncover.
async fn nothing_is_left_under_the_editor(app: &App, controller: &Rc<Controller>, editor: &EditorView) {
    println!("opening a message, writing, then uncovering what the editor covered");
    // Warm, as it comes back quickly enough to cover the message before the composer is drawn.
    app.invoke_verb(Verb::Compose);
    until("the editor on screen", || editor.is_on_screen()).await;
    controller.finish_writing();
    until("the message list", || app.get_rows().row_count() > 0).await;
    controller.select_row(0);
    until("the message", || app.get_pane() == Pane::Text).await;
    sleep(500).await;
    app.invoke_verb(Verb::Compose);
    until("the editor on screen", || editor.is_on_screen()).await;
    sleep(300).await;

    let top = app.get_editor_y();
    let offered: Vec<SharedString> = vec!["An <an@example.com>".into(), "Binh <binh@example.com>".into()];
    app.set_suggestions(ModelRc::new(VecModel::from(offered)));
    app.set_suggesting(0);
    sleep(300).await;
    app.set_suggesting(-1);
    sleep(300).await;
    app.set_draft_error("Too large".into());
    sleep(300).await;
    assert!(app.get_editor_y() > top, "the row pushes the editor down");
    assert!(is_clean(app, top + 1.0, app.get_editor_y() - 1.0), "something is left in the row");
    app.set_draft_error("".into());
    sleep(300).await;
}

async fn scenario(app: App, controller: Rc<Controller>, editor: Rc<EditorView>) {
    nothing_is_left_under_the_editor(&app, &controller, &editor).await;

    // Twice, as the second click lands on the field Slint already thinks is focused, and that
    // click moves nothing inside Slint at all.
    for subject in ["x", "xx"] {
        println!("writing in the body, then clicking the subject");
        app.invoke_focus_editor();
        let mut waited = 0;
        while !body_has_focus(&editor).await {
            waited += 1;
            assert!(waited < 200, "the body never took the keyboard");
            sleep(10).await;
        }

        click_subject(&app).await;
        assert!(!body_has_focus(&editor).await, "the body keeps the keyboard");
        // Windows sends keys to the window now, and from there they go where Slint's focus is.
        assert_eq!(unsafe { GetFocus() }, window(&app), "the window has the keyboard");
        typed(&app, "x");
        assert_eq!(app.get_draft_subject(), subject, "the key lands in the subject");
    }
    println!("all composer checks passed");
}

fn main() {
    let dir = std::env::temp_dir().join(format!("mailfunction-composer-{}", std::process::id()));
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
    let message = NewMessage {
        uid: 1,
        remote_id: "m1".into(),
        gm_thread_id: Some(1),
        mid: Some("m1@x".into()),
        references: vec![],
        subject: "A long subject that puts the sender where the row will go".into(),
        from_name: "Brand".into(),
        from_addr: "news@brand.example".into(),
        date: 1_793_000_000,
        size: 100,
        seen: true,
        flagged: false,
        labels: vec![],
    };
    let id = store.save_messages(&inbox, &[message]).unwrap()[0];
    store.save_body(id, b"From: Brand <news@brand.example>\r\nSubject: x\r\n\r\nHello\r\n").unwrap();
    drop(store);

    let app = App::new().unwrap();
    let pane = Rc::new(Webview::new(dir.join("webview")));
    let editor = Rc::new(EditorView::new(dir.join("webview")));
    let notifier = Box::new(RecordingNotifier::default());
    let controller = Controller::new(app.clone_strong(), &dir, None, Box::new(pane), Box::new(editor.clone()), notifier, Arc::new(Offline)).unwrap();
    controller.load_accounts().unwrap();
    app.window().set_size(slint::LogicalSize::new(1280.0, 900.0));
    chrome::watch_window(&app);
    app.show().unwrap();

    let driver = app.clone_strong();
    slint::spawn_local(async move {
        scenario(driver, controller, editor).await;
        slint::quit_event_loop().unwrap();
    })
    .unwrap();
    slint::run_event_loop().unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    std::process::exit(0);
}
