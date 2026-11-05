//! Daily use of the real UI on a Gmail-shaped store: reading threads, triage, writing, searching.
//! The server's side of all this is in the core crate's `dovecot.rs` and `gmail_wire.rs`.

mod common;

use std::rc::Rc;
use std::sync::Arc;

use common::{RecordingNotifier, RecordingPane, click, fresh_dir, linger, offscreen, press, settle, shot, typed, with_control};
use mailfunction::notify::Opens;
use mailfunction::{App, Controller, Pane, Theme};
use mailfunction_core::engine::Event;
use mailfunction_core::remote::Offline;
use mailfunction_core::store::{Account, AuthKind, LabelSpec, NewMessage, Reader, Role, Security, Store};
use slint::{ComponentHandle, Model, PhysicalSize};

const NOW: i64 = 1_793_000_000;
const ESCAPE: &str = "\u{1b}";
const ENTER: &str = "\n";
const TAB: &str = "\t";

struct Mail {
    uid: u32,
    thread: u64,
    from: (&'static str, &'static str),
    subject: &'static str,
    hours_ago: i64,
    seen: bool,
    labels: Vec<LabelSpec>,
}

fn inbox() -> Vec<LabelSpec> {
    vec![LabelSpec::system(Role::Inbox)]
}

fn seeded() -> Vec<Mail> {
    let anh = ("Nguyễn Thị Minh Anh", "minhanh@example.vn");
    let huy = ("Trần Đức Huy", "huy.tran@example.vn");
    vec![
        Mail { uid: 1, thread: 1, from: anh, subject: "Lịch họp tuần sau", hours_ago: 30, seen: true, labels: inbox() },
        Mail { uid: 2, thread: 1, from: huy, subject: "Re: Lịch họp tuần sau", hours_ago: 26, seen: false, labels: inbox() },
        Mail { uid: 3, thread: 1, from: anh, subject: "Re: Lịch họp tuần sau", hours_ago: 2, seen: false, labels: inbox() },
        Mail { uid: 4, thread: 2, from: ("Linear", "n@linear.example"), subject: "3 issues assigned to you", hours_ago: 3, seen: false, labels: inbox() },
        Mail { uid: 5, thread: 3, from: huy, subject: "Hóa đơn tháng 10", hours_ago: 5, seen: true, labels: vec![LabelSpec::system(Role::Inbox), LabelSpec::user("Hóa đơn")] },
        Mail { uid: 6, thread: 4, from: ("Sarah Chen", "sarah@studio.example"), subject: "Design review notes", hours_ago: 50, seen: true, labels: inbox() },
    ]
}

fn body(subject: &str, from: &str, text: &str) -> Vec<u8> {
    format!(
        "From: {from}\r\nTo: Kong <kong@example.com>\r\nCc: team@example.vn\r\nSubject: {subject}\r\nMessage-ID: <{}@x>\r\n\
         Date: Wed, 04 Nov 2026 10:00:00 +0700\r\nContent-Type: text/plain; charset=utf-8\r\n\r\n{text}\r\n",
        subject.len()
    )
    .into_bytes()
}

const WITH_FILE: &[u8] = b"From: Sarah <sarah@studio.example>\r\nTo: kong@example.com\r\nSubject: Design review notes\r\n\
Message-ID: <file@x>\r\nContent-Type: multipart/mixed; boundary=b\r\n\r\n\
--b\r\nContent-Type: text/plain\r\n\r\nNotes attached.\r\n\
--b\r\nContent-Type: application/pdf\r\nContent-Disposition: attachment; filename=notes.pdf\r\n\r\nPDF\r\n--b--\r\n";

struct Scene {
    dir: std::path::PathBuf,
    app: App,
    store: Arc<Store>,
    reader: Reader,
    controller: Rc<Controller>,
    account: i64,
}

impl Scene {
    fn subjects(&self) -> Vec<String> {
        self.app.get_rows().iter().map(|r| r.subject.to_string()).collect()
    }

    fn label(&self, name: &str) -> usize {
        self.app.get_labels().iter().position(|l| l.name == name).unwrap_or_else(|| panic!("no {name}"))
    }

    fn unread(&self) -> String {
        self.app.get_labels().row_data(0).unwrap().unread.to_string()
    }

