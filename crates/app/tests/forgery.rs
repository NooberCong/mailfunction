//! What a forged sender gets from the real UI and controller, in an offscreen window: no logo, no
//! lookup, none of the trust given to who it claims to be, and nothing sent anywhere by its arrival.

mod common;

use std::rc::Rc;
use std::sync::{Arc, Mutex};

use common::{RecordingEditor, RecordingNotifier, RecordingPane, fresh_dir, offscreen, settle};
use mailfunction::{App, Controller, ImageBar};
use mailfunction_core::compose;
use mailfunction_core::engine::Event;
use mailfunction_core::remote::{Logo, Network};
use mailfunction_core::store::{Account, AuthKind, Folder, LabelSpec, NewMessage, Role, Security, Store};
use slint::platform::WindowEvent;
use slint::{ComponentHandle, Model, PhysicalSize};

const NOW: i64 = 1_793_000_000;

/// Keeps count of what would have left the machine.
#[derive(Default)]
struct Watched {
    images: Mutex<Vec<String>>,
    logos: Mutex<Vec<String>>,
}

impl Watched {
    fn asked_for_logo(&self, domain: &str) -> bool {
        self.logos.lock().unwrap().iter().any(|asked| asked == domain)
    }
}

impl Network for Watched {
    fn image(&self, url: &str) -> Option<Vec<u8>> {
        self.images.lock().unwrap().push(url.to_string());
        None
    }

    fn logo(&self, domain: &str) -> Logo {
        self.logos.lock().unwrap().push(domain.to_string());
        Logo::Missing
    }
}

fn html_body(sender: &str) -> Vec<u8> {
    format!(
        "From: {sender}\r\nTo: me@example.com\r\nSubject: x\r\nContent-Type: text/html; charset=utf-8\r\n\r\n\
         <h1>News</h1><img src=\"https://t.example/p.gif\">\r\n"
    )
    .into_bytes()
}

fn tile() -> Vec<u8> {
    let (width, pixels) = (128, [200u8, 10, 30, 255].repeat(128 * 128));
    mailfunction_core::logo::png(pixels, width).unwrap()
}

struct World {
    app: App,
    store: Arc<Store>,
    controller: Rc<Controller>,
    inbox: Folder,
    spam: Folder,
    uid: std::cell::Cell<u32>,
}

impl World {
    /// Mail arriving from a sender, as sync would store it.
    fn arrive(&self, folder: &Folder, subject: &str, name: &str, address: &str, verified: bool) -> i64 {
        self.uid.set(self.uid.get() + 1);
        let message = NewMessage {
            uid: self.uid.get(),
            remote_id: format!("m{}", self.uid.get()),
            gm_thread_id: Some(u64::from(self.uid.get())),
            mid: None,
            references: vec![],
            subject: subject.into(),
            from_name: name.into(),
            from_addr: address.into(),
            verified,
            date: NOW + i64::from(self.uid.get()),
            size: 1,
            seen: false,
            flagged: false,
            labels: vec![],
        };
        let id = self.store.save_messages(folder, &[message]).unwrap()[0];
        self.store.save_body(id, &html_body(address)).unwrap();
        id
    }

    fn label(&self, name: &str) {
        let labels = self.app.get_labels();
        let row = (0..labels.row_count()).find(|&i| labels.row_data(i).unwrap().name == name).unwrap();
        self.app.invoke_select_label(row as i32);
        settle();
    }

    fn row(&self, subject: &str) -> usize {
        let rows = self.app.get_rows();
        (0..rows.row_count()).find(|&i| rows.row_data(i).unwrap().subject == subject).unwrap_or_else(|| {
            panic!("no row for {subject} in {:?}", rows.iter().map(|r| r.subject).collect::<Vec<_>>())
        })
    }

    fn open(&self, subject: &str) {
        self.controller.select_row(self.row(subject));
        settle();
    }

    fn logo_width(&self, subject: &str) -> u32 {
        self.app.get_rows().row_data(self.row(subject)).unwrap().logo.size().width
    }
}

