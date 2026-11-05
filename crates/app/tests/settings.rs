//! Every setting, from the control on the settings screen to what it changes, on the real UI.

mod common;

use std::rc::Rc;
use std::sync::{Arc, Mutex};

use common::{RecordingNotifier, RecordingPane, click, fresh_dir, linger, offscreen, press, settle, shot, typed, with_control};
use mailfunction::{App, Controller, ImageBar, Pane, Theme};
use mailfunction_core::engine::Event;
use mailfunction_core::remote::{Logo, Network};
use mailfunction_core::settings::{Appearance, MarkRead, Settings, TextSize};
use mailfunction_core::store::{Account, AuthKind, Folder, LabelSpec, NewMessage, Reader, Role, Security, Store};
use slint::platform::WindowEvent;
use slint::{ComponentHandle, Model, PhysicalSize};

const ESCAPE: &str = "\u{1b}";
const DOWN: &str = "\u{f701}";
const LEFT_OF_CONTROLS: f32 = 400.0;
const IMAGE: &str = "https://t.example/p.gif";

/// Keeps count of what would have left the machine.
#[derive(Default)]
struct Watched {
    images: Mutex<Vec<String>>,
    logos: Mutex<Vec<String>>,
}

const GIF: &[u8] = b"GIF89a";

impl Network for Watched {
    fn image(&self, url: &str) -> Option<Vec<u8>> {
        self.images.lock().unwrap().push(url.to_string());
        None
    }

    fn logo(&self, domain: &str) -> Logo {
        self.logos.lock().unwrap().push(domain.to_string());
        Logo::Unknown
    }
}

fn html_body(sender: &str) -> Vec<u8> {
    format!(
        "From: {sender}\r\nTo: kong@example.com\r\nSubject: x\r\nMessage-ID: <{sender}@x>\r\nContent-Type: text/html; charset=utf-8\r\n\r\n\
         <h1>News</h1><img src=\"{IMAGE}\">\r\n"
    )
    .into_bytes()
}

fn plain_body(subject: &str) -> Vec<u8> {
    format!(
        "From: Huy <huy@example.vn>\r\nTo: kong@example.com\r\nSubject: {subject}\r\nMessage-ID: <{subject}@x>\r\n\
         Content-Type: text/plain; charset=utf-8\r\n\r\nHello there\r\n"
    )
    .into_bytes()
}

fn mail(uid: u32, subject: &str, from: &str, hours_ago: i64, labels: Vec<LabelSpec>) -> NewMessage {
    NewMessage {
        uid,
        remote_id: format!("m{uid}"),
        gm_thread_id: Some(u64::from(uid)),
        mid: Some(format!("m{uid}@x")),
        references: vec![],
        subject: subject.into(),
        from_name: "Someone".into(),
        from_addr: from.into(),
        date: 1_793_000_000 - hours_ago * 3600,
        size: 100,
        seen: false,
        flagged: false,
        labels,
    }
}

struct World {
    app: App,
    store: Arc<Store>,
    reader: Reader,
    controller: Rc<Controller>,
    notices: Rc<std::cell::RefCell<Vec<mailfunction::notify::Notice>>>,
    network: Arc<Watched>,
    shown: Rc<std::cell::RefCell<Vec<String>>>,
    account: i64,
    inbox: Folder,
}

impl World {
    fn page(&self, index: i32) {
        self.app.set_settings_page(index);
        settle();
    }

    fn settings(&self) -> Settings {
        self.reader.settings()
    }

    fn row_of(&self, subject: &str) -> usize {
        let rows = self.app.get_rows();
        (0..rows.row_count()).find(|&i| rows.row_data(i).unwrap().subject == subject).unwrap()
    }

    fn open(&self, subject: &str) {
        self.controller.select_row(self.row_of(subject));
        settle();
    }

    fn spam_row(&self) -> usize {
        let labels = self.app.get_labels();
        (0..labels.row_count()).find(|&i| labels.row_data(i).unwrap().name == "Spam").unwrap()
    }

    fn inbox_row(&self) -> usize {
        let labels = self.app.get_labels();
        (0..labels.row_count()).find(|&i| labels.row_data(i).unwrap().name == "Inbox").unwrap()
    }