    /// Picks a list row up, like a click on it.
    fn open_subject(&self, subject: &str) {
        let row = self.subjects().iter().position(|s| s == subject).unwrap_or_else(|| panic!("no {subject}"));
        self.controller.select_row(row);
        settle();
    }

    fn notice(&self) -> (String, String) {
        (self.app.get_notice().to_string(), self.app.get_notice_action().to_string())
    }
}

fn scene() -> (Scene, std::path::PathBuf) {
    let window = offscreen();
    let dir = fresh_dir("mail");
    let store = Arc::new(Store::open(&dir).unwrap());
    let account = store
        .add_account(&Account {
            id: 0,
            email: "kong@example.com".into(),
            host: "localhost".into(),
            port: 993,
            security: Security::Tls,
            auth: AuthKind::Google,
            smtp_host: "localhost".into(),
            smtp_port: 25,
        })
        .unwrap();
    store.ensure_label(account, &LabelSpec::system(Role::Inbox)).unwrap();
    store.ensure_label(account, &LabelSpec::system(Role::Drafts)).unwrap();
    store.ensure_label(account, &LabelSpec::system(Role::Sent)).unwrap();
    let all = store.folder(account, "[Gmail]/All Mail", &LabelSpec::system(Role::All)).unwrap();
    store.folder(account, "[Gmail]/Spam", &LabelSpec::system(Role::Spam)).unwrap();
    store.folder(account, "[Gmail]/Trash", &LabelSpec::system(Role::Trash)).unwrap();
    let messages: Vec<_> = seeded()
        .into_iter()
        .map(|m| NewMessage {
            uid: m.uid,
            remote_id: format!("gm:{}", m.uid),
            gm_thread_id: Some(m.thread),
            mid: Some(format!("{}@x", m.uid)),
            references: vec![],
            subject: m.subject.into(),
            from_name: m.from.0.into(),
            from_addr: m.from.1.into(),
            date: NOW - m.hours_ago * 3600,
            size: 1000,
            seen: m.seen,
            flagged: false,
            labels: m.labels,
        })
        .collect();
    store.save_messages(&all, &messages).unwrap();
    let reader = Reader::open(&dir).unwrap();
    for (uid, raw) in [
        (1, body("Lịch họp tuần sau", "Nguyễn Thị Minh Anh <minhanh@example.vn>", "Thứ Ba lúc 9 giờ sáng nhé.")),
        (2, body("Re: Lịch họp tuần sau", "Trần Đức Huy <huy.tran@example.vn>", "Anh đồng ý.")),
        (3, body("Re: Lịch họp tuần sau", "Nguyễn Thị Minh Anh <minhanh@example.vn>", "Em đã đặt phòng.")),
        (4, body("3 issues assigned to you", "Linear <n@linear.example>", "Three new issues.")),
        (5, body("Hóa đơn tháng 10", "Trần Đức Huy <huy.tran@example.vn>", "Em gửi hóa đơn.")),
        (6, WITH_FILE.to_vec()),
    ] {
        let id: i64 = rusqlite_id(&dir, uid);
        store.save_body(id, &raw).unwrap();
    }

    let app = App::new().unwrap();
    window.set_size(PhysicalSize::new(1280, 760));
    let controller = Controller::new(app.clone_strong(), &dir, None, Box::new(RecordingPane::default()), Box::new(RecordingNotifier::default()), Arc::new(Offline)).unwrap();
    app.show().unwrap();
    window.dispatch_event(slint::platform::WindowEvent::WindowActiveChanged(true));
    controller.load_accounts().unwrap();
    settle();
    (Scene { dir: dir.clone(), app, store, reader, controller, account }, dir)
}

fn rusqlite_id(dir: &std::path::Path, uid: u32) -> i64 {
    let conn = rusqlite::Connection::open(dir.join("meta.db")).unwrap();
    conn.query_row("SELECT id FROM message WHERE remote_id = ?1", [format!("gm:{uid}")], |r| r.get(0)).unwrap()
}

