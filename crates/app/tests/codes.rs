//! Login codes through the real controller and UI, in an offscreen window: who gets the button on
//! a notification, what a held notification waits for, and the bar over an open message.

mod common;

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use common::{RecordingEditor, RecordingNotifier, RecordingPane, fresh_dir, offscreen, settle, shot};
use mailfunction::notify::{Notice, Opens};
use mailfunction::{App, Controller, Theme};
use mailfunction_core::engine::Event;
use mailfunction_core::remote::{Logo, Network};
use mailfunction_core::settings::Settings;
use mailfunction_core::store::{Account, AuthKind, Folder, LabelSpec, NewMessage, Role, Security, Store};
use slint::ComponentHandle;

struct Offline;

impl Network for Offline {
    fn image(&self, _: &str) -> Option<Vec<u8>> {
        None
    }

    fn logo(&self, _: &str) -> Logo {
        Logo::Missing
    }
}

fn now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs() as i64
}

fn body(text: &str) -> Vec<u8> {
    format!("From: a@brand.example\r\nTo: me@example.com\r\nSubject: x\r\nContent-Type: text/plain; charset=utf-8\r\n\r\n{text}\r\n").into_bytes()
}

const CODE_MAIL: &str = "Enter this on the page to confirm it is you: 482913\nThis code will expire in 15 minutes.";

struct World {
    app: App,
    store: Arc<Store>,
    controller: Rc<Controller>,
    account: i64,
    inbox: Folder,
    notices: Rc<RefCell<Vec<Notice>>>,
    copied: Rc<RefCell<Vec<String>>>,
    uid: Cell<u32>,
}

impl World {
    fn new(name: &str) -> Self {
        let dir = fresh_dir(name);
        let store = Arc::new(Store::open(&dir).unwrap());
        let app = App::new().unwrap();
        let notifier = RecordingNotifier::default();
        let (notices, copied) = (notifier.shown.clone(), notifier.copied.clone());
        let pane = Box::new(RecordingPane::default());
        let editor = Box::new(RecordingEditor::default());
        let controller = Controller::new(app.clone_strong(), &dir, None, pane, editor, Box::new(notifier), Arc::new(Offline)).unwrap();
        app.show().unwrap();
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
        controller.load_accounts().unwrap();
        settle();
        Self { app, store, controller, account, inbox, notices, copied, uid: Cell::default() }
    }

    fn message(&self, subject: &str, verified: bool, date: i64) -> NewMessage {
        NewMessage {
            uid: self.uid.get(),
            remote_id: format!("m{}", self.uid.get()),
            gm_thread_id: Some(u64::from(self.uid.get())),
            mid: None,
            references: vec![],
            subject: subject.into(),
            from_name: "Brand".into(),
            from_addr: "a@brand.example".into(),
            verified,
            date,
            size: 1,
            seen: false,
            flagged: false,
            labels: vec![],
        }
    }

    fn save(&self, subject: &str, verified: bool, date: i64) -> i64 {
        self.uid.set(self.uid.get() + 1);
        self.store.save_messages(&self.inbox, &[self.message(subject, verified, date)]).unwrap()[0]
    }

    fn arrive(&self, id: i64) {
        self.controller.on_event(self.account, Event::Arrived(vec![id]));
    }

    /// The one notice since the last call, and whether it offers a code.
    fn take_notice(&self) -> Option<bool> {
        let notices = std::mem::take(&mut *self.notices.borrow_mut());
        assert!(notices.len() <= 1, "{notices:?}");
        notices.first().map(|notice| notice.has_code)
    }

    fn opens(&self, message: i64) -> Opens {
        Opens { account: self.account, message: Some(message) }
    }

    fn set(&self, change: impl FnOnce(&mut Settings)) {
        let theme = self.app.global::<Theme>();
        let mut settings = Settings::from(theme.get_preferences());
        change(&mut settings);
        theme.set_preferences(settings.into());
        settle();
    }

    fn open_newest(&self) {
        self.controller.refresh_now();
        settle();
        self.controller.select_row(0);
        settle();
    }
}

#[test]
fn login_codes() {
    let window = offscreen();
    let w = World::new("codes");
    window.set_size(slint::PhysicalSize::new(1280, 760));
    // A logo lookup writes from a thread of its own, and this test has a store of its own.
    w.set(|s| s.sender_logos = false);
    notifications(&w);
    the_bar_over_a_message(&w);
}