    fn set(&self, change: impl FnOnce(&mut Settings)) {
        let theme = self.app.global::<Theme>();
        let mut settings = Settings::from(theme.get_preferences());
        change(&mut settings);
        theme.set_preferences(settings.into());
        settle();
        assert_eq!(self.settings(), settings, "the change was not saved");
    }
}

fn account() -> Account {
    Account {
        id: 0,
        email: "kong@example.com".into(),
        host: "localhost".into(),
        port: 993,
        security: Security::Tls,
        auth: AuthKind::Google,
        smtp_host: "localhost".into(),
        smtp_port: 25,
    }
}

fn world(window: &slint::platform::software_renderer::MinimalSoftwareWindow) -> World {
    let dir = fresh_dir("settings");
    let store = Store::open(&dir).unwrap();
    let account = store.add_account(&account()).unwrap();
    let inbox = store.folder(account, "INBOX", &LabelSpec::system(Role::Inbox)).unwrap();
    let spam = store.folder(account, "Spam", &LabelSpec::system(Role::Spam)).unwrap();
    store.folder(account, "Drafts", &LabelSpec::system(Role::Drafts)).unwrap();
    store.folder(account, "Trash", &LabelSpec::system(Role::Trash)).unwrap();
    store
        .save_messages(
            &inbox,
            &[
                mail(1, "Newsletter", "news@brand.example", 1, vec![]),
                mail(2, "Second", "huy@example.vn", 2, vec![]),
                mail(3, "Third", "huy@example.vn", 3, vec![]),
                mail(4, "Fourth", "huy@example.vn", 4, vec![]),
            ],
        )
        .unwrap();
    store.save_messages(&spam, &[mail(5, "Win a prize", "spam@bad.example", 1, vec![])]).unwrap();
    let reader = Reader::open(&dir).unwrap();
    let label = inbox.label_id;
    for row in reader.messages(&[label], 0, 10).unwrap() {
        let raw = if row.subject == "Newsletter" { html_body(&row.from_addr) } else { plain_body(&row.subject) };
        store.save_body(row.id, &raw).unwrap();
    }
    let junk = reader.message_at(account, "Spam", 5).unwrap().unwrap();
    store.save_body(junk, &html_body("spam@bad.example")).unwrap();

    let app = App::new().unwrap();
    window.set_size(PhysicalSize::new(1280, 760));
    let pane = RecordingPane::default();
    let shown = pane.shown.clone();
    let notifier = RecordingNotifier::default();
    let notices = notifier.shown.clone();
    let network = Arc::new(Watched::default());
    let controller = Controller::new(app.clone_strong(), &dir, None, Box::new(pane), Box::new(common::RecordingEditor::default()), Box::new(notifier), network.clone()).unwrap();
    app.show().unwrap();
    window.dispatch_event(WindowEvent::WindowActiveChanged(true));
    controller.load_accounts().unwrap();
    settle();
    World { app, store: Arc::new(store), reader, controller, notices, network, shown, account, inbox }
}

/// The screen itself: getting there and back, and how each page looks.
fn the_screen(w: &World) {
    w.open("Second");
    assert_eq!(w.app.get_pane(), Pane::Text);

    // The sidebar's last line.
    click(&w.app, 60.0, 734.0);
    assert!(w.app.get_settings_open());
    assert_eq!(w.app.get_pane(), Pane::Empty, "the message stayed under the screen");
    for (page, name) in [(0, "general"), (1, "mail"), (2, "notifications"), (3, "accounts")] {
        w.page(page);
        shot(&w.app, &format!("40-settings-{name}"));
    }
    w.set(|s| s.appearance = Appearance::Dark);
    for (page, name) in [(0, "general"), (1, "mail"), (2, "notifications"), (3, "accounts")] {
        w.page(page);
        shot(&w.app, &format!("40-settings-{name}-dark"));
    }
    w.set(|s| s.appearance = Appearance::System);
    settle();

    // Keys move between pages, and letters do nothing to the mail behind.
    w.page(0);
    press(&w.app, DOWN);
    assert_eq!(w.app.get_settings_page(), 1);
    press(&w.app, "k");
    assert_eq!(w.app.get_settings_page(), 0);
    press(&w.app, "e");
    assert_eq!(w.app.get_notice(), "");

    // Escape goes back to the message that was open.
    press(&w.app, ESCAPE);
    assert!(!w.app.get_settings_open());
    assert_eq!(w.app.get_pane(), Pane::Text);
    assert_eq!(w.app.get_message().subject, "Second");

    // The same key opens and closes it, from anywhere.
    with_control(&w.app, ",");
    assert!(w.app.get_settings_open());
    with_control(&w.app, ",");
    assert!(!w.app.get_settings_open());

    // What was being written is safe in Drafts before the screen covers it.
    press(&w.app, "c");
    assert!(w.app.get_composing());
    typed(&w.app, "ann@example.com");
    with_control(&w.app, ",");
    assert!(!w.app.get_composing() && w.app.get_settings_open());
    press(&w.app, ESCAPE);
}