#[test]
fn a_day_of_mail() {
    let (s, dir) = scene();
    let app = &s.app;

    // A thread is one row, with how many messages it holds.
    let rows = app.get_rows();
    assert_eq!(rows.row_count(), 4);
    assert_eq!(rows.row_data(0).unwrap().subject, "Re: Lịch họp tuần sau");
    assert_eq!(rows.row_data(0).unwrap().count, "3");
    assert!(rows.row_data(0).unwrap().unread);
    assert_eq!(s.unread(), "3");

    // Opening goes to the first message not yet read, with the others one line away.
    press(app, "j");
    assert_eq!(app.get_thread().row_count(), 3);
    assert_eq!(app.get_thread_open(), 1);
    assert_eq!(app.get_message().text.trim(), "Anh đồng ý.");
    shot(app, "20-thread");
    app.global::<Theme>().set_dark(true);
    settle();
    shot(app, "20-thread-dark");
    app.global::<Theme>().set_dark(false);
    press(app, "n");
    assert_eq!(app.get_message().text.trim(), "Em đã đặt phòng.");
    press(app, "n");
    assert_eq!(app.get_thread_open(), 2, "there is no fourth");
    press(app, "p");
    press(app, "p");
    assert_eq!(app.get_message().text.trim(), "Thứ Ba lúc 9 giờ sáng nhé.");

    // It stays unread until it has been open a moment, and then reading it is queued.
    linger();
    assert_eq!(s.unread(), "1", "the thread's two unread messages");
    assert!(!app.get_rows().row_data(0).unwrap().unread);
    assert_eq!(s.store.pending_count(s.account).unwrap(), 2);

    // Moving straight past a message leaves it unread.
    let before = s.unread();
    press(app, "j");
    press(app, "k");
    assert_eq!(s.unread(), before);

    // Marked unread, it stays so while it is open.
    press(app, "u");
    assert_eq!(s.unread(), "4", "all of the thread, and Linear");
    linger();
    assert_eq!(s.unread(), "4");
    press(app, "u");
    assert_eq!(s.unread(), "1");

    // A star goes on the newest message and shows at once.
    press(app, "s");
    assert!(app.get_starred());
    assert!(app.get_rows().row_data(0).unwrap().starred);
    assert_eq!(s.controller_label_count("Starred"), 1);
    press(app, "s");
    assert!(!app.get_starred());

    // Archive leaves the Inbox, offers Undo, and moves on to the next thread.
    press(app, "e");
    assert_eq!(s.subjects(), ["3 issues assigned to you", "Hóa đơn tháng 10", "Design review notes"]);
    assert_eq!(s.notice(), ("Archived".to_string(), "Undo".to_string()));
    assert_eq!(app.get_selected_row(), 0);
    assert_eq!(app.get_message().subject, "3 issues assigned to you");
    shot(app, "21-archived");
    press(app, "z");
    assert_eq!(s.subjects()[0], "Re: Lịch họp tuần sau");
    assert_eq!(s.notice().0, "");

    // The notice's button does the same as the key.
    press(app, "e");
    click(app, 530.0, 728.0);
    assert_eq!(s.subjects()[0], "Re: Lịch họp tuần sau", "the Undo button");

    // Delete takes it to Trash, where the same key brings it back.
    s.open_subject("Hóa đơn tháng 10");
    press(app, "#");
    assert!(!s.subjects().contains(&"Hóa đơn tháng 10".to_string()));
    assert_eq!(s.notice().0, "Deleted");
    s.controller.select_label(s.label("Trash"));
    assert_eq!(s.subjects(), ["Hóa đơn tháng 10"]);
    press(app, "j");
    assert!(app.get_in_trash());
    shot(app, "22-in-trash");
    press(app, "#");
    assert!(s.subjects().is_empty());
    assert_eq!(s.notice().0, "Moved to Inbox");
    assert_eq!(app.get_pane(), Pane::Empty);
    s.controller.select_label(0);
    assert!(s.subjects().contains(&"Hóa đơn tháng 10".to_string()));

    // Spam, and back.
    s.open_subject("Design review notes");
    press(app, "!");
    s.controller.select_label(s.label("Spam"));
    assert_eq!(s.subjects(), ["Design review notes"]);
    press(app, "j");
    assert!(app.get_in_spam());
    press(app, "!");
    assert_eq!(s.notice().0, "Moved to Inbox");
    s.controller.select_label(0);

    // Labels: a panel over the list, because the webview covers everything else.
    s.open_subject("3 issues assigned to you");
    press(app, "l");
    assert!(app.get_picking());
    let picks = app.get_picks();
    assert_eq!(picks.iter().map(|p| p.name.to_string()).collect::<Vec<_>>(), ["Hóa đơn"]);
    assert!(!picks.row_data(0).unwrap().on);
    press(app, ENTER);
    assert!(picks.row_data(0).unwrap().on);
    shot(app, "23-labels");
    press(app, ESCAPE);
    assert!(!app.get_picking());
    assert_eq!(app.get_message().subject, "3 issues assigned to you", "Esc closed only the panel");

    // Attachments can be seen, and saved, but never opened.
    s.open_subject("Design review notes");
    assert_eq!(app.get_message().attachments.row_data(0).unwrap().name, "notes.pdf");

    // The keyboard has its own list.
    press(app, "?");
    assert!(app.get_showing_keys());
    shot(app, "24-keys");
    app.global::<Theme>().set_dark(true);
    settle();
    shot(app, "24-keys-dark");
    app.global::<Theme>().set_dark(false);
    press(app, ESCAPE);
    assert!(!app.get_showing_keys());

    // `g` then a letter goes to a label.
    press(app, "g");
    press(app, "t");
    assert_eq!(app.get_active_label(), s.label("Sent") as i32);
    press(app, "g");
    press(app, "i");
    assert_eq!(app.get_active_label(), 0);

    writing(&s);
    searching(&s);
    across_accounts(&s);

    drop(s);
    let _ = std::fs::remove_dir_all(&dir);
}

