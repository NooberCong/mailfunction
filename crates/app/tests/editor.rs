//! The real editor: the real page in a real WebView2 in the real window, sending through real
//! Dovecot and mailpit. It opens a window, but no input reaches the desktop: everything happens
//! through the page's own script. It leaves notifications alone, so it can run beside the app.
//! Run with `scripts/test-imap.ps1`.

mod common;

use std::path::Path;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::mpsc;
use std::sync::atomic::Ordering;
use std::time::Instant;

use common::server::{canary, client_pixels, deliver, delivered_to_mailpit, dovecot_port, received_by_mailpit, sleep, until};
use common::RecordingNotifier;
use mailfunction::editor::EditorView;
use mailfunction::webview::Webview;
use mailfunction::{App, Controller, Verb};
use mailfunction_core::remote::Offline;
use mailfunction_core::secrets;
use mailfunction_core::store::{Account, AuthKind, Reader, Role, Security, Store};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use slint::{ComponentHandle, Model};

const PNG: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==";

/// The real window, webview and all, in `target/ui`.
fn shot(app: &App, name: &str) {
    let handle = app.window().window_handle();
    let RawWindowHandle::Win32(handle) = handle.window_handle().unwrap().as_raw() else { panic!("not Win32") };
    let (width, height, mut pixels) = client_pixels(handle.hwnd.get()).unwrap();
    for pixel in pixels.chunks_exact_mut(4) {
        pixel.swap(0, 2);
        pixel[3] = 255;
    }
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/ui");
    std::fs::create_dir_all(&dir).unwrap();
    let file = std::fs::File::create(dir.join(format!("{name}.png"))).unwrap();
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), width as u32, height as u32);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.write_header().unwrap().write_image_data(&pixels).unwrap();
}

async fn eval(editor: &EditorView, script: &str) -> String {
    let (tx, rx) = mpsc::channel();
    editor.with_view(|view| view.evaluate_script_with_callback(script, move |result| drop(tx.send(result)))).unwrap().unwrap();
    loop {
        if let Ok(result) = rx.try_recv() {
            return result;
        }
        sleep(10).await;
    }
}

fn hostile_mail(port: u16) -> Vec<u8> {
    format!(
        "From: Mallory <mallory@example.com>\r\nTo: me@example.com\r\nSubject: Hostile\r\nMessage-ID: <hostile@x>\r\n\
         Date: Wed, 04 Nov 2026 12:00:00 +0700\r\nContent-Type: text/html; charset=utf-8\r\n\r\n\
         <p>Rich <b>quoted</b> text</p><script>window.pwned=1</script>\
         <img src=\"http://127.0.0.1:{port}/pixel.gif\" onerror=\"window.pwned=2\">\
         <a href=\"javascript:window.pwned=3\">link</a>\r\n"
    )
    .into_bytes()
}