fn images(w: &World) {
    w.open("Newsletter");
    assert_eq!(w.app.get_image_bar(), ImageBar::Blocked);
    assert!(w.network.images.lock().unwrap().is_empty());

    // The switch on the screen, by its pixels: first line of the Mail page.
    let shown = w.shown.borrow().len();
    w.controller.open_settings();
    w.page(1);
    click(&w.app, LEFT_OF_CONTROLS, 95.0);
    assert!(w.settings().show_all_images, "the click did not reach the switch");
    shot(&w.app, "41-all-images-on");
    press(&w.app, ESCAPE);

    // Back at the message the page goes up at once, with no bar coming and going around it.
    assert_eq!((w.app.get_pane(), w.app.get_image_bar()), (Pane::Html, ImageBar::Hidden));
    assert!(w.shown.borrow()[shown..].last().unwrap().contains("https://message.invalid/image/0"));
    let key = w.controller.page_key().unwrap();
    w.controller.image_fetched(key, 1, Some(GIF.to_vec()));
    assert_eq!(w.app.get_image_bar(), ImageBar::Hidden);

    // A failure can still be retried, once nothing at all has come.
    w.app.invoke_load_images();
    let key = w.controller.page_key().unwrap();
    w.controller.image_fetched(key, 2, None);
    assert_eq!(w.app.get_image_bar(), ImageBar::Failed);

    // Not in Spam, and that does not depend on how the message was reached.
    w.app.invoke_select_label(w.spam_row() as i32);
    settle();
    w.open("Win a prize");
    assert_eq!(w.app.get_image_bar(), ImageBar::Blocked);
    w.app.invoke_select_label(w.inbox_row() as i32);
    settle();

    // Off again, each sender is back to being asked about.
    w.set(|s| s.show_all_images = false);
    w.open("Newsletter");
    assert_eq!(w.app.get_image_bar(), ImageBar::Blocked);
}

fn reading(w: &World) {
    // Compact rows, and bigger text, take effect at once and move nothing else.
    let theme = w.app.global::<Theme>();
    assert_eq!(theme.get_row(), 68.0);
    w.set(|s| s.compact_list = true);
    assert_eq!(theme.get_row(), 56.0);
    shot(&w.app, "42-compact-list");
    w.set(|s| s.compact_list = false);
    assert_eq!(theme.get_reading(), 14.0);
    w.set(|s| s.text_size = TextSize::Large);
    assert_eq!(theme.get_reading(), 16.0);
    w.set(|s| s.text_size = TextSize::Small);
    assert_eq!(theme.get_reading(), 13.0);
    w.set(|s| s.text_size = TextSize::Medium);

    // Appearance overrides the system and gives it back.
    w.set(|s| s.appearance = Appearance::Dark);
    assert!(theme.get_dark());
    w.set(|s| s.appearance = Appearance::Light);
    assert!(!theme.get_dark());
    w.set(|s| s.appearance = Appearance::System);

    // Marking as read: never means never.
    let unread = |subject: &str| w.app.get_rows().row_data(w.row_of(subject)).unwrap().unread;
    w.set(|s| s.mark_read = MarkRead::Never);
    w.open("Third");
    linger();
    assert!(unread("Third"));
    // After a while means not yet.
    w.set(|s| s.mark_read = MarkRead::AfterAWhile);
    w.open("Fourth");
    linger();
    assert!(unread("Fourth"));
    w.set(|s| s.mark_read = MarkRead::OnOpening);
    w.open("Third");
    linger();
    assert!(!unread("Third"));

    // Next message: on, the next one opens. Off, the reader empties.
    let rows = w.app.get_rows().row_count();
    w.open("Third");
    press(&w.app, "#");
    assert_eq!((w.app.get_rows().row_count(), w.app.get_pane()), (rows - 1, Pane::Text));
    assert_eq!(w.app.get_message().subject, "Fourth");
    press(&w.app, "z");
    settle();
    w.set(|s| s.open_next = false);
    w.open("Third");
    press(&w.app, "#");
    assert_eq!((w.app.get_rows().row_count(), w.app.get_pane()), (rows - 1, Pane::Empty));
    press(&w.app, "z");
    settle();
    w.set(|s| s.open_next = true);
}