impl Scene {
    fn controller_label_count(&self, name: &str) -> usize {
        let label = self.reader.labels(self.account).unwrap().into_iter().find(|l| l.name == name).unwrap();
        self.reader.message_count(label.id).unwrap()
    }

    fn drafts(&self) -> Vec<String> {
        let label = self.reader.labels(self.account).unwrap().into_iter().find(|l| l.role == Role::Drafts).unwrap();
        self.reader.messages(label.id, 0, 10).unwrap().into_iter().map(|m| m.subject).collect()
    }
}

fn writing(s: &Scene) {
    let app = &s.app;

    // A new message: the keys now type, rather than act on the thread behind.
    s.open_subject("Design review notes");
    press(app, "c");
    assert!(app.get_composing());
    typed(app, "sarah@studio.example");
    assert_eq!(app.get_draft_to(), "sarah@studio.example");
    press(app, ESCAPE);
    assert!(!app.get_composing());
    assert_eq!(s.notice().0, "Saved to Drafts");
    assert_eq!(s.drafts(), [""], "it keeps what was typed, even with no subject");
    assert_eq!(app.get_message().subject, "Design review notes", "the message comes back");

    // A draft opens where it was left off, and an emptied one goes away.
    press(app, "g");
    press(app, "d");
    press(app, "j");
    assert!(app.get_composing());
    assert_eq!(app.get_draft_to(), "sarah@studio.example");
    app.set_draft_to("".into());
    press(app, ESCAPE);
    assert!(s.drafts().is_empty());
    press(app, "g");
    press(app, "i");

    // Only looking at a draft leaves it alone: what is on the server may be richer than plain
    // text, and passing it on the way through Drafts must not replace it.
    let foreign = "From: kong@example.com\r\nTo: a@example.com\r\nSubject: From the web\r\nMessage-ID: <web@x>\r\n\
                   Content-Type: text/html\r\n\r\n<b>rich</b>\r\n";
    let all = s.store.folder(s.account, "[Gmail]/All Mail", &LabelSpec::system(Role::All)).unwrap();
    let web = NewMessage {
        uid: 900,
        remote_id: "gm:900".into(),
        gm_thread_id: Some(900),
        mid: Some("web@x".into()),
        references: vec![],
        subject: "From the web".into(),
        from_name: "".into(),
        from_addr: "kong@example.com".into(),
        date: NOW,
        size: 100,
        seen: true,
        flagged: false,
        labels: vec![LabelSpec::system(Role::Drafts)],
    };
    s.store.save_messages(&all, &[web]).unwrap();
    let web_id = rusqlite_id(&s.dir, 900);
    s.store.save_body(web_id, foreign.as_bytes()).unwrap();
    s.controller.refresh_now();
    let pending = s.store.pending_count(s.account).unwrap();
    press(app, "g");
    press(app, "d");
    press(app, "j");
    assert!(app.get_composing());
    press(app, ESCAPE);
    assert_eq!(s.store.pending_count(s.account).unwrap(), pending, "nothing to upload");
    assert_eq!(s.reader.body(web_id).unwrap().unwrap(), foreign.as_bytes());
    s.store.discard_draft(web_id).unwrap();
    press(app, "g");
    press(app, "i");

    // Starting and leaving at once keeps nothing.
    press(app, "c");
    press(app, ESCAPE);
    assert!(s.drafts().is_empty());

    // Typing is saved after a pause, with no action from the user.
    press(app, "c");
    typed(app, "Bin");
    app.invoke_draft_edited();
    linger();
    linger();
    assert_eq!(s.drafts().len(), 1, "autosaved");
    shot(app, "30-compose");
    app.global::<Theme>().set_dark(true);
    settle();
    shot(app, "30-compose-dark");
    app.global::<Theme>().set_dark(false);

    // Nothing goes without a real address, and the composer says what is wrong.
    with_control(app, ENTER);
    assert!(app.get_composing());
    assert!(app.get_draft_error().contains("Bin"), "{}", app.get_draft_error());
    settle();
    shot(app, "30-compose-error");
    click(app, 1240.0, 738.0);
    assert!(!app.get_composing(), "the delete button");
    assert!(s.drafts().is_empty());

    // People who have written come up as you type, and Tab takes the first.
    press(app, "c");
    typed(app, "min");
    assert_eq!(app.get_suggesting(), 0);
    assert_eq!(app.get_suggestions().row_data(0).unwrap(), "Nguyễn Thị Minh Anh <minhanh@example.vn>");
    settle();
    shot(app, "31-suggestions");
    press(app, TAB);
    assert_eq!(app.get_draft_to(), "Nguyễn Thị Minh Anh <minhanh@example.vn>, ");
    assert_eq!(app.get_suggesting(), -1);
    typed(app, "huy");
    press(app, TAB);
    assert_eq!(app.get_draft_to(), "Nguyễn Thị Minh Anh <minhanh@example.vn>, Trần Đức Huy <huy.tran@example.vn>, ", "the caret went to the end");
    app.set_draft_subject("Hello".into());
    app.set_draft_body("Text".into());

    // Sending waits a moment, in which it can be called back as it was.
    with_control(app, ENTER);
    assert!(!app.get_composing());
    assert_eq!(s.notice(), ("Sending".to_string(), "Undo".to_string()));
    assert!(s.drafts().is_empty());
    shot(app, "32-sending");
    press(app, "z");
    assert!(app.get_composing());
    assert_eq!(app.get_draft_subject(), "Hello");
    assert_eq!(s.drafts(), ["Hello"]);
    // A long list of people stays in its field, with the rest of the composer where it was.
    shot(app, "33-long-recipients");
    click(app, 1240.0, 738.0);
    assert!(s.drafts().is_empty());

    // A reply is addressed, quoted and in the thread, with the cursor ready above the quote.
    s.open_subject("Re: Lịch họp tuần sau");
    press(app, "r");
    assert!(app.get_composing());
    assert_eq!(app.get_draft_to(), "Nguyễn Thị Minh Anh <minhanh@example.vn>");
    assert_eq!(app.get_draft_cc(), "");
    assert!(app.get_draft_body().contains("> Em đã đặt phòng."), "{}", app.get_draft_body());
    assert_eq!(app.get_draft_subject(), "Re: Lịch họp tuần sau");
    typed(app, "Đồng ý");
    assert!(app.get_draft_body().starts_with("Đồng ý"), "{}", app.get_draft_body());
    settle();
    shot(app, "33-reply");
    click(app, 1240.0, 738.0);

    // Left untouched, a reply was never worth keeping.
    press(app, "r");
    press(app, ESCAPE);
    assert!(s.drafts().is_empty());

    // Reply all brings in everyone else, and shows the field they are in.
    press(app, "a");
    assert_eq!(app.get_draft_cc(), "team@example.vn");
    assert!(app.get_draft_copies());
    settle();
    shot(app, "33-reply-all");
    press(app, ESCAPE);

    // A forward carries the attachments and asks who to.
    s.open_subject("Design review notes");
    press(app, "f");
    assert_eq!(app.get_draft_subject(), "Fwd: Design review notes");
    assert_eq!(app.get_draft_to(), "");
    assert_eq!(app.get_draft_files().row_data(0).unwrap().name, "notes.pdf");
    settle();
    shot(app, "34-forward");

    // Files can be added and taken away, but not past what mail can carry.
    let file = std::env::temp_dir().join(format!("mailfunction-attach-{}.txt", std::process::id()));
    std::fs::write(&file, "nội dung").unwrap();
    s.controller.attach_paths(std::slice::from_ref(&file));
    assert_eq!(app.get_draft_files().row_count(), 2);
    assert_eq!(app.get_draft_files().row_data(1).unwrap().size, "10 B");
    settle();
    shot(app, "34-attachments");
    app.invoke_remove_draft_file(0);
    assert_eq!(app.get_draft_files().row_count(), 1);
    let large = file.with_extension("bin");
    std::fs::File::create(&large).unwrap().set_len(40 * 1024 * 1024).unwrap();
    s.controller.attach_paths(std::slice::from_ref(&large));
    assert_eq!(app.get_draft_files().row_count(), 1);
    assert!(app.get_draft_error().contains("too large"), "{}", app.get_draft_error());
    let _ = std::fs::remove_file(&file);
    let _ = std::fs::remove_file(&large);
    click(app, 1240.0, 738.0);

    // Opening another message from the list saves what was being written.
    press(app, "c");
    typed(app, "x@example.com");
    click(app, 400.0, 170.0);
    assert!(!app.get_composing());
    assert_eq!(s.drafts().len(), 1);
    let draft = s.reader.messages(s.reader.labels(s.account).unwrap().into_iter().find(|l| l.role == Role::Drafts).unwrap().id, 0, 1).unwrap()[0].id;
    s.store.discard_draft(draft).unwrap();

    // A search finds mail waiting to go, and opening it takes it back rather than edit it live.
    press(app, "c");
    app.set_draft_to("a@example.com".into());
    app.set_draft_subject("Waiting".into());
    app.invoke_draft_edited();
    with_control(app, ENTER);
    assert_eq!(s.notice().0, "Sending");
    let waiting = s.reader.search(s.account, "Waiting", 1).unwrap()[0].clone();
    press(app, "/");
    typed(app, "Waiting");
    linger();
    s.controller.select_row(0);
    settle();
    assert!(app.get_composing());
    assert_eq!(s.drafts(), ["Waiting"], "back from the outbox");
    assert!(s.store.queue_send(waiting.id, 0).is_ok(), "and an ordinary draft again");
    s.store.cancel_send(waiting.id).unwrap();
    click(app, 1240.0, 738.0);
    press(app, ESCAPE);

    // The outcome of a send arrives as a notice, whichever way it went.
    s.controller.on_event(s.account, Event::SendFailed { message: 0, maybe: false, reason: "Mailbox full.".into() });
    assert!(s.notice().0.contains("Mailbox full"));
    s.controller.on_event(s.account, Event::SendFailed { message: 0, maybe: true, reason: "lost".into() });
    assert!(s.notice().0.contains("Check Sent"));
    settle();
    shot(app, "35-maybe-sent");
    s.controller.on_event(s.account, Event::Sent(0));
    assert_eq!(s.notice().0, "Sent");
    s.controller.on_event(s.account, Event::ActionRefused);
    assert!(s.notice().0.contains("put back"));
}