#[test]
fn a_forgery_gets_nothing_that_a_genuine_sender_has_earned() {
    let window = offscreen();
    let dir = fresh_dir("forgery");
    let store = Arc::new(Store::open(&dir).unwrap());
    let app = App::new().unwrap();
    window.set_size(PhysicalSize::new(1280, 760));
    let pane = RecordingPane::default();
    let shown = pane.shown.clone();
    let notifier = RecordingNotifier::default();
    let notices = notifier.shown.clone();
    let editor = RecordingEditor::default();
    let opened = editor.opened.clone();
    let network = Arc::new(Watched::default());
    let controller = Controller::new(app.clone_strong(), &dir, None, Box::new(pane), Box::new(editor), Box::new(notifier), network.clone()).unwrap();
    app.show().unwrap();
    window.dispatch_event(WindowEvent::WindowActiveChanged(true));

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
    let spam = store.folder(account, "Spam", &LabelSpec::system(Role::Spam)).unwrap();
    let w = World { app: app.clone_strong(), store: store.clone(), controller: controller.clone(), inbox, spam, uid: Default::default() };

    let far_ahead = i64::MAX / 2;
    w.store.save_logo("brand.example", Some(&tile()), far_ahead).unwrap();
    w.arrive(&w.inbox, "Genuine", "Brand", "news@brand.example", true);
    w.arrive(&w.inbox, "Forged", "Brand", "news@brand.example", false);
    w.arrive(&w.inbox, "New forged", "Other", "x@forged.example", false);
    w.arrive(&w.inbox, "New genuine", "Other", "x@real.example", true);
    w.arrive(&w.inbox, "Posing", "security@brand.example", "evil@attacker.example", false);
    w.arrive(&w.inbox, "No address", "security@brand.example", "", false);
    controller.load_accounts().unwrap();
    settle();

    // The real brand's logo goes on its real mail, and never on a forgery of it.
    assert_eq!(w.logo_width("Genuine"), 38);
    assert_eq!(w.logo_width("Forged"), 0);
    w.open("Forged");
    assert_eq!(app.get_message().logo.size().width, 0);

    // Drawing a row asks a genuine sender's domain for a logo, and a forgery's never.
    w.logo_width("New forged");
    w.logo_width("New genuine");
    settle();
    assert!(network.asked_for_logo("real.example"));
    assert!(!network.asked_for_logo("forged.example"), "a forged sender's domain was contacted");

    // A name dressed up as an address gives way to the real one.
    let sender = |subject: &str| app.get_rows().row_data(w.row(subject)).unwrap().sender;
    assert_eq!(sender("Posing"), "evil@attacker.example");
    assert_eq!(sender("No address"), "(no address)");
    assert_eq!(sender("Genuine"), "Brand");

    // Only genuine mail may vouch for its sender, and then only genuine mail benefits.
    w.open("Forged");
    assert_eq!((app.get_image_bar(), app.get_can_always_load()), (ImageBar::Blocked, false));
    w.open("Genuine");
    assert_eq!((app.get_image_bar(), app.get_can_always_load()), (ImageBar::Blocked, true));
    app.invoke_always_load_images();
    assert_eq!(app.get_image_bar(), ImageBar::Allowed);
    w.open("Forged");
    assert_eq!(app.get_image_bar(), ImageBar::Blocked, "a forgery of an allowed sender loaded images");
    assert!(!shown.borrow().last().unwrap().contains("message.invalid/image"));
    w.open("Genuine");
    assert_eq!(app.get_image_bar(), ImageBar::Allowed);
    assert!(shown.borrow().last().unwrap().contains("message.invalid/image"));

    // Spam is where forgeries that fail end up, so nothing there is believed, verified or not.
    w.arrive(&w.spam, "Spammed", "Brand", "news@brand.example", true);
    controller.on_event(account, Event::Changed);
    settle();
    w.label("Spam");
    w.open("Spammed");
    assert_eq!((app.get_image_bar(), app.get_can_always_load()), (ImageBar::Blocked, false));
    assert!(!shown.borrow().last().unwrap().contains("message.invalid/image"));
    w.label("Inbox");

    // Mail arriving tells nobody: a notice uses a logo that is held, and never goes to find one.
    let arrived = w.arrive(&w.inbox, "Arrived", "New", "hello@arrival.example", true);
    let known = w.arrive(&w.inbox, "Known", "Brand", "news@brand.example", true);
    let forged = w.arrive(&w.inbox, "Forged arrival", "Brand", "news@brand.example", false);
    controller.on_event(account, Event::Arrived(vec![arrived, known, forged]));
    settle();
    assert_eq!(notices.borrow().len(), 3);
    assert!(!network.asked_for_logo("arrival.example"), "mail arriving contacted the sender's domain");
    let held = tile_in_notice(&notices.borrow()[1]);
    let initials = tile_in_notice(&notices.borrow()[0]);
    assert_ne!(held, initials, "the genuine sender's held logo shows, and the forgery's initials do");
    assert!(network.images.lock().unwrap().is_empty(), "something fetched an image");

    // A mailto link starts a message and goes no further than what the composer can show.
    w.open("Known");
    let link = "mailto:a@x.example?subject=Hi&body=Line%20one&bcc=evil@x.example&attach=C:%5Csecret.txt";
    controller.compose_to(compose::from_mailto(link).unwrap());
    settle();
    assert!(app.get_composing());
    assert_eq!((app.get_draft_to().as_str(), app.get_draft_subject().as_str(), app.get_draft_bcc().as_str()), ("a@x.example", "Hi", ""));
    assert_eq!(app.get_draft_files().row_count(), 0);
    assert!(opened.borrow().last().unwrap().contains("Line one"), "{}", opened.borrow().last().unwrap());

    drop(w);
    drop(controller);
    drop(store);
    let _ = std::fs::remove_dir_all(&dir);
}

fn tile_in_notice(notice: &mailfunction::notify::Notice) -> Vec<u8> {
    notice.picture.clone().unwrap()
}