fn logos(w: &World) {
    w.network.logos.lock().unwrap().clear();
    w.store.save_logo("brand.example", Some(&logo_png()), i64::MAX / 2).unwrap();
    w.controller.logo_ready("brand.example");
    settle();
    let logo = || w.app.get_rows().row_data(w.row_of("Newsletter")).unwrap().logo.size().width;
    assert!(logo() > 0);

    w.set(|s| s.sender_logos = false);
    assert_eq!(logo(), 0, "the logo stayed on screen");
    w.set(|s| s.sender_logos = true);
    assert!(logo() > 0);
}

fn logo_png() -> Vec<u8> {
    let mut bytes = Vec::new();
    let mut encoder = png::Encoder::new(&mut bytes, 128, 128);
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.write_header().unwrap().write_image_data(&[90u8, 100, 200].repeat(128 * 128)).unwrap();
    bytes
}

fn notifications(w: &World) {
    let fresh = w.store.save_messages(&w.inbox, &[mail(6, "Fresh", "huy@example.vn", 0, vec![])]).unwrap()[0];
    let arrives = || {
        w.notices.borrow_mut().clear();
        w.controller.on_event(w.account, Event::Arrived(vec![fresh]));
    };

    arrives();
    assert_eq!(w.notices.borrow()[0].text, "Fresh");
    assert!(!w.notices.borrow()[0].silent);

    w.set(|s| s.notification_sound = false);
    arrives();
    assert!(w.notices.borrow()[0].silent);
    w.set(|s| s.notification_sound = true);

    // Nothing of the message shows, and a click still finds it.
    w.set(|s| s.notification_preview = false);
    arrives();
    let notice = w.notices.borrow()[0].clone();
    assert_eq!((notice.title.as_str(), notice.text.as_str(), notice.picture), ("New email", "", None));
    assert_eq!(notice.opens.message, Some(fresh));
    w.set(|s| s.notification_preview = true);

    // The taskbar keeps quiet, without the count going anywhere.
    w.controller.refresh_now();
    assert!(w.controller.taskbar_count() > 0);
    w.set(|s| s.taskbar_count = false);
    assert_eq!(w.controller.taskbar_count(), 0);
    assert!(w.controller.unread() > 0);
    w.set(|s| s.taskbar_count = true);

    w.set(|s| s.notifications = false);
    arrives();
    assert!(w.notices.borrow().is_empty());
    w.set(|s| s.notifications = true);

    // The account's own switch, by its pixels.
    let account_switch = || {
        w.controller.open_settings();
        w.page(3);
        click(&w.app, LEFT_OF_CONTROLS, 322.0);
        w.controller.close_settings();
    };
    account_switch();
    assert!(!w.reader.identity(w.account).unwrap().notify, "the click did not reach the switch");
    arrives();
    assert!(w.notices.borrow().is_empty(), "a muted account interrupted");
    account_switch();
    assert!(w.reader.identity(w.account).unwrap().notify);
}