/// Whatever is in progress stays with the account it began in.
fn across_accounts(s: &Scene) {
    let app = &s.app;

    // A notification names a message, and the list holds threads. An older message in a thread
    // opens that thread, and a half-written draft is kept on the way.
    let thread_row = s.subjects().iter().position(|t| t == "Re: Lịch họp tuần sau").unwrap();
    let older = rusqlite_id(&s.dir, 1);
    press(app, "c");
    typed(app, "kept@example.com");
    s.controller.open(Opens { account: s.account, message: Some(older) });
    settle();
    assert!(!app.get_composing());
    assert_eq!(app.get_selected_row(), thread_row as i32);
    assert_eq!(app.get_thread().row_count(), 3);
    assert_eq!(s.drafts().len(), 1);
    let drafts = s.reader.labels(s.account).unwrap().into_iter().find(|l| l.role == Role::Drafts).unwrap();
    s.store.discard_draft(s.reader.messages(drafts.id, 0, 1).unwrap()[0].id).unwrap();
    let work = Account { email: "work@example.com".into(), ..s.reader.accounts().unwrap()[0].clone() };
    let work_id = s.store.add_account(&work).unwrap();
    s.store.ensure_label(work_id, &LabelSpec::system(Role::Inbox)).unwrap();
    s.store.ensure_label(work_id, &LabelSpec::system(Role::Drafts)).unwrap();
    s.controller.add_account(Account { id: work_id, ..work }).unwrap();
    s.controller.select_account(0);
    settle();

    // Undo after switching reverses the action where it happened.
    let pending = s.store.pending_count(s.account).unwrap();
    s.open_subject("Design review notes");
    press(app, "e");
    let archived = s.store.pending_count(s.account).unwrap();
    assert!(archived > pending);
    s.controller.select_account(1);
    press(app, "z");
    assert_eq!(s.store.pending_count(work_id).unwrap(), 0, "nothing queued against the other account");
    assert_eq!(s.store.pending_count(s.account).unwrap(), archived + 1);
    s.controller.select_account(0);
    settle();
    assert!(s.subjects().contains(&"Design review notes".to_string()));

    // A message half written is saved to its own account on the way out.
    press(app, "c");
    typed(app, "a@example.com");
    s.controller.select_account(1);
    assert!(!app.get_composing());
    assert_eq!(s.drafts().len(), 1);
    let work_drafts = s.reader.labels(work_id).unwrap().into_iter().find(|l| l.role == Role::Drafts).unwrap();
    assert_eq!(s.reader.message_count(work_drafts.id).unwrap(), 0);
    s.controller.select_account(0);
    settle();

    // Switching ends a search, and so does picking a label from the sidebar.
    press(app, "/");
    typed(app, "design");
    linger();
    click(app, 90.0, 112.0);
    assert!(!app.get_searching(), "the sidebar leaves the search");
    assert_eq!(app.get_active_label(), 1);
}

