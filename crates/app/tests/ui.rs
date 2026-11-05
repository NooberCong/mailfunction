//! The real UI and controller on a seeded store, in an offscreen software window.

mod common;

use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use common::{RecordingNotifier, RecordingPane, click, fresh_dir, offscreen, press, settle, shot};
use mailfunction::notify::Opens;
use mailfunction::{App, Controller, ImageBar, Pane, Theme};
use mailfunction_core::engine::Event;
use mailfunction_core::logo;
use mailfunction_core::remote::Offline;
use mailfunction_core::store::{Account, AuthKind, Folder, LabelSpec, NewMessage, Reader, Role, Security, Store};
use slint::platform::WindowEvent;
use slint::{ComponentHandle, Model, PhysicalSize};

/// The picture's size and the colour at its centre, with its alpha.
fn picture_centre(png_bytes: &[u8]) -> (u32, [u8; 4]) {
    let mut reader = png::Decoder::new(std::io::Cursor::new(png_bytes)).read_info().unwrap();
    let mut pixels = vec![0; reader.output_buffer_size().unwrap()];
    let info = reader.next_frame(&mut pixels).unwrap();
    let at = (info.height / 2 * info.width + info.width / 2) as usize * 4;
    (info.width, pixels[at..at + 4].try_into().unwrap())
}

fn account_row() -> Account {
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

fn add_account(store: &Store) -> i64 {
    store.add_account(&account_row()).unwrap()
}

const NOW: i64 = 1_793_000_000;

struct Sample {
    from: (&'static str, &'static str),
    subject: &'static str,
    age_hours: i64,
    seen: bool,
    flagged: bool,
    labels: &'static [&'static str],
}

const MAIL: &[Sample] = &[
    Sample { from: ("Nguyễn Thị Minh Anh", "minhanh@example.vn"), subject: "Lịch họp tuần sau và tài liệu cần chuẩn bị", age_hours: 1, seen: false, flagged: false, labels: &["Công việc"] },
    Sample { from: ("Linear", "notifications@linear.example"), subject: "3 issues were assigned to you this week", age_hours: 3, seen: false, flagged: false, labels: &[] },
    Sample { from: ("Trần Đức Huy", "huy.tran@example.vn"), subject: "Re: Hóa đơn tháng 10", age_hours: 5, seen: true, flagged: true, labels: &["Hóa đơn"] },
    Sample { from: ("", "billing@cloud.example"), subject: "Your invoice for October is ready", age_hours: 26, seen: true, flagged: false, labels: &["Hóa đơn"] },
    Sample { from: ("Vietcombank", "noreply@vcb.example"), subject: "Thông báo biến động số dư tài khoản", age_hours: 30, seen: true, flagged: false, labels: &[] },
    Sample { from: ("Sarah Chen", "sarah@studio.example"), subject: "Design review notes, and a very long subject line that has to be cut off cleanly at the edge of the list column", age_hours: 50, seen: true, flagged: false, labels: &["Công việc"] },
    Sample { from: ("GitHub", "noreply@github.example"), subject: "[mailfunction] Your weekly digest", age_hours: 75, seen: true, flagged: false, labels: &[] },
    Sample { from: ("Lê Hoàng Nam", "nam@example.vn"), subject: "", age_hours: 120, seen: true, flagged: false, labels: &[] },
    Sample { from: ("The Browser", "editor@thebrowser.example"), subject: "Five things worth reading this weekend", age_hours: 9000, seen: true, flagged: false, labels: &[] },
];

fn plain_body() -> Vec<u8> {
    "From: Nguyễn Thị Minh Anh <minhanh@example.vn>\r\nTo: Kong <kong@example.com>, Trần Đức Huy <huy.tran@example.vn>\r\n\
     Subject: x\r\nContent-Type: text/plain; charset=utf-8\r\n\r\n\
     Chào anh,\r\n\r\nEm gửi lịch họp tuần sau. Thứ Ba lúc 9 giờ sáng ở phòng họp tầng 3, \
     anh xem giúp em tài liệu đính kèm trước buổi họp nhé. Nếu anh có thay đổi gì thì báo em sớm \
     để em sắp xếp lại với mọi người.\r\n\r\nCảm ơn anh,\r\nMinh Anh\r\n"
        .as_bytes()
        .to_vec()
}