fn identity(w: &World) {
    w.controller.open_settings();
    w.page(3);
    w.app.invoke_identity_edited(0, "Kong N.".into(), "Kong\nRocket".into(), true);
    // Written after a pause, without having to leave the page.
    linger();
    linger();
    let saved = w.reader.identity(w.account).unwrap();
    assert_eq!((saved.name.as_str(), saved.signature.as_str()), ("Kong N.", "Kong\nRocket"));
    press(&w.app, ESCAPE);

    // Coming back, the page shows what was written, with no sync needed to bring it up to date.
    let account = w.app.get_accounts().row_data(0).unwrap();
    assert_eq!((account.name.as_str(), account.signature.as_str()), ("Kong N.", "Kong\nRocket"));
    w.controller.open_settings();
    w.page(3);
    settle();
    shot(&w.app, "43-account-filled");
    press(&w.app, ESCAPE);

    // A new message starts signed, with the cursor above it.
    press(&w.app, "c");
    assert_eq!(w.controller.draft_text(), "\n\n-- \nKong\nRocket");
    shot(&w.app, "44-signed-compose");

    // Nothing written means nothing kept, signature or not.
    let drafts = |w: &World| {
        let labels = w.reader.labels(w.account).unwrap();
        let drafts = labels.iter().find(|l| l.role == Role::Drafts).unwrap();
        w.reader.message_count(&[drafts.id]).unwrap()
    };
    let before = drafts(w);
    press(&w.app, ESCAPE);
    assert_eq!(drafts(w), before);

    // Above the quote in a reply.
    w.open("Second");
    press(&w.app, "r");
    let body = w.controller.draft_text();
    let (signature, quote) = (body.find("-- \nKong").unwrap(), body.find("> Hello there").unwrap());
    assert!(signature < quote, "{body}");
    press(&w.app, ESCAPE);

    // Mail goes out under the chosen name. The live test sees it arrive that way.
    press(&w.app, "c");
    w.app.set_draft_to("ann@example.com".into());
    w.app.set_draft_subject("From the new name".into());
    let draft = w.controller.save_draft().unwrap();
    assert_eq!(w.reader.rows(&[draft]).unwrap()[0].from_name, "Kong N.");
    w.controller.finish_writing();
}

fn undo_send(w: &World) {
    let send = |w: &World| {
        press(&w.app, "c");
        w.app.set_draft_to("ann@example.com".into());
        w.app.set_draft_subject("Now".into());
        w.controller.send_draft();
    };
    w.set(|s| s.undo_send_seconds = 0);
    send(w);
    assert_eq!((w.app.get_notice().as_str(), w.app.get_notice_action().as_str()), ("Sending", ""));
    press(&w.app, "z");
    assert!(!w.app.get_composing(), "there was nothing to take back");

    w.set(|s| s.undo_send_seconds = 30);
    send(w);
    assert_eq!(w.app.get_notice_action(), "Undo");
    press(&w.app, "z");
    settle();
    assert!(w.app.get_composing(), "undo did not bring it back");
    w.controller.finish_writing();
    w.set(|s| s.undo_send_seconds = 10);
}

fn start_with_windows(w: &World) {
    let heard = Rc::new(std::cell::RefCell::new(Vec::new()));
    let recorded = heard.clone();
    w.app.on_start_with_windows_changed(move |on| recorded.borrow_mut().push(on));
    w.set(|s| s.start_with_windows = true);
    w.set(|s| s.compact_list = true);
    w.set(|s| s.compact_list = false);
    w.set(|s| s.start_with_windows = false);
    assert_eq!(*heard.borrow(), [true, false], "only a change of this setting should reach Windows");
}

/// Removing the last account leaves no screen to stand on but the sign-in.
fn the_last_account(w: &World) {
    w.controller.open_settings();
    w.page(3);
    w.controller.remove_account(0);
    settle();
    assert!(!w.app.get_settings_open() && !w.app.get_signed_in());
}

#[test]
fn every_setting() {
    let window = offscreen();
    let w = world(&window);
    the_screen(&w);
    images(&w);
    reading(&w);
    logos(&w);
    notifications(&w);
    identity(&w);
    undo_send(&w);
    start_with_windows(&w);

    // What was chosen is there next time.
    let dir = fresh_dir("settings");
    let next = Reader::open(&dir).unwrap();
    assert_eq!(next.settings(), w.settings());

    window.set_size(PhysicalSize::new(900, 480));
    w.controller.open_settings();
    w.page(1);
    shot(&w.app, "45-settings-smallest");
    w.controller.close_settings();
    the_last_account(&w);
}
