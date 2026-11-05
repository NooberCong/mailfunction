//! The pictures in the README, taken from the real window beside the real WebView2. The mail is
//! made up and lives in a throwaway folder, so nothing of anyone's can appear. Sender logos come
//! from the web, as they do in the app. Run it once for each theme:
//!
//! ```powershell
//! cargo test -p mailfunction --test showcase
//! cargo test -p mailfunction --test showcase -- dark
//! ```

mod common;

use std::path::Path;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use common::RecordingNotifier;
use common::server::{client_pixels, sleep, until};
use mailfunction::editor::EditorView;
use mailfunction::webview::Webview;
use mailfunction::{App, Controller, Pane, Verb, chrome};
use mailfunction_core::remote::{Logo, Network, Web};
use mailfunction_core::settings::{Appearance, Settings};
use mailfunction_core::store::{Account, AuthKind, LabelSpec, NewMessage, Role, Security, Store};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use slint::platform::WindowEvent;
use slint::{ComponentHandle, Model};

const ACCOUNTS: [&str; 2] = ["alex.rivera@example.com", "alex@example.org"];
const CORNER: f32 = 18.0;
const OPENED: &str = "Launch plan for Tuesday";

struct Mail {
    account: usize,
    from: (&'static str, &'static str),
    subject: &'static str,
    minutes_ago: i64,
    seen: bool,
    flagged: bool,
    labels: &'static [&'static str],
    body: &'static str,
}

const fn mail(account: usize, from: (&'static str, &'static str), subject: &'static str, minutes_ago: i64, body: &'static str) -> Mail {
    Mail { account, from, subject, minutes_ago, seen: true, flagged: false, labels: &[], body }
}

const PRIYA: &str = "Hi Alex,\r\n\r\n\
Here is where we landed for Tuesday. Design signed off this morning, and the pricing page goes live with the announcement at 9:00 AM.\r\n\r\n\
- Marketing sends the launch email at 9:15\r\n\
- Support has the new help articles ready\r\n\
- Jonas is on call for the first two hours\r\n\r\n\
The plan and the slides are attached. Could you take a last look before tomorrow's standup?\r\n\r\n\
Thanks,\r\nPriya";

const MAIL: &[Mail] = &[
    Mail { seen: false, flagged: true, labels: &["Launch"], ..mail(0, ("Priya Raman", "priya@northwind.example"), OPENED, 6, PRIYA) },
    Mail { seen: false, ..mail(0, ("GitHub", "notifications@github.com"), "[northwind/web] Fix checkout redirect on Safari (#482)", 24, "<p>Jonas requested your review on this pull request.</p>") },
    Mail { seen: false, ..mail(0, ("Figma", "no-reply@figma.com"), "Maya commented on Pricing page v3", 41, "<p>“Love the new layout. Can we try the toggle above the plans?”</p>") },
    Mail { seen: false, labels: &["Travel"], ..mail(1, ("Airbnb", "automated@airbnb.com"), "Your stay in Lisbon is coming up", 58, "<p>Check-in is on Friday from 3:00 PM.</p>") },
    mail(0, ("Stripe", "notifications@stripe.com"), "Your payout of $4,218.60 is on the way", 95, "<p>Expected in your bank account by Monday.</p>"),
    mail(0, ("Linear", "notifications@linear.app"), "3 issues were assigned to you this week", 140, "<p>Here is what is on your plate.</p>"),
    mail(1, ("Spotify", "no-reply@spotify.com"), "New music from artists you follow", 210, "<p>Fresh releases, picked for you.</p>"),
    mail(0, ("Notion", "notify@notion.so"), "Q4 roadmap was updated", 320, "<p>Maya edited 4 pages in Product.</p>"),
    mail(0, ("Vercel", "notifications@vercel.com"), "Production deployment ready for northwind-web", 460, "<p>Your deployment finished in 48 seconds.</p>"),
    mail(1, ("Uber", "noreply@uber.com"), "Your morning trip with Uber", 1500, "<p>Thanks for riding with us.</p>"),
    mail(0, ("Jonas Weber", "jonas@northwind.example"), "Re: On-call for launch day", 1620, "<p>Works for me, I'll take the first shift.</p>"),
    mail(1, ("Dropbox", "no-reply@dropbox.com"), "Sam shared “Lisbon photos” with you", 2900, "<p>Open the folder to see 48 new photos.</p>"),
    mail(1, ("Duolingo", "hello@duolingo.com"), "You're on a 30 day streak!", 4400, "<p>Keep it going with a quick lesson.</p>"),
];

