//! The queue, threads, drafts and search against a real store, with no server. The server's side
//! of these is in `dovecot.rs` and `gmail_wire.rs`.

use mailfunction_core::compose::{Attachment, Compose};
use mailfunction_core::ops::Triage;
use mailfunction_core::settings::{Identity, Settings};
use mailfunction_core::store::{Account, AuthKind, Change, Folder, LabelRow, LabelSpec, NewMessage, Reader, Role, Security, Store};

struct Fixture {
    dir: std::path::PathBuf,
    store: Store,
    reader: Reader,
    account: i64,
    all: Folder,
    trash: Folder,
    spam: Folder,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// A Gmail-shaped account: mail lives in All Mail and carries labels.
fn fixture(name: &str) -> Fixture {
    let dir = std::env::temp_dir().join(format!("mailfunction-queue-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let store = Store::open(&dir).unwrap();
    let reader = Reader::open(&dir).unwrap();
    let account = store
        .add_account(&Account {
            id: 0,
            email: "me@example.com".into(),
            host: "localhost".into(),
            port: 993,
            security: Security::Tls,
            auth: AuthKind::Password,
            smtp_host: "localhost".into(),
            smtp_port: 465,
        })
        .unwrap();
    store.ensure_label(account, &LabelSpec::system(Role::Inbox)).unwrap();
    let all = store.folder(account, "[Gmail]/All Mail", &LabelSpec::system(Role::All)).unwrap();
    let trash = store.folder(account, "[Gmail]/Trash", &LabelSpec::system(Role::Trash)).unwrap();
    let spam = store.folder(account, "[Gmail]/Spam", &LabelSpec::system(Role::Spam)).unwrap();
    Fixture { dir, store, reader, account, all, trash, spam }
}

fn message(uid: u32, thread: u64, date: i64) -> NewMessage {
    NewMessage {
        uid,
        remote_id: format!("gm:{uid}"),
        gm_thread_id: Some(thread),
        mid: Some(format!("{uid}@x")),
        references: vec![],
        subject: format!("Subject {uid}"),
        from_name: "Ann".into(),
        from_addr: "ann@example.com".into(),
        verified: false,
        date,
        size: 100,
        seen: false,
        flagged: false,
        labels: vec![LabelSpec::system(Role::Inbox)],
    }
}

impl Fixture {
    fn label(&self, name: &str) -> LabelRow {
        self.reader.labels(self.account).unwrap().into_iter().find(|l| l.name == name).unwrap()
    }

    fn subjects(&self, name: &str) -> Vec<String> {
        self.reader.messages(&[self.label(name).id], 0, 100).unwrap().into_iter().map(|m| m.subject).collect()
    }

    fn id(&self, name: &str, row: usize) -> i64 {
        self.reader.messages(&[self.label(name).id], row, 1).unwrap()[0].id
    }
}

#[test]
fn a_draft_moves_to_another_account_with_everything_in_it() {
    let f = fixture("move-draft");
    let work = Account { email: "work@example.com".into(), ..f.reader.accounts().unwrap()[0].clone() };
    let work = f.store.add_account(&work).unwrap();
    let compose = Compose {
        to: "a@example.com".into(),
        subject: "Quote".into(),
        body: "Here it is".into(),
        in_reply_to: "<root@x>".into(),
        ..Compose::default()
    };
    let file = Attachment { content_id: None, name: "quote.pdf".into(), mime: "application/pdf".into(), data: b"PDF".to_vec() };
    let draft = f.store.create_draft(f.account, &compose, None, std::slice::from_ref(&file)).unwrap();

    let moved = f.store.move_draft(draft, work).unwrap();
    assert_eq!(f.reader.account_of(moved).unwrap(), Some(work));
    assert_eq!(f.reader.draft(moved).unwrap().unwrap(), compose);
    assert_eq!(f.reader.draft_attachments(moved).unwrap().len(), 1);
    assert_eq!(f.reader.account_of(draft).unwrap(), None, "it is in one place, not two");
    assert!(f.subjects("Drafts").is_empty());
    assert_eq!(f.store.pending_count(f.account).unwrap(), 0, "a draft never uploaded has nothing to take back");

    assert!(f.store.move_draft(draft, f.account).is_err(), "there is nothing left to move");
}

#[test]
fn a_list_shows_one_row_per_thread_led_by_its_newest_message() {
    let f = fixture("heads");
    f.store.save_messages(&f.all, &[message(1, 7, 10), message(2, 7, 20), message(3, 8, 15)]).unwrap();
    assert_eq!(f.subjects("Inbox"), ["Subject 2", "Subject 3"]);
    assert_eq!(f.reader.message_count(&[f.label("Inbox").id]).unwrap(), 2);

    let row = &f.reader.messages(&[f.label("Inbox").id], 0, 1).unwrap()[0];
    assert_eq!(row.count, 2);
    let thread = f.reader.thread(f.account, row.thread_id, false).unwrap();
    assert_eq!(thread.iter().map(|m| m.subject.as_str()).collect::<Vec<_>>(), ["Subject 1", "Subject 2"]);
    assert_eq!(f.reader.position(&[f.label("Inbox").id], row.thread_key()).unwrap(), Some(0));

    // A later reply moves the thread up, whichever batch it came in.
    f.store.save_messages(&f.all, &[message(4, 8, 30)]).unwrap();
    assert_eq!(f.subjects("Inbox"), ["Subject 4", "Subject 2"]);
    assert_eq!(f.reader.message_count(&[f.label("Inbox").id]).unwrap(), 2);
}

#[test]
fn a_thread_is_unread_if_any_of_it_is_and_starred_if_any_of_it_is() {
    let f = fixture("state");
    let mut old = message(1, 7, 10);
    old.flagged = true;
    let mut new = message(2, 7, 20);
    new.seen = true;
    f.store.save_messages(&f.all, &[old, new]).unwrap();
    let row = &f.reader.messages(&[f.label("Inbox").id], 0, 1).unwrap()[0];
    assert!(!row.seen && row.flagged, "{row:?}");
}

#[test]
fn servers_without_threads_get_them_from_references() {
    let f = fixture("refs");
    let plain = |uid: u32, mid: &str, references: &[&str]| NewMessage {
        gm_thread_id: None,
        mid: Some(mid.into()),
        references: references.iter().map(|r| r.to_string()).collect(),
        ..message(uid, 0, uid as i64)
    };
    // The reply arrives first, as a newest-first backfill delivers it.
    f.store.save_messages(&f.all, &[plain(3, "c@x", &["a@x", "b@x"])]).unwrap();
    f.store.save_messages(&f.all, &[plain(1, "a@x", &[]), plain(9, "other@x", &[])]).unwrap();
    assert_eq!(f.subjects("Inbox"), ["Subject 9", "Subject 3"]);

    // A message that mentions two threads joins them.
    f.store.save_messages(&f.all, &[plain(10, "d@x", &["c@x", "other@x"])]).unwrap();
    assert_eq!(f.subjects("Inbox"), ["Subject 10"]);
    assert_eq!(f.reader.messages(&[f.label("Inbox").id], 0, 1).unwrap()[0].count, 4);
}

#[test]
fn reading_applies_at_once_and_survives_a_stale_sync() {
    let f = fixture("overlay");
    f.store.save_messages(&f.all, &[message(1, 1, 10)]).unwrap();
    let id = f.id("Inbox", 0);
    f.store.act(f.account, &[id], Triage::Seen(true)).unwrap();
    assert_eq!(f.label("Inbox").unread, 0);
    assert_eq!(f.store.pending_count(f.account).unwrap(), 1);

    // Sync fetched this before the click, and writes it after.
    f.store.apply_changes(&f.all, &[Change { uid: 1, seen: false, flagged: false, labels: None }]).unwrap();
    f.store.save_messages(&f.all, &[message(1, 1, 10)]).unwrap();
    assert_eq!(f.label("Inbox").unread, 0, "the server has not heard yet, and must not win");
}

#[test]
fn doing_what_is_already_so_queues_nothing() {
    let f = fixture("noop");
    f.store.save_messages(&f.all, &[message(1, 1, 10)]).unwrap();
    let id = f.id("Inbox", 0);
    assert!(f.store.act(f.account, &[id], Triage::Seen(false)).unwrap().is_empty());
    assert!(f.store.act(f.account, &[id], Triage::Starred(false)).unwrap().is_empty());
    assert!(f.store.act(f.account, &[id], Triage::Inbox).unwrap().is_empty());
    assert_eq!(f.store.pending_count(f.account).unwrap(), 0);
}

#[test]
fn archiving_leaves_all_mail_and_undo_brings_it_back() {
    let f = fixture("archive");
    f.store.save_messages(&f.all, &[message(1, 1, 10)]).unwrap();
    let id = f.id("Inbox", 0);
    let undo = f.store.act(f.account, &[id], Triage::Archive).unwrap();
    assert!(f.subjects("Inbox").is_empty());
    assert_eq!(f.subjects("All Mail"), ["Subject 1"]);

    f.store.undo(f.account, &undo).unwrap();
    assert_eq!(f.subjects("Inbox"), ["Subject 1"]);
    assert_eq!(f.store.pending_count(f.account).unwrap(), 2, "the server hears both");
}

#[test]
fn trash_takes_mail_out_of_every_label_until_undone() {
    let f = fixture("trash");
    let mut m = message(1, 1, 10);
    m.flagged = true;
    m.labels.push(LabelSpec::user("Work"));
    f.store.save_messages(&f.all, &[m]).unwrap();
    let id = f.id("Inbox", 0);

    let undo = f.store.act(f.account, &[id], Triage::Trash).unwrap();
    for label in ["Inbox", "All Mail", "Work", "Starred"] {
        assert!(f.subjects(label).is_empty(), "{label}");
    }
    assert_eq!(f.subjects("Trash"), ["Subject 1"]);
    // A list of what is in the bin and a thread of what is not see different things.
    let row = &f.reader.messages(&[f.label("Trash").id], 0, 1).unwrap()[0];
    assert!(f.reader.thread(f.account, row.thread_id, false).unwrap().is_empty());
    assert_eq!(f.reader.thread(f.account, row.thread_id, true).unwrap().len(), 1);

    f.store.undo(f.account, &undo).unwrap();
    for label in ["Inbox", "All Mail", "Work", "Starred"] {
        assert_eq!(f.subjects(label), ["Subject 1"], "{label}");
    }
    assert!(f.subjects("Trash").is_empty());
}

#[test]
fn a_pending_move_outlasts_sync_reporting_the_old_place() {
    let f = fixture("pending-move");
    f.store.save_messages(&f.all, &[message(1, 1, 10)]).unwrap();
    let id = f.id("Inbox", 0);
    f.store.act(f.account, &[id], Triage::Spam).unwrap();
    f.store.save_messages(&f.all, &[message(1, 1, 10)]).unwrap();
    assert!(f.subjects("Inbox").is_empty());
    assert_eq!(f.subjects("Spam"), ["Subject 1"]);
}

#[test]
fn not_spam_returns_mail_to_the_inbox() {
    let f = fixture("not-spam");
    let mut m = message(1, 1, 10);
    m.labels.clear();
    f.store.save_messages(&f.spam, &[m]).unwrap();
    let id = f.id("Spam", 0);
    f.store.act(f.account, &[id], Triage::Inbox).unwrap();
    assert_eq!(f.subjects("Inbox"), ["Subject 1"]);
    assert_eq!(f.subjects("All Mail"), ["Subject 1"]);
    assert!(f.subjects("Spam").is_empty());
}

#[test]
fn mail_in_the_bin_shows_nowhere_else_whatever_labels_it_keeps() {
    let f = fixture("bin-labels");
    f.store.ensure_label(f.account, &LabelSpec::user("Work")).unwrap();
    let mut m = message(1, 1, 10);
    m.labels = vec![LabelSpec::user("Work")];
    m.flagged = true;
    f.store.save_messages(&f.trash, &[m]).unwrap();
    assert_eq!(f.subjects("Trash"), ["Subject 1"]);
    assert!(f.subjects("Work").is_empty());
    assert!(f.subjects("Starred").is_empty());
}

#[test]
fn a_label_is_added_and_removed() {
    let f = fixture("label");
    f.store.ensure_label(f.account, &LabelSpec::user("Work")).unwrap();
    f.store.save_messages(&f.all, &[message(1, 1, 10)]).unwrap();
    let id = f.id("Inbox", 0);
    let label = f.label("Work").id;
    f.store.act(f.account, &[id], Triage::Label { label, on: true }).unwrap();
    assert_eq!(f.subjects("Work"), ["Subject 1"]);
    f.store.act(f.account, &[id], Triage::Label { label, on: false }).unwrap();
    assert!(f.subjects("Work").is_empty());
}

#[test]
fn archiving_part_of_a_thread_keeps_the_rest_listed() {
    let f = fixture("partial");
    f.store.save_messages(&f.all, &[message(1, 7, 10), message(2, 7, 20)]).unwrap();
    let newest = f.id("Inbox", 0);
    f.store.act(f.account, &[newest], Triage::Archive).unwrap();
    assert_eq!(f.subjects("Inbox"), ["Subject 1"], "the older message takes over the row");
}

fn compose() -> Compose {
    Compose { to: "a@x.example".into(), subject: "Kế hoạch".into(), body: "Nội dung".into(), ..Compose::default() }
}

#[test]
fn a_draft_appears_at_once_and_goes_when_discarded() {
    let f = fixture("draft");
    let id = f.store.create_draft(f.account, &compose(), None, &[]).unwrap();
    assert_eq!(f.subjects("Drafts"), ["Kế hoạch"]);
    assert_eq!(f.reader.draft(id).unwrap().unwrap().to, "a@x.example");

    f.store.update_draft(id, &Compose { subject: "New".into(), ..compose() }).unwrap();
    assert_eq!(f.subjects("Drafts"), ["New"]);
    assert_eq!(f.store.pending_count(f.account).unwrap(), 0, "autosave stays local");

    f.store.discard_draft(id).unwrap();
    assert!(f.subjects("Drafts").is_empty());
    assert_eq!(f.reader.draft(id).unwrap(), None);
    assert_eq!(f.store.pending_count(f.account).unwrap(), 0, "the server never had it");
}

#[test]
fn a_server_copy_of_a_draft_stays_the_same_message() {
    let f = fixture("adopt");
    let id = f.store.create_draft(f.account, &compose(), None, &[]).unwrap();
    f.store.upload_draft(id).unwrap();
    let raw = f.reader.body(id).unwrap().unwrap();
    let mid = mail_parser::MessageParser::default().parse(&raw).unwrap().root().message_id().unwrap().to_string();

    let mut copy = message(50, 99, 10);
    copy.mid = Some(mid);
    copy.labels = vec![LabelSpec::system(Role::Drafts)];
    f.store.save_messages(&f.all, &[copy]).unwrap();

    assert_eq!(f.reader.messages(&[f.label("Drafts").id], 0, 10).unwrap().len(), 1);
    assert_eq!(f.id("Drafts", 0), id);
    assert_eq!(f.reader.location(id).unwrap().unwrap().1, 50);
    assert!(f.reader.draft(id).unwrap().is_some());
}

/// Gmail files mail sent to oneself as one message, in Sent and in the Inbox, which comes back
/// as the row made here for sending it. It is still mail that arrived.
#[test]
fn mail_to_oneself_is_news_and_mail_to_others_is_not() {
    let f = fixture("to-oneself");
    let server_copy = |uid, labels: Vec<LabelSpec>| {
        let id = f.store.create_draft(f.account, &compose(), None, &[]).unwrap();
        f.store.upload_draft(id).unwrap();
        let raw = f.reader.body(id).unwrap().unwrap();
        let mid = mail_parser::MessageParser::default().parse(&raw).unwrap().root().message_id().unwrap().to_string();
        (id, NewMessage { mid: Some(mid), labels, ..message(uid, uid.into(), 10) })
    };

    let (mine, copy) = server_copy(50, vec![LabelSpec::system(Role::Inbox), LabelSpec::system(Role::Sent)]);
    let arrived = f.store.save_messages(&f.all, &[copy]).unwrap();
    assert_eq!(f.reader.unread_in_inbox(&arrived).unwrap().iter().map(|m| m.id).collect::<Vec<_>>(), [mine]);

    let (_, copy) = server_copy(51, vec![LabelSpec::system(Role::Sent)]);
    let arrived = f.store.save_messages(&f.all, &[copy]).unwrap();
    assert!(f.reader.unread_in_inbox(&arrived).unwrap().is_empty());
}

#[test]
fn a_draft_from_another_client_can_be_taken_over() {
    let f = fixture("foreign-draft");
    let mut m = message(1, 1, 10);
    m.labels = vec![LabelSpec::system(Role::Drafts)];
    f.store.save_messages(&f.all, &[m]).unwrap();
    let id = f.id("Drafts", 0);
    assert_eq!(f.reader.draft(id).unwrap(), None);
    let raw = b"From: me@example.com\r\nTo: a@x.example\r\nSubject: Half done\r\nMessage-ID: <web@x>\r\n\r\nSo far\r\n";
    let fields = f.store.adopt_draft(id, raw).unwrap().unwrap();
    assert_eq!((fields.to.as_str(), fields.subject.as_str()), ("a@x.example", "Half done"));
    assert_eq!(f.reader.draft(id).unwrap().unwrap().subject, "Half done");

    // Discarding must take the server's copy too, which needs its Message-ID.
    f.store.discard_draft(id).unwrap();
    assert_eq!(f.store.pending_count(f.account).unwrap(), 1);
}

#[test]
fn a_reply_joins_its_thread_but_waits_there_unseen() {
    let f = fixture("reply");
    f.store.save_messages(&f.all, &[message(1, 7, 10)]).unwrap();
    let original = f.id("Inbox", 0);
    let reply = Compose { in_reply_to: "1@x".into(), references: "1@x".into(), ..compose() };
    let id = f.store.create_draft(f.account, &reply, Some(original), &[]).unwrap();
    let thread = f.reader.messages(&[f.label("Inbox").id], 0, 1).unwrap()[0].thread_id;
    assert_eq!(f.reader.messages(&[f.label("Drafts").id], 0, 1).unwrap()[0].thread_id, thread);
    assert_eq!(f.reader.thread(f.account, thread, false).unwrap().len(), 1, "a draft is not part of the conversation");
    assert_eq!(f.id("Drafts", 0), id);
}

#[test]
fn only_mail_in_drafts_can_become_a_draft() {
    let f = fixture("not-a-draft");
    f.store.save_messages(&f.all, &[message(1, 1, 10)]).unwrap();
    let received = f.id("Inbox", 0);
    let raw = b"From: a@x.example\r\nSubject: Hello\r\nMessage-ID: <1@x>\r\n\r\nHi\r\n";
    assert_eq!(f.store.adopt_draft(received, raw).unwrap(), None);
    assert_eq!(f.reader.draft(received).unwrap(), None);
    assert_eq!(f.subjects("Inbox"), ["Subject 1"]);
}

#[test]
fn mail_restored_elsewhere_comes_back_while_its_old_copy_is_still_listed() {
    let f = fixture("restored");
    let mut m = message(1, 1, 10);
    m.labels.clear();
    f.store.save_messages(&f.trash, &[m]).unwrap();

    // A client elsewhere restored it. All Mail is read first, with Trash not yet looked at.
    let mut back = message(1, 1, 10);
    back.uid = 7;
    f.store.save_messages(&f.all, &[back]).unwrap();
    assert_eq!(f.subjects("Inbox"), ["Subject 1"], "it belongs where the newest folder says");

    f.store.remove_uids(&f.trash, &[1]).unwrap();
    assert_eq!(f.subjects("Inbox"), ["Subject 1"]);
    assert_eq!(f.subjects("All Mail"), ["Subject 1"]);
    assert!(f.subjects("Trash").is_empty());
}

#[test]
fn a_draft_outlives_its_copy_on_the_server() {
    let f = fixture("draft-copy-gone");
    let id = f.store.create_draft(f.account, &compose(), None, &[]).unwrap();
    f.store.upload_draft(id).unwrap();
    let raw = f.reader.body(id).unwrap().unwrap();
    let mid = mail_parser::MessageParser::default().parse(&raw).unwrap().root().message_id().unwrap().to_string();
    let mut copy = message(50, 99, 10);
    copy.mid = Some(mid);
    copy.labels = vec![LabelSpec::system(Role::Drafts)];
    f.store.save_messages(&f.all, &[copy]).unwrap();

    // A phone deleted the copy. What is being written here is still being written.
    f.store.remove_uids(&f.all, &[50]).unwrap();
    f.store.purge_orphans(f.account).unwrap();
    assert_eq!(f.reader.draft(id).unwrap().unwrap().subject, "Kế hoạch");
    f.store.update_draft(id, &Compose { subject: "Still here".into(), ..compose() }).unwrap();
    assert_eq!(f.subjects("Drafts"), ["Still here"]);
    f.store.upload_draft(id).unwrap();
}

#[test]
fn mail_on_its_way_cannot_be_changed() {
    let f = fixture("no-edit-while-queued");
    let id = f.store.create_draft(f.account, &compose(), None, &[]).unwrap();
    f.store.queue_send(id, i64::MAX).unwrap();
    assert!(f.store.update_draft(id, &Compose { subject: "Half typed".into(), ..compose() }).is_err());
    assert!(f.store.upload_draft(id).is_err());
    assert!(f.store.queue_send(id, 0).is_err(), "and it cannot be queued twice");

    // Opening it takes it back first, and then it is an ordinary draft again.
    assert!(f.store.take_back(id).unwrap());
    f.store.update_draft(id, &Compose { subject: "Changed".into(), ..compose() }).unwrap();
    assert!(f.store.take_back(id).unwrap(), "already being edited");

    f.store.queue_send(id, 0).unwrap();
    f.store.simulate_crash_during_send(id).unwrap();
    assert!(!f.store.take_back(id).unwrap(), "too late while it is going");
}

#[test]
fn a_thread_that_gets_a_new_id_leaves_a_head_behind() {
    let f = fixture("rethreaded");
    f.store.save_messages(&f.all, &[message(1, 7, 10), message(2, 7, 20)]).unwrap();
    assert_eq!(f.subjects("Inbox"), ["Subject 2"]);
    // The newest message turns out to belong elsewhere.
    f.store.save_messages(&f.all, &[message(2, 8, 20)]).unwrap();
    assert_eq!(f.subjects("Inbox"), ["Subject 2", "Subject 1"]);
}

#[test]
fn queued_mail_can_be_called_back_until_it_goes() {
    let f = fixture("undo-send");
    let id = f.store.create_draft(f.account, &compose(), None, &[]).unwrap();
    f.store.queue_send(id, i64::MAX).unwrap();
    assert!(f.subjects("Drafts").is_empty());
    assert!(!f.reader.labels(f.account).unwrap().iter().any(|l| l.role == Role::Outbox), "only stuck mail shows there");

    assert!(f.store.cancel_send(id).unwrap());
    assert_eq!(f.subjects("Drafts"), ["Kế hoạch"]);
    assert!(!f.store.cancel_send(id).unwrap(), "nothing left to call back");
}

#[test]
fn attachments_stay_with_the_draft_within_the_limit() {
    let f = fixture("attach");
    let file = |n: &str, size| Attachment { content_id: None, name: n.into(), mime: "text/plain".into(), data: vec![b'x'; size] };
    let id = f.store.create_draft(f.account, &compose(), None, &[file("a.txt", 3)]).unwrap();
    f.store.attach(id, &file("b.txt", 5)).unwrap();
    let names = |f: &Fixture| f.reader.draft_attachments(id).unwrap().into_iter().map(|a| (a.name, a.size)).collect::<Vec<_>>();
    assert_eq!(names(&f), [("a.txt".to_string(), 3), ("b.txt".to_string(), 5)]);

    assert!(f.store.attach(id, &file("big.bin", 19 * 1024 * 1024)).is_err());
    assert_eq!(names(&f).len(), 2);

    f.store.remove_attachment(id, 0).unwrap();
    assert_eq!(names(&f), [("b.txt".to_string(), 5)]);
    f.store.discard_draft(id).unwrap();
    assert!(names(&f).is_empty());
}

#[test]
fn search_finds_words_and_prefixes_without_their_accents_and_skips_the_bin() {
    let f = fixture("search");
    let mut hit = message(1, 1, 10);
    hit.subject = "Lịch họp tuần sau".into();
    let mut body_hit = message(2, 2, 20);
    body_hit.subject = "Thông báo".into();
    let mut invoice = message(4, 4, 5);
    invoice.subject = "Hóa đơn tháng 10".into();
    let mut binned = message(3, 3, 30);
    binned.subject = "lịch trong thùng rác".into();
    f.store.save_messages(&f.all, &[hit, body_hit, binned, invoice]).unwrap();
    let body_id = f.id("Inbox", 1);
    f.store.save_body(body_id, "Subject: x\r\nContent-Type: text/plain; charset=utf-8\r\n\r\nBáo cáo doanh thu quý ba\r\n".as_bytes()).unwrap();
    let trashed = f.id("Inbox", 0);
    f.store.act(f.account, &[trashed], Triage::Trash).unwrap();

    let search = |q: &str| f.reader.search(f.account, q, 10).unwrap().into_iter().map(|m| m.subject).collect::<Vec<_>>();
    assert_eq!(search("lịch"), ["Lịch họp tuần sau"]);
    assert_eq!(search("lich hop"), ["Lịch họp tuần sau"], "typing without accents still finds it");
    assert_eq!(search("doanh thu quy"), ["Thông báo"], "in bodies too");
    assert_eq!(search("tu"), ["Lịch họp tuần sau"], "the last word may be half typed");
    assert_eq!(search("hoa don"), ["Hóa đơn tháng 10"], "đ is a letter of its own, and is still folded");
    assert_eq!(search("hóa đơn"), ["Hóa đơn tháng 10"]);
    assert_eq!(search("doanh thu"), ["Thông báo"], "bodies count once they are here");
    assert_eq!(search("ann@example"), ["Thông báo", "Lịch họp tuần sau", "Hóa đơn tháng 10"]);
    assert!(search("").is_empty());
    assert!(search("nothing-like-this").is_empty());
    // Query syntax has no meaning here, and cannot cause an error.
    for odd in ["\"", "a OR", "NOT", "subject:", "(", "*", "a\"b"] {
        f.reader.search(f.account, odd, 10).unwrap();
    }
}

#[test]
fn a_thread_is_found_once_and_deleted_mail_leaves_the_index() {
    let f = fixture("search-thread");
    let mut a = message(1, 7, 10);
    a.subject = "budget".into();
    let mut b = message(2, 7, 20);
    b.subject = "Re: budget".into();
    f.store.save_messages(&f.all, &[a, b]).unwrap();
    let found = f.reader.search(f.account, "budget", 10).unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].subject, "Re: budget");

    f.store.remove_uids(&f.all, &[1, 2]).unwrap();
    f.store.purge_orphans(f.account).unwrap();
    assert!(f.reader.search(f.account, "budget", 10).unwrap().is_empty());
}

#[test]
fn contacts_come_from_people_who_wrote() {
    let f = fixture("contacts");
    let person = |uid, name: &str, address: &str| NewMessage { from_name: name.into(), from_addr: address.into(), ..message(uid, uid as u64, 1) };
    f.store
        .save_messages(
            &f.all,
            &[
                person(1, "Nguyễn An", "an@x.example"),
                person(2, "Nguyễn An", "an@x.example"),
                person(3, "Anh Tuấn", "tuan@x.example"),
                person(4, "Service", "no-reply@x.example"),
                person(5, "100% Real", "real@x.example"),
            ],
        )
        .unwrap();
    let names = |q: &str| f.reader.contacts(f.account, q, 5).unwrap().into_iter().map(|c| c.address).collect::<Vec<_>>();
    assert_eq!(names("an"), ["an@x.example", "tuan@x.example"], "most frequent first, by address or by any name");
    assert_eq!(names("tu"), ["tuan@x.example"]);
    assert!(names("no").is_empty(), "nobody replies to no-reply");
    assert!(names("").is_empty());
    assert!(names("%").is_empty(), "a wildcard is just a character");
    assert_eq!(names("100%"), ["real@x.example"]);
}

#[test]
fn a_store_from_before_the_queue_migrates_with_its_mail() {
    let dir = std::env::temp_dir().join(format!("mailfunction-queue-migrate-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    {
        // The schema as it stood, with a body to index and a thread of two.
        let conn = rusqlite::Connection::open(dir.join("meta.db")).unwrap();
        conn.execute("ATTACH DATABASE ?1 AS bodies", [dir.join("bodies.db").to_string_lossy()]).unwrap();
        conn.execute_batch(include_str!("old_schema.sql")).unwrap();
        let body = zstd::encode_all(&b"Subject: x\r\n\r\nwords from the past\r\n"[..], 3).unwrap();
        conn.execute("INSERT INTO bodies.body VALUES (2, ?1)", [body]).unwrap();
    }
    let store = Store::open(&dir).unwrap();
    let reader = Reader::open(&dir).unwrap();
    let account = reader.accounts().unwrap()[0].clone();
    assert_eq!((account.smtp_host.as_str(), account.smtp_port), ("smtp.gmail.com", 465));
    let inbox = reader.labels(account.id).unwrap()[0].id;
    let rows = reader.messages(&[inbox], 0, 10).unwrap();
    assert_eq!(rows.iter().map(|m| (m.subject.as_str(), m.count)).collect::<Vec<_>>(), [("Second", 2), ("Alone", 1)]);
    assert_eq!(reader.search(account.id, "alone", 10).unwrap().len(), 1, "headers are indexed");
    assert_eq!(reader.search(account.id, "past", 10).unwrap()[0].subject, "Second", "and bodies already here");
    assert_eq!(reader.settings(), Settings::default());
    assert_eq!(reader.identity(account.id).unwrap(), Identity::default());
    drop((store, reader));
    let _ = std::fs::remove_dir_all(&dir);
}

/// Whether these words are anywhere in the store's files, as anyone with a disk tool would see.
fn on_disk(dir: &std::path::Path, words: &[u8]) -> Vec<String> {
    let files = std::fs::read_dir(dir).unwrap().flatten();
    let found = files.filter(|file| std::fs::read(file.path()).unwrap().windows(words.len()).any(|w| w == words));
    found.map(|file| file.file_name().to_string_lossy().into_owned()).collect()
}

#[test]
fn a_removed_account_leaves_no_mail_in_the_files() {
    let dir = std::env::temp_dir().join(format!("mailfunction-erased-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let store = Store::open(&dir).unwrap();
    let account = store
        .add_account(&Account {
            id: 0,
            email: "me@example.com".into(),
            host: "localhost".into(),
            port: 993,
            security: Security::Tls,
            auth: AuthKind::Password,
            smtp_host: "localhost".into(),
            smtp_port: 465,
        })
        .unwrap();
    let inbox = store.folder(account, "INBOX", &LabelSpec::system(Role::Inbox)).unwrap();
    let mut mail = message(1, 1, 10);
    mail.subject = "Xylophonesubject".into();
    mail.from_name = "Xylophonesender".into();
    let id = store.save_messages(&inbox, &[mail]).unwrap()[0];
    let raw = "From: a@example.com\r\nSubject: x\r\n\r\nXylophonebody and more\r\n";
    store.save_body(id, raw.as_bytes()).unwrap();
    let reader = Reader::open(&dir).unwrap();
    assert_eq!(reader.search(account, "xylophonesubject", 10).unwrap().len(), 1);
    for word in [&b"Xylophonesubject"[..], b"xylophonesender", b"xylophonebody"] {
        assert!(!on_disk(&dir, word).is_empty(), "{} was never there to remove", String::from_utf8_lossy(word));
    }

    store.remove_account(account).unwrap();
    // Closing the last connection folds the log into the databases, as ending the app would.
    drop((store, reader));
    for word in [&b"Xylophonesubject"[..], b"xylophonesubject", b"xylophonesender", b"xylophonebody"] {
        assert_eq!(on_disk(&dir, word), Vec::<String>::new(), "{} is still on disk", String::from_utf8_lossy(word));
    }
    let _ = std::fs::remove_dir_all(&dir);
}