fn searching(s: &Scene) {
    let app = &s.app;
    s.controller.select_label(0);
    settle();

    // `/` swaps the title for a field, and results replace the list as you type.
    press(app, "/");
    assert!(app.get_searching());
    typed(app, "hoa don");
    linger();
    assert_eq!(s.subjects(), ["Hóa đơn tháng 10"], "found without the accents");
    assert_eq!(app.get_pane(), Pane::Empty);
    shot(app, "40-search");
    app.global::<Theme>().set_dark(true);
    settle();
    shot(app, "40-search-dark");
    app.global::<Theme>().set_dark(false);

    // A result opens as its whole thread would.
    s.controller.select_row(0);
    settle();
    assert_eq!(app.get_message().subject, "Hóa đơn tháng 10");

    // The server's answer joins what was here, and one for an older search is ignored.
    let other = s.reader.search(s.account, "Design", 5).unwrap()[0].id;
    s.controller.on_event(s.account, Event::SearchDone { key: 0, ids: vec![other] });
    assert_eq!(s.subjects().len(), 1);
    s.controller.run_search();
    let key = 1 + s.controller.search_key();
    s.controller.on_event(s.account, Event::SearchDone { key: key - 1, ids: vec![other] });
    assert_eq!(s.subjects().len(), 2, "{:?}", s.subjects());

    // Nothing found says so.
    app.set_query("zzzzzz".into());
    s.controller.run_search();
    settle();
    assert!(s.subjects().is_empty());
    shot(app, "40-search-nothing");

    // Esc goes back to where the search began.
    press(app, ESCAPE);
    assert!(!app.get_searching());
    assert_eq!(app.get_query(), "");
    assert!(s.subjects().len() > 1);
}