const REPLY: &str = "<p>Thanks Priya, this looks great.</p>\
<p>I'll go through the slides tonight and leave any notes in the doc. One thing to check: can we move the \
launch email to <b>9:30</b>, so the pricing page has time to settle first?</p><p>Alex</p>";

fn now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs() as i64
}

fn window(app: &App) -> isize {
    let handle = app.window().window_handle();
    let RawWindowHandle::Win32(handle) = handle.window_handle().unwrap().as_raw() else { panic!("not Win32") };
    handle.hwnd.get()
}

/// A whole message as it would arrive. Attachments are filler, there for their name and size.
fn raw(mail: &Mail) -> Vec<u8> {
    let (name, address) = mail.from;
    let head = format!("From: {name} <{address}>\r\nTo: Alex Rivera <{}>\r\nSubject: {}\r\nMIME-Version: 1.0\r\n", ACCOUNTS[mail.account], mail.subject);
    if mail.subject != OPENED {
        return format!("{head}Content-Type: text/html; charset=utf-8\r\n\r\n{}\r\n", mail.body).into_bytes();
    }
    // Plain text takes the app's theme, where HTML mail keeps the white page it was written for.
    let text = format!("Content-Type: text/plain; charset=utf-8\r\n\r\n{}\r\n", mail.body);
    let mut message = format!("{head}Content-Type: multipart/mixed; boundary=b\r\n\r\n--b\r\n{text}");
    for (file, size) in [("Launch plan.pdf", 1_840_000), ("Launch slides.pptx", 6_200_000)] {
        let filler = "x".repeat(size);
        message += &format!("--b\r\nContent-Type: application/octet-stream\r\nContent-Disposition: attachment; filename=\"{file}\"\r\n\r\n{filler}\r\n");
    }
    (message + "--b--\r\n").into_bytes()
}

fn seed(dir: &Path, dark: bool) {
    let store = Store::open(dir).unwrap();
    store.save_settings(&Settings { appearance: if dark { Appearance::Dark } else { Appearance::Light }, ..Settings::default() }).unwrap();
    // Both accounts are Gmail in all but name, as a real address cannot go in a picture.
    if let Logo::Found(google) = Web::default().logo("google.com") {
        for domain in ["example.com", "example.org"] {
            store.save_logo(domain, Some(&google), now()).unwrap();
        }
    }
    for (index, email) in ACCOUNTS.into_iter().enumerate() {
        let account = store
            .add_account(&Account {
                id: 0,
                email: email.into(),
                host: "localhost".into(),
                port: 993,
                security: Security::Tls,
                auth: AuthKind::Google,
                smtp_host: "localhost".into(),
                smtp_port: 25,
            })
            .unwrap();
        let inbox = store.folder(account, "INBOX", &LabelSpec::system(Role::Inbox)).unwrap();
        for (folder, role) in [("Starred", Role::Starred), ("Sent", Role::Sent), ("Drafts", Role::Drafts), ("All Mail", Role::All), ("Spam", Role::Spam), ("Trash", Role::Trash)] {
            store.folder(account, folder, &LabelSpec::system(role)).unwrap();
        }
        let mine: Vec<&Mail> = MAIL.iter().filter(|mail| mail.account == index).collect();
        let messages: Vec<NewMessage> = mine
            .iter()
            .enumerate()
            .map(|(i, mail)| NewMessage {
                uid: i as u32 + 1,
                remote_id: format!("m{i}"),
                gm_thread_id: Some(i as u64 + 1),
                mid: Some(format!("m{i}@example.com")),
                references: vec![],
                subject: mail.subject.into(),
                from_name: mail.from.0.into(),
                from_addr: mail.from.1.into(),
                verified: true,
                date: now() - mail.minutes_ago * 60,
                size: 40_000,
                seen: mail.seen,
                flagged: mail.flagged,
                labels: mail.labels.iter().map(|label| LabelSpec::user(*label)).collect(),
            })
            .collect();
        for (id, mail) in store.save_messages(&inbox, &messages).unwrap().into_iter().zip(&mine) {
            store.save_body(id, &raw(mail)).unwrap();
        }
    }
}