const HTML_BODY: &[u8] = b"From: Linear <n@linear.example>\r\nSubject: x\r\nContent-Type: multipart/mixed; boundary=b\r\n\r\n\
--b\r\nContent-Type: text/html; charset=utf-8\r\n\r\n<h1>Issues</h1><script>alert(1)</script><img src=\"https://t.example/p.gif\">\r\n\
--b\r\nContent-Type: application/pdf\r\nContent-Disposition: attachment; filename=report.pdf\r\n\r\nPDF\r\n--b--\r\n";

/// A stored logo: a solid tile, as `logo::find` would leave it.
fn logo_png(colour: [u8; 3]) -> Vec<u8> {
    let mut bytes = Vec::new();
    let mut encoder = png::Encoder::new(&mut bytes, 128, 128);
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.write_header().unwrap().write_image_data(&colour.repeat(128 * 128)).unwrap();
    bytes
}

fn seed(store: &Store, inbox: &Folder) {
    seed_from(store, inbox, MAIL);
}

fn seed_from(store: &Store, inbox: &Folder, mail: &[Sample]) {
    let messages: Vec<_> = mail
        .iter()
        .enumerate()
        .map(|(i, m)| NewMessage {
            uid: i as u32 + 1,
            remote_id: format!("m{i}"),
            gm_thread_id: None,
            mid: None,
            references: vec![],
            subject: m.subject.into(),
            from_name: m.from.0.into(),
            from_addr: m.from.1.into(),
            date: NOW - m.age_hours * 3600,
            size: 1000,
            seen: m.seen,
            flagged: m.flagged,
            labels: m.labels.iter().map(|l| LabelSpec::user(*l)).collect(),
        })
        .collect();
    store.save_messages(inbox, &messages).unwrap();
}