async fn the_page_is_locked_down(editor: &EditorView, port: u16) {
    println!("lockdown");
    let ran = "const s=document.createElement('script');s.textContent='window.ran=1';document.head.append(s);window.ran===undefined";
    assert_eq!(eval(editor, ran).await, "true", "an inline script must not run");
    let foreign = "const f=document.createElement('script');f.src='http://127.0.0.1:PORT/x.js';document.head.append(f);\
                   new Image().src='http://127.0.0.1:PORT/y.png';fetch('http://127.0.0.1:PORT/z').catch(()=>{});true";
    assert_eq!(eval(editor, &foreign.replace("PORT", &port.to_string())).await, "true");
    let moved = "location.href='http://127.0.0.1:PORT/away';true".replace("PORT", &port.to_string());
    eval(editor, &moved).await;
    sleep(400).await;
    assert_eq!(eval(editor, "location.pathname").await, "\"/editor.html\"", "the page goes nowhere");
    assert_eq!(eval(editor, r#"document.fonts.check('14px "Be Vietnam Pro"')"#).await, "true", "its font is served from memory");
}

async fn writing_and_sending(app: &App, controller: &Rc<Controller>, editor: &EditorView, user: &str, dir: &Path) {
    println!("writing");
    let started = Instant::now();
    app.invoke_verb(Verb::Compose);
    until("the editor on screen", || editor.is_on_screen()).await;
    println!("  first open: {} ms", started.elapsed().as_millis());
    assert!(!eval(editor, "body.innerHTML").await.contains("<script"));

    let subject = format!("rich-{}", std::process::id());
    app.set_draft_to("someone@example.com".into());
    app.set_draft_subject(subject.as_str().into());
    let write = r##"
        body.focus(); getSelection().collapse(body, 0);
        document.execCommand("insertText", false, "Hello brave world");
        const text = body.firstChild.firstChild ?? body.firstChild;
        const range = document.createRange(); range.setStart(text, 6); range.setEnd(text, 11);
        getSelection().removeAllRanges(); getSelection().addRange(range); remember();
        run("bold"); run("foreColor", "#c4122f");
        true"##;
    assert_eq!(eval(editor, write).await, "true");
    until("the words reaching the app", || controller.draft_text().contains("Hello brave world")).await;

    let picture = format!(
        r#"getSelection().collapse(body, body.childNodes.length); remember();
           document.execCommand("insertHTML", false, '<img alt="dot" src="data:image/png;base64,{PNG}">');
           adoptPictures(); changed(); true"#
    );
    assert_eq!(eval(editor, &picture).await, "true");
    sleep(300).await;

    sleep(400).await;
    shot(app, "40-editor");

    println!("sending");
    assert_eq!(eval(editor, "document.querySelector('#send').click(); true").await, "true");
    until("the composer closing", || !app.get_composing()).await;
    assert!(!editor.is_on_screen(), "the page leaves with the composer");
    until("the message at mailpit", || delivered_to_mailpit(&subject) == 1).await;
    let raw = received_by_mailpit(&subject);
    assert!(raw.contains(&format!("<{user}>")), "{raw}");
    for part in ["multipart/alternative", "multipart/related", "text/plain", "text/html", "image/png", "Content-ID:"] {
        assert!(raw.contains(part), "no {part} in:\n{raw}");
    }
    let parsed = mail_parser::MessageParser::default().parse(raw.as_bytes()).unwrap();
    let html = parsed.body_html(0).unwrap();
    assert!(html.contains("brave") && (html.contains("font-weight") || html.contains("<b>")), "{html}");
    assert!(html.contains("cid:") && !html.contains("base64"), "the picture goes beside the text: {html}");
    assert!(parsed.body_text(0).unwrap().contains("Hello brave world"));
    assert_eq!(parsed.attachments().filter(|p| p.content_id().is_some()).count(), 1);
    assert_eq!(Reader::open(dir).unwrap().accounts().unwrap().len(), 2);
}

async fn replying_to_hostile_mail(app: &App, controller: &Rc<Controller>, editor: &EditorView, pane: &Webview, hits: &std::sync::atomic::AtomicU32) {
    println!("replying");
    until("the hostile mail", || app.get_rows().iter().any(|r| r.subject == "Hostile")).await;
    let row = app.get_rows().iter().position(|r| r.subject == "Hostile").unwrap();
    controller.select_row(row);
    until("the message in the reading pane", || pane.is_on_screen()).await;
    app.invoke_verb(Verb::Reply);
    until("the reply in the editor", || editor.is_on_screen()).await;
    assert!(!pane.is_on_screen(), "one browser at a time in the same place");
    let page = eval(editor, "body.innerHTML").await;
    assert!(page.contains("quoted"), "{page}");
    assert!(!page.contains("<script") && !page.contains("onerror") && !page.contains("pixel.gif"), "{page}");
    assert!(!page.contains("javascript:"), "{page}");
    assert_eq!(eval(editor, "window.pwned === undefined").await, "true");
    shot(app, "41-editor-reply");
    let order = "body.querySelector('.quoted') && !!body.querySelector('.quoted blockquote')";
    assert_eq!(eval(editor, order).await, "true");

    // Esc in the page ends the composer, and a reply nobody touched leaves nothing behind.
    let keys = "dispatchEvent(new KeyboardEvent('keydown',{key:'Escape',bubbles:true,cancelable:true}));true";
    assert_eq!(eval(editor, keys).await, "true");
    until("Esc closing the composer", || !app.get_composing()).await;
    until("the message back in the reading pane", || pane.is_on_screen()).await;
    assert!(!editor.is_on_screen());
    assert_eq!(hits.load(Ordering::SeqCst), 0, "the editor reached the network");
}

async fn moving_between_accounts(app: &App, controller: &Rc<Controller>, editor: &EditorView, dir: &Path) {
    println!("from another account");
    let reader = Reader::open(dir).unwrap();
    let second = reader.accounts().unwrap()[1].id;
    controller.select_all_accounts();
    app.invoke_identity_edited(0, "".into(), "First signature".into(), true);
    app.invoke_identity_edited(1, "".into(), "Second signature".into(), true);
    app.invoke_verb(Verb::Compose);
    until("the editor on screen", || editor.is_on_screen()).await;
    assert!(eval(editor, "body.innerText").await.contains("First signature"));
    app.set_draft_to("kept@example.com".into());
    let typed = r#"body.focus(); getSelection().collapse(body, 0); document.execCommand("insertText", false, "Kept"); true"#;
    assert_eq!(eval(editor, typed).await, "true");
    until("the words", || controller.draft_text().contains("Kept")).await;
    controller.save_draft();

    let key = "dispatchEvent(new KeyboardEvent('keydown',{key:'2',code:'Digit2',ctrlKey:true,bubbles:true,cancelable:true}));true";
    assert_eq!(eval(editor, key).await, "true");
    until("the signature changing", || controller.draft_text().contains("Second signature")).await;
    let shown = eval(editor, "body.innerText").await;
    assert!(shown.contains("Kept") && !shown.contains("First signature"), "{shown}");
    controller.finish_writing();

    println!("a draft comes back as it was");
    let drafts = reader.labels(second).unwrap().into_iter().find(|l| l.role == Role::Drafts).unwrap();
    let draft = reader.messages(&[drafts.id], 0, 1).unwrap()[0].id;
    let stored = reader.draft(draft).unwrap().unwrap();
    assert!(stored.html.contains("Kept") && stored.html.contains("Second signature"), "{}", stored.html);
    let inbox = app.get_labels().iter().position(|l| l.name == "Drafts").unwrap();
    controller.select_label(inbox);
    until("the list of drafts", || app.get_rows().row_count() > 0).await;
    controller.select_row(0);
    until("the draft in the editor", || editor.is_on_screen()).await;
    assert!(eval(editor, "body.innerText").await.contains("Kept"));
    controller.finish_writing();
}

#[allow(clippy::too_many_arguments)]
async fn scenario(app: App, controller: Rc<Controller>, editor: Rc<EditorView>, pane: Rc<Webview>, user: String, dir: std::path::PathBuf, port: u16, hits: Arc<std::sync::atomic::AtomicU32>) {
    until("the first sync", || app.get_labels().row_count() > 0).await;
    writing_and_sending(&app, &controller, &editor, &user, &dir).await;
    app.invoke_verb(Verb::Compose);
    until("the editor on screen", || editor.is_on_screen()).await;
    the_page_is_locked_down(&editor, port).await;
    controller.finish_writing();
    replying_to_hostile_mail(&app, &controller, &editor, &pane, &hits).await;
    assert!(editor.refused.borrow().iter().all(|url| url.contains(&port.to_string())), "{:?}", editor.refused.borrow());
    moving_between_accounts(&app, &controller, &editor, &dir).await;
    assert_eq!(hits.load(Ordering::SeqCst), 0, "the editor reached the network");
    println!("all editor checks passed");
}

fn main() {
    let dir = std::env::temp_dir().join(format!("mailfunction-editor-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let stamp = std::process::id();
    let (user, second) = (format!("editor-{stamp}@example.com"), format!("editor-two-{stamp}@example.com"));
    let store = Arc::new(Store::open(&dir).unwrap());
    for email in [&user, &second] {
        store
            .add_account(&Account {
                id: 0,
                email: email.clone(),
                host: "127.0.0.1".into(),
                port: dovecot_port(),
                security: Security::Plain,
                auth: AuthKind::Password,
                smtp_host: "127.0.0.1".into(),
                smtp_port: 11025,
            })
            .unwrap();
        secrets::save(email, "pass").unwrap();
    }
    drop(store);
    let (port, hits) = canary();
    deliver(user.clone(), vec![hostile_mail(port)]);

    let app = App::new().unwrap();
    // Both real, as in the app: two browsers on one profile, one with scripts and one without.
    let pane = Rc::new(Webview::new(dir.join("webview")));
    let editor = Rc::new(EditorView::new(dir.join("webview")));
    let controller = Controller::new(
        app.clone_strong(),
        &dir,
        None,
        Box::new(pane.clone()),
        Box::new(editor.clone()),
        Box::new(RecordingNotifier::default()),
        Arc::new(Offline),
    )
    .unwrap();
    controller.load_accounts().unwrap();
    controller.start_engine();
    app.show().unwrap();

    let driver = app.clone_strong();
    let (scene_dir, scene_user) = (dir.clone(), user.clone());
    slint::spawn_local(async move {
        scenario(driver, controller, editor, pane, scene_user, scene_dir, port, hits).await;
        slint::quit_event_loop().unwrap();
    })
    .unwrap();
    slint::run_event_loop().unwrap();
    let _ = secrets::delete(&user);
    let _ = secrets::delete(&second);
    std::process::exit(0);
}