fn notifications(w: &World) {
    // A code in the subject needs no waiting, and comes from the store when the button is pressed.
    let id = w.save("Your verification code is 482913", true, now());
    w.arrive(id);
    assert_eq!(w.take_notice(), Some(true));
    w.controller.copy_code(w.opens(id));
    assert_eq!(w.copied.borrow_mut().drain(..).collect::<Vec<_>>(), ["482913"]);

    // A forged sender gets a plain notification, and a press that reaches here anyway copies nothing.
    let forged = w.save("Your verification code is 771204", false, now());
    w.arrive(forged);
    assert_eq!(w.take_notice(), Some(false));
    w.controller.copy_code(w.opens(forged));
    assert!(w.copied.borrow().is_empty());

    // Nor does a message of another account, whatever a button claims.
    w.controller.copy_code(Opens { account: w.account + 1, message: Some(id) });
    assert!(w.copied.borrow().is_empty());

    // Mail that waited for the app to come back has a code that has expired.
    let stale = w.save("Your verification code is 482913", true, now() - 16 * 60);
    w.arrive(stale);
    assert_eq!(w.take_notice(), Some(false));

    // Ordinary mail is announced at once, without looking at its body.
    let plain = w.save("Lunch on Friday?", true, now());
    w.arrive(plain);
    assert_eq!(w.take_notice(), Some(false));

    // A login mail waits for its body, and the code is found in it.
    let held = w.save("Verify your email address", true, now());
    w.arrive(held);
    assert_eq!(w.take_notice(), None);
    w.store.save_body(held, &body(CODE_MAIL)).unwrap();
    w.controller.on_event(w.account, Event::BodyReady(held));
    assert_eq!(w.take_notice(), Some(true));
    w.controller.copy_code(w.opens(held));
    assert_eq!(w.copied.borrow_mut().drain(..).collect::<Vec<_>>(), ["482913"]);

    // The hold's own timer finds nothing to do once the body has answered.
    std::thread::sleep(std::time::Duration::from_millis(3100));
    settle();
    assert_eq!(w.take_notice(), None);

    // A body that fails says so at once, and one that never comes does not cost the notification.
    let failed = w.save("Verify your email address", true, now());
    w.arrive(failed);
    w.controller.on_event(w.account, Event::BodyFailed(failed));
    assert_eq!(w.take_notice(), Some(false));
    let silent = w.save("Verify your email address", true, now());
    w.arrive(silent);
    assert_eq!(w.take_notice(), None);
    std::thread::sleep(std::time::Duration::from_millis(3100));
    settle();
    assert_eq!(w.take_notice(), Some(false));

    // Mail read elsewhere during the wait is no longer news.
    let read = w.save("Verify your email address", true, now());
    w.arrive(read);
    let seen = NewMessage { seen: true, ..w.message("Verify your email address", true, now()) };
    w.store.save_messages(&w.inbox, &[seen]).unwrap();
    w.controller.on_event(w.account, Event::BodyFailed(read));
    assert_eq!(w.take_notice(), None);

    // Turned off, nothing waits and nothing is offered.
    w.set(|s| s.login_codes = false);
    let off = w.save("Your verification code is 482913", true, now());
    w.arrive(off);
    assert_eq!(w.take_notice(), Some(false));
    let off = w.save("Verify your email address", true, now());
    w.arrive(off);
    assert_eq!(w.take_notice(), Some(false));
    w.set(|s| s.login_codes = true);

    // A press with no code to copy opens the message instead.
    w.controller.copy_code(w.opens(plain));
    assert!(w.copied.borrow().is_empty());
    assert_eq!(w.controller.selected_message(), Some(plain));
}

fn the_bar_over_a_message(w: &World) {
    let shown = || w.app.get_message().code.to_string();

    // Newer than anything above, so that each is the top row.
    let id = w.save("Verify your email address", true, now() + 2);
    w.store.save_body(id, &body(CODE_MAIL)).unwrap();
    w.open_newest();
    assert_eq!(shown(), "482913");
    shot(&w.app, "code-bar");
    assert!(w.app.invoke_copy_code());
    assert_eq!(w.copied.borrow_mut().drain(..).collect::<Vec<_>>(), ["482913"]);

    // Old mail keeps its bar: opening it is a choice, unlike a notification.
    let forged = w.save("Verify your email address", false, now() + 4);
    w.store.save_body(forged, &body(CODE_MAIL)).unwrap();
    w.open_newest();
    assert_eq!(shown(), "", "a forged sender's code was shown");

    let plain = w.save("Lunch on Friday?", true, now() + 6);
    w.store.save_body(plain, &body("The code is 482913")).unwrap();
    w.open_newest();
    assert_eq!(shown(), "");
}