#[test]
fn the_whole_window() {
    let window = offscreen();

    let dir = fresh_dir("main");
    let store = Store::open(&dir).unwrap();
    let app = App::new().unwrap();
    window.set_size(PhysicalSize::new(1280, 760));
    let pane = RecordingPane::default();
    let html_shown = pane.shown.clone();
    let answered = pane.images.clone();
    let notifier = RecordingNotifier::default();
    let notices = notifier.shown.clone();
    let controller = Controller::new(app.clone_strong(), &dir, None, Box::new(pane), Box::new(common::RecordingEditor::default()), Box::new(notifier), Arc::new(Offline)).unwrap();
    app.show().unwrap();
    window.dispatch_event(WindowEvent::WindowActiveChanged(true));

    // First run: nothing but the sign-in button.
    controller.load_accounts().unwrap();
    assert!(!app.get_signed_in());
    settle();
    shot(&app, "01-sign-in");
    app.global::<Theme>().set_dark(true);
    settle();
    shot(&app, "01-sign-in-dark");
    app.global::<Theme>().set_dark(false);

    let account = add_account(&store);
    let inbox = store.folder(account, "INBOX", &LabelSpec::system(Role::Inbox)).unwrap();
    store.folder(account, "Sent", &LabelSpec::system(Role::Sent)).unwrap();
    store.folder(account, "Trash", &LabelSpec::system(Role::Trash)).unwrap();
    seed(&store, &inbox);
    let reader = Reader::open(&dir).unwrap();
    let inbox_rows = reader.messages(&[inbox.label_id], 0, 20).unwrap();
    store.save_body(inbox_rows[0].id, &plain_body()).unwrap();
    store.save_body(inbox_rows[1].id, HTML_BODY).unwrap();
    // Logos are looked up by the brand's domain, and checked far enough ahead never to go stale.
    let fresh = i64::MAX / 2;
    store.save_logo("linear.example", Some(&logo_png([94, 106, 210])), fresh).unwrap();
    store.save_logo("example.com", Some(&logo_png([196, 18, 47])), fresh).unwrap();

    controller.load_accounts().unwrap();
    assert!(app.get_signed_in());
    assert_eq!(app.get_rows().row_count(), MAIL.len());
    let labels = app.get_labels();
    let names: Vec<_> = labels.iter().map(|l| l.name.to_string()).collect();
    assert_eq!(names, ["Inbox", "Starred", "Sent", "Trash", "Công việc", "Hóa đơn"]);
    assert_eq!(labels.row_data(0).unwrap().unread, "2");
    assert_eq!(controller.unread(), 2);
    assert_eq!(app.get_pane(), Pane::Empty);
    settle();
    shot(&app, "02-inbox");

    // Opening: down arrow selects the first row, which is plain text and needs no webview.
    press(&app, "j");
    assert_eq!(app.get_selected_row(), 0);
    assert_eq!(app.get_pane(), Pane::Text);
    assert!(app.get_message().text.contains("Chào anh"));
    assert!(app.get_message().recipients.contains("Trần Đức Huy"));
    assert!(html_shown.borrow().is_empty());
    shot(&app, "03-plain-message");
    app.global::<Theme>().set_dark(true);
    settle();
    shot(&app, "03-plain-message-dark");
    app.global::<Theme>().set_dark(false);
    settle();

    // HTML goes to the pane, sanitised, and attachments are listed.
    press(&app, "j");
    assert_eq!(app.get_pane(), Pane::Html);
    assert_eq!(html_shown.borrow().len(), 1);
    assert!(!html_shown.borrow()[0].contains("alert") && !html_shown.borrow()[0].contains("t.example"));
    let attachments = app.get_message().attachments;
    assert_eq!(attachments.row_data(0).unwrap().name, "report.pdf");
    assert_eq!(attachments.row_data(0).unwrap().size, "3 B");
    shot(&app, "03-html-message");

    // A known sender's logo replaces the initials, in the row and in the header, at the size each
    // draws it. Everyone else keeps their initials, and so does the account's row without one.
    let linear = (0..MAIL.len()).find(|&i| MAIL[i].from.1.ends_with("@linear.example")).unwrap();
    assert_eq!(app.get_rows().row_data(linear).unwrap().logo.size().width, 38);
    assert_eq!(app.get_message().logo.size().width, 40);
    let stranger = (0..MAIL.len()).find(|&i| !MAIL[i].from.1.ends_with("@linear.example")).unwrap();
    assert_eq!(app.get_rows().row_data(stranger).unwrap().logo.size().width, 0);
    let account_colour = || {
        let logo = app.get_accounts().row_data(0).unwrap().logo;
        let pixels = logo.to_rgba8().unwrap();
        pixels.as_slice()[14 * 28 + 14].rgb()
    };
    assert_eq!(app.get_accounts().row_data(0).unwrap().logo.size().width, 28);
    assert_eq!(account_colour(), slint::Rgb8Pixel::new(196, 18, 47));

    // The account's own picture takes over from its provider's logo as soon as it arrives.
    store.save_logo(&logo::picture_key("kong@example.com"), Some(&logo_png([20, 160, 90])), fresh).unwrap();
    controller.on_event(account, Event::AccountPicture);
    assert_eq!(account_colour(), slint::Rgb8Pixel::new(20, 160, 90));

    // Remote images stay out until asked for, and nothing is fetched by opening the message.
    let sender = reader.messages(&[inbox.label_id], 0, 20).unwrap()[1].from_addr.clone();
    let page_image = "https://message.invalid/image/0";
    assert_eq!(app.get_image_bar(), ImageBar::Blocked);
    assert!(!html_shown.borrow().last().unwrap().contains(page_image));
    shot(&app, "03-images-blocked");
    app.global::<Theme>().set_dark(true);
    settle();
    shot(&app, "03-images-blocked-dark");
    app.global::<Theme>().set_dark(false);
    settle();

    // A subject that wraps pushes everything below it down, so the attachments stay in view.
    let one_line = app.get_slot_y();
    window.set_size(PhysicalSize::new(900, 480));
    settle();
    shot(&app, "03-images-blocked-narrow");
    assert!(app.get_slot_y() >= one_line + 25.0, "the header did not grow: {} to {}", one_line, app.get_slot_y());
    window.set_size(PhysicalSize::new(1280, 760));
    settle();
    assert_eq!(app.get_slot_y(), one_line);

    // Asked for, the page comes back at once and names only itself.
    click(&app, 1055.0, 212.0);
    assert_eq!(app.get_image_bar(), ImageBar::Hidden);
    assert!(html_shown.borrow().last().unwrap().contains(page_image));
    assert!(!html_shown.borrow().last().unwrap().contains("t.example"));

    // Each request gets an answer. One for an image the page does not have is turned down.
    controller.image_wanted(7, 70);
    assert_eq!(answered.borrow().last(), Some(&(70, None)));
    let key = controller.page_key().unwrap();
    controller.image_fetched(key, 1, Some(b"GIF89a".to_vec()));
    assert_eq!(answered.borrow().last(), Some(&(1, Some(b"GIF89a".to_vec()))));
    assert_eq!(app.get_image_bar(), ImageBar::Hidden);

    // When nothing comes at all, the user can try again.
    app.invoke_load_images();
    let key = controller.page_key().unwrap();
    controller.image_fetched(key, 2, None);
    assert_eq!(app.get_image_bar(), ImageBar::Failed);
    settle();
    shot(&app, "03-images-failed");
    app.invoke_load_images();
    assert_eq!(app.get_image_bar(), ImageBar::Hidden);

    // An answer for a page that has gone changes nothing on this one.
    controller.image_fetched(key, 3, None);
    assert_eq!(app.get_image_bar(), ImageBar::Hidden);

    // Once only: coming back to the message blocks them again.
    press(&app, "k");
    assert_eq!(app.get_image_bar(), ImageBar::Hidden);
    press(&app, "j");
    assert_eq!(app.get_image_bar(), ImageBar::Blocked);
    assert!(!reader.images_allowed(account, &sender).unwrap());

    // Always: from now on the message opens with them, and the bar offers a way back.
    app.invoke_always_load_images();
    assert!(reader.images_allowed(account, &sender.to_uppercase()).unwrap());
    assert_eq!(app.get_image_bar(), ImageBar::Allowed);
    settle();
    shot(&app, "03-images-allowed");
    press(&app, "k");
    press(&app, "j");
    assert_eq!((app.get_pane(), app.get_image_bar()), (Pane::Html, ImageBar::Allowed));
    assert!(html_shown.borrow().last().unwrap().contains(page_image));

    // A late answer for a message the user has left changes nothing, and the next page has
    // nothing to answer from.
    let key = controller.page_key().unwrap();
    press(&app, "k");
    controller.image_fetched(key, 4, None);
    assert_eq!((app.get_pane(), app.get_image_bar()), (Pane::Text, ImageBar::Hidden));
    assert_eq!(controller.page_key(), None);
    controller.image_wanted(0, 80);
    assert_eq!(answered.borrow().last(), Some(&(80, None)));
    press(&app, "j");

    app.invoke_stop_loading_images();
    assert!(!reader.images_allowed(account, &sender).unwrap());
    assert_eq!(app.get_image_bar(), ImageBar::Blocked);
    assert!(!html_shown.borrow().last().unwrap().contains(page_image));
    assert_eq!(controller.page_key(), None);
    settle();

    // No local body and no engine: the pane says so rather than sitting empty forever.
    press(&app, "j");
    assert_eq!(app.get_pane(), Pane::Loading);
    let third = inbox_rows[2].id;
    controller.on_event(account, Event::BodyFailed(third));
    assert_eq!(app.get_pane(), Pane::Failed);
    shot(&app, "04-failed");

    // A late answer for a message the user left must not replace the current one.
    press(&app, "k");
    let before = app.get_message().subject;
    controller.on_event(account, Event::BodyReady(third));
    assert_eq!(app.get_message().subject, before);

    // New mail arrives above the selection: the same message stays selected.
    let selected = app.get_selected_row();
    store
        .save_messages(
            &inbox,
            &[NewMessage {
                uid: 100,
                remote_id: "new".into(),
                gm_thread_id: None,
                mid: None,
                references: vec![],
                subject: "Just arrived".into(),
                from_name: "New".into(),
                from_addr: "new@example.com".into(),
                date: NOW,
                size: 1,
                seen: false,
                flagged: false,
                labels: vec![],
            }],
        )
        .unwrap();
    controller.refresh_now();
    assert_eq!(app.get_rows().row_count(), MAIL.len() + 1);
    assert_eq!(app.get_selected_row(), selected + 1);
    assert_eq!(app.get_rows().row_data(0).unwrap().subject, "Just arrived");

    // Switching label clears the reading pane.
    controller.select_label(4);
    assert_eq!(app.get_rows().row_count(), 2);
    assert_eq!(app.get_pane(), Pane::Empty);
    assert_eq!(app.get_selected_row(), -1);

    // The sidebar and the list answer to the pointer, through the marker and the fades.
    click(&app, 90.0, 112.0);
    assert_eq!(app.get_active_label(), 1, "a click on the second sidebar row");
    controller.select_label(0);
    settle();
    click(&app, 400.0, 170.0);
    assert_eq!(app.get_selected_row(), 1, "a click on the second message");

    // Sync shows as a moving line, and being offline as a word in the sidebar.
    let status = |index| app.get_accounts().row_data(index).unwrap().status;
    controller.on_event(account, Event::Syncing);
    assert!(app.get_syncing());
    settle();
    shot(&app, "04-syncing");
    controller.on_event(account, Event::Offline);
    assert!(!app.get_syncing());
    assert_eq!(status(0), "Offline");
    settle();
    shot(&app, "04-offline");
    controller.on_event(account, Event::Syncing);
    assert_eq!(status(0), "", "reconnecting clears the word");
    controller.on_event(account, Event::UpToDate);
    assert!(!app.get_syncing());

    // An empty label says so.
    controller.select_label(3);
    assert_eq!(app.get_rows().row_count(), 0);
    settle();
    shot(&app, "05-empty");

    // The sync button beside the title asks for a sync, unless one is already running.
    let syncs = Rc::new(Cell::new(0));
    app.on_sync_now({
        let syncs = syncs.clone();
        move || syncs.set(syncs.get() + 1)
    });
    click(&app, 546.0, 39.0);
    assert_eq!(syncs.get(), 1);

    // While it runs an empty list may only be waiting for its mail, so it does not say empty.
    controller.on_event(account, Event::Syncing);
    settle();
    shot(&app, "05-empty-syncing");
    click(&app, 546.0, 39.0);
    assert_eq!(syncs.get(), 1, "a running sync needs no second one");
    controller.on_event(account, Event::UpToDate);
    settle();
    shot(&app, "05-sync-button");

    // A second account: the first stays where it is, and each keeps to its own mail.
    let second = Account { email: "work@studio.example".into(), ..account_row() };
    controller.add_account(second).unwrap();
    assert_eq!(app.get_accounts().row_count(), 2);
    assert_eq!(app.get_active_account(), 1, "a new account opens at once");
    assert_eq!(app.get_rows().row_count(), 0, "no mail from the other account");
    assert_eq!(app.get_labels().row_count(), 0);
    settle();
    shot(&app, "08-new-account");

    // With no folders yet there is no list to show, so one message takes their place.
    let work = reader.accounts().unwrap()[1].id;
    controller.on_event(work, Event::Syncing);
    settle();
    shot(&app, "08-first-sync");
    controller.on_event(work, Event::SignInRequired("token revoked".into()));
    settle();
    shot(&app, "08-no-mail-signed-out");
    app.global::<Theme>().set_dark(true);
    settle();
    shot(&app, "08-no-mail-signed-out-dark");
    app.global::<Theme>().set_dark(false);
    click(&app, 756.0, 456.0);
    assert!(app.get_adding(), "the sign-in button");
    press(&app, "\u{1b}");
    controller.on_event(work, Event::Syncing);

    let work_inbox = store.folder(work, "INBOX", &LabelSpec::system(Role::Inbox)).unwrap();
    store.folder(work, "Sent", &LabelSpec::system(Role::Sent)).unwrap();
    seed_from(&store, &work_inbox, &MAIL[3..7]);
    controller.on_event(work, Event::Changed);
    controller.refresh_now();
    assert_eq!(app.get_rows().row_count(), 4, "the first sync fills the list");
    assert_eq!(app.get_labels().row_data(0).unwrap().name, "Inbox");
    assert_eq!(app.get_active_label(), 0);

    // A click on an account's row switches to it, and the pointer finds the row by its place.
    let rows_y = 760.0 - 8.0 - 2.0 * 36.0 - 2.0 * 46.0;
    click(&app, 90.0, rows_y + 22.0);
    assert_eq!(app.get_active_account(), 0);
    assert_eq!(app.get_rows().row_count(), MAIL.len() + 1);
    assert_eq!(app.get_labels().row_data(0).unwrap().name, "Inbox");
    assert_eq!(app.get_pane(), Pane::Empty);
    settle();
    shot(&app, "08-two-accounts");

    // So does the keyboard.
    let control = "\u{11}";
    app.window().dispatch_event(WindowEvent::KeyPressed { text: control.into() });
    press(&app, "2");
    assert_eq!(app.get_active_account(), 1);
    press(&app, "9");
    assert_eq!(app.get_active_account(), 1, "there is no ninth account");
    press(&app, "1");
    assert_eq!(app.get_active_account(), 0);
    app.window().dispatch_event(WindowEvent::KeyReleased { text: control.into() });

    // Mail for the account in the background moves its count and leaves the screen alone.
    press(&app, "j");
    let (open, subject) = (app.get_selected_row(), app.get_message().subject);
    store
        .save_messages(
            &work_inbox,
            &[NewMessage {
                uid: 200,
                remote_id: "background".into(),
                gm_thread_id: None,
                mid: None,
                references: vec![],
                subject: "For the other account".into(),
                from_name: "Bg".into(),
                from_addr: "bg@example.com".into(),
                date: NOW,
                size: 1,
                seen: false,
                flagged: false,
                labels: vec![],
            }],
        )
        .unwrap();
    controller.on_event(work, Event::Changed);
    std::thread::sleep(Duration::from_millis(450));
    settle();
    assert_eq!(app.get_accounts().row_data(1).unwrap().unread, "1");
    // The taskbar counts every account, not just the one on screen.
    let per_account: i64 = app.get_accounts().iter().map(|a| a.unread.parse::<i64>().unwrap_or(0)).sum();
    assert!(per_account >= 1);
    assert_eq!(controller.unread(), per_account);
    assert_eq!((app.get_selected_row(), app.get_message().subject), (open, subject));
    assert_eq!(app.get_rows().row_count(), MAIL.len() + 1);

    // A body for the other account's message cannot land in this one's pane.
    controller.on_event(work, Event::BodyFailed(-1));
    assert_ne!(app.get_pane(), Pane::Failed);

    // One account losing its sign-in stops only that account, and its mail stays readable.
    controller.on_event(work, Event::SignInRequired("token revoked".into()));
    assert_eq!(status(1), "Needs sign-in");
    assert!(app.get_signed_in() && !app.get_needs_sign_in());
    controller.select_account(1);
    assert!(app.get_needs_sign_in());
    assert_eq!(app.get_rows().row_count(), 5);
    settle();
    shot(&app, "08-signed-out");

    // Sign-in covers the mail, and leaving it brings the message back.
    press(&app, "j");
    let reading = app.get_message().subject;
    app.invoke_sign_in_again();
    assert!(app.get_adding());
    assert_eq!(app.get_sign_in_message().as_str(), "");
    settle();
    shot(&app, "08-add-account");
    press(&app, "\u{1b}");
    assert!(!app.get_adding());
    assert_eq!(app.get_message().subject, reading);
    app.invoke_add_account();
    assert!(app.get_adding());
    click(&app, 40.0, 26.0);
    assert!(!app.get_adding(), "the back button");

    // New mail: only what is unread in the Inbox is news, and a notice says who it is from.
    let work_spam = store.folder(work, "Spam", &LabelSpec::system(Role::Spam)).unwrap();
    let next_uid = Cell::new(300);
    let arrive = |folder: &Folder, name: &str, address: &str, seen: bool| {
        next_uid.set(next_uid.get() + 1);
        let message = NewMessage {
            uid: next_uid.get(),
            remote_id: format!("new-{}", next_uid.get()),
            gm_thread_id: None,
            mid: None,
            references: vec![],
            subject: format!("Message {}", next_uid.get()),
            from_name: name.into(),
            from_addr: address.into(),
            date: NOW + i64::from(next_uid.get()),
            size: 1,
            seen,
            flagged: false,
            labels: vec![],
        };
        store.save_messages(folder, &[message]).unwrap()[0]
    };
    let person = arrive(&work_inbox, "Ann <Lee>", "ann@gmail.com", false);
    let read_elsewhere = arrive(&work_inbox, "Read", "read@gmail.com", true);
    let junk = arrive(&work_spam, "Junk", "junk@gmail.com", false);
    controller.on_event(work, Event::Arrived(vec![person, read_elsewhere, junk]));
    let notice = notices.borrow().last().unwrap().clone();
    assert_eq!(notices.borrow().len(), 1);
    assert_eq!((notice.title.as_str(), notice.text.as_str()), ("Ann <Lee>", "Message 301"));
    assert_eq!(notice.account, "work@studio.example");
    assert_eq!(notice.opens, Opens { account: work, message: Some(person) });
    let (size, centre) = picture_centre(notice.picture.as_ref().unwrap());
    assert_eq!((size, centre[3]), (128, 255), "initials stand in for a logo");

    // A sender with a logo shows it, and the newest of a few ends up on top.
    notices.borrow_mut().clear();
    let older = arrive(&work_inbox, "Linear", "n@linear.example", false);
    let newer = arrive(&work_inbox, "Ann", "ann@gmail.com", false);
    controller.on_event(work, Event::Arrived(vec![newer, older]));
    let shown: Vec<_> = notices.borrow().iter().map(|n| n.opens.message).collect();
    assert_eq!(shown, [Some(older), Some(newer)]);
    let logo = notices.borrow()[0].picture.clone().unwrap();
    assert_eq!(picture_centre(&logo).1, [94, 106, 210, 255]);

    // A flood is one notice, naming a few senders once each.
    notices.borrow_mut().clear();
    let flood = ["Ann", "Ann", "Bob", "Cy", "Di"].map(|name| arrive(&work_inbox, name, "x@gmail.com", false));
    controller.on_event(work, Event::Arrived(flood.to_vec()));
    let summary = notices.borrow()[0].clone();
    assert_eq!(notices.borrow().len(), 1);
    assert_eq!((summary.title.as_str(), summary.text.as_str()), ("5 new emails", "Di, Cy, Bob"));
    assert_eq!((summary.picture.as_ref(), summary.opens.message), (None, None));

    // Nothing to tell, nothing shown.
    notices.borrow_mut().clear();
    controller.on_event(work, Event::Arrived(vec![read_elsewhere, junk]));
    assert!(notices.borrow().is_empty());

    // A click leads to the message, wherever the app was.
    controller.select_account(0);
    app.invoke_add_account();
    controller.open(notice.opens);
    assert!(!app.get_adding());
    assert_eq!((app.get_active_account(), app.get_active_label()), (1, 0));
    assert_eq!(controller.selected_message(), Some(person));
    assert_eq!(app.get_message().sender, "Ann <Lee>");
    assert_eq!(app.get_rows().row_data(app.get_selected_row() as usize).unwrap().sender, "Ann <Lee>");
    settle();
    shot(&app, "09-opened-from-notification");

    // A summary leads to the Inbox, and an account since removed leads nowhere.
    controller.select_label(1);
    controller.open(summary.opens);
    assert_eq!((app.get_active_label(), controller.selected_message()), (0, None));
    controller.open(Opens { account: 999, message: Some(person) });
    assert_eq!(app.get_active_account(), 1);

    // Removing an account: hovering offers it, and it takes a second click.
    let work_row_y = rows_y + 46.0 + 22.0;
    click(&app, 200.0, work_row_y);
    assert_eq!(app.get_accounts().row_count(), 2, "the first click only asks");
    settle();
    shot(&app, "08-confirm-remove");
    click(&app, 188.0, work_row_y);
    assert_eq!(app.get_accounts().row_count(), 1);
    assert_eq!(app.get_active_account(), 0, "the remaining account takes over");
    assert_eq!(app.get_rows().row_count(), MAIL.len() + 1);
    assert!(!app.get_needs_sign_in());

    controller.select_label(0);
    press(&app, "j");

    for (scale, size) in [(1.5, (1770, 1140)), (2.0, (1520, 840))] {
        window.dispatch_event(WindowEvent::ScaleFactorChanged { scale_factor: scale });
        window.set_size(PhysicalSize::new(size.0, size.1));
        settle();
        shot(&app, &format!("06-scale-{scale}"));
    }
    window.dispatch_event(WindowEvent::ScaleFactorChanged { scale_factor: 1.0 });
    window.set_size(PhysicalSize::new(900, 420));
    settle();
    shot(&app, "07-smallest");

    // The last account going leaves the sign-in screen. Its data goes on a background thread.
    // Spam gets no logos, even from a sender whose logo is already known.
    let last = reader.accounts().unwrap()[0].id;
    let spam = store.folder(last, "[Gmail]/Spam", &LabelSpec::system(Role::Spam)).unwrap();
    let all = store.folder(last, "Linear mail", &LabelSpec::user("Linear mail")).unwrap();
    for (folder, id) in [(&spam, "from-spam"), (&all, "from-linear")] {
        let message = NewMessage {
            uid: 1,
            remote_id: id.into(),
            gm_thread_id: None,
            mid: None,
            references: vec![],
            subject: id.into(),
            from_name: "Linear".into(),
            from_addr: "n@linear.example".into(),
            date: NOW,
            size: 1000,
            seen: false,
            flagged: false,
            labels: Vec::new(),
        };
        store.save_messages(folder, &[message]).unwrap();
    }
    controller.refresh_now();
    let label = |name: &str| app.get_labels().iter().position(|l| l.name == name).unwrap();
    controller.select_label(label("Linear mail"));
    let row = app.get_rows().row_data(0).unwrap();
    assert_eq!((row.subject.as_str(), row.logo.size().width), ("from-linear", 38));
    controller.select_label(label("Spam"));
    let row = app.get_rows().row_data(0).unwrap();
    assert_eq!((row.subject.as_str(), row.logo.size().width), ("from-spam", 0));

    controller.remove_account(0);
    assert!(!app.get_signed_in());
    assert_eq!(app.get_accounts().row_count(), 0);
    assert_eq!(app.get_pane(), Pane::Empty);
    let deadline = Instant::now() + Duration::from_secs(5);
    while !reader.accounts().unwrap().is_empty() {
        assert!(Instant::now() < deadline, "the accounts were not deleted");
        std::thread::sleep(Duration::from_millis(20));
    }
    // With nobody signed in, nothing is left to say who wrote.
    assert!(reader.logo("linear.example").unwrap().is_none());

    drop(controller);
    drop(reader);
    drop(store);
    let _ = std::fs::remove_dir_all(&dir);
}