/// How much of a pixel at this place lies inside the window's rounded corners.
fn coverage(x: usize, y: usize, width: usize, height: usize, radius: f32) -> f32 {
    let inset = |at: usize, size: usize| (radius - (at as f32 + 0.5).min(size as f32 - at as f32 - 0.5)).max(0.0);
    let reach = inset(x, width).hypot(inset(y, height));
    (radius - reach + 0.5).clamp(0.0, 1.0)
}

async fn shot(app: &App, name: &str) {
    // The real pointer may be anywhere, and a highlighted row says nothing.
    app.window().dispatch_event(WindowEvent::PointerExited);
    sleep(400).await;
    let (width, height, mut pixels) = client_pixels(window(app)).unwrap();
    let (w, h) = (width as usize, height as usize);
    let radius = CORNER * app.window().scale_factor();
    for (at, pixel) in pixels.as_chunks_mut::<4>().0.iter_mut().enumerate() {
        pixel.swap(0, 2);
        pixel[3] = (coverage(at % w, at / w, w, h, radius) * 255.0) as u8;
    }
    let file = std::fs::File::create(format!("../../docs/images/{name}.png")).unwrap();
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), width as u32, height as u32);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.set_compression(png::Compression::High);
    encoder.write_header().unwrap().write_image_data(&pixels).unwrap();
    println!("saved {name}");
}

async fn scenario(app: App, controller: Rc<Controller>, editor: Rc<EditorView>, suffix: &str) {
    until("the message list", || app.get_rows().row_count() == MAIL.len()).await;
    // Rows ask for their logos as they are drawn, and the web takes a moment to answer.
    sleep(9000).await;
    let opened = app.get_rows().iter().position(|row| row.subject == OPENED).unwrap();
    controller.select_row(opened);
    until("the message", || app.get_pane() != Pane::Empty).await;
    sleep(2500).await;
    shot(&app, &format!("inbox{suffix}")).await;

    app.invoke_verb(Verb::Reply);
    until("the editor on screen", || editor.is_on_screen()).await;
    sleep(1000).await;
    let script = format!("editor.load({});", serde_json::to_string(REPLY).unwrap());
    editor.with_view(|view| view.evaluate_script(&script)).unwrap().unwrap();
    sleep(1500).await;
    shot(&app, &format!("compose{suffix}")).await;
}

fn main() {
    let dark = std::env::args().any(|argument| argument == "dark");
    let dir = std::env::temp_dir().join(format!("mailfunction-showcase-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::create_dir_all("../../docs/images").unwrap();
    seed(&dir, dark);

    let app = App::new().unwrap();
    let pane = Rc::new(Webview::new(dir.join("webview")));
    let editor = Rc::new(EditorView::new(dir.join("webview")));
    let notifier = Box::new(RecordingNotifier::default());
    let controller = Controller::new(app.clone_strong(), &dir, None, Box::new(pane), Box::new(editor.clone()), notifier, Arc::new(Web::default())).unwrap();
    controller.load_accounts().unwrap();
    app.window().set_size(slint::LogicalSize::new(1280.0, 800.0));
    chrome::watch_window(&app);
    app.show().unwrap();

    let driver = app.clone_strong();
    let suffix = if dark { "-dark" } else { "" };
    slint::spawn_local(async move {
        scenario(driver, controller, editor, suffix).await;
        slint::quit_event_loop().unwrap();
    })
    .unwrap();
    slint::run_event_loop().unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    std::process::exit(0);
}
