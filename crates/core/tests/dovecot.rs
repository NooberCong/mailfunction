//! Sync against a real IMAP server. Start one and run these with `scripts/test-imap.ps1`.
//!
//! An admin connection plays "the rest of the world": it delivers mail and changes flags,
//! then the test checks that sync notices.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use async_imap::types::Flag;
use async_imap::{Client, Session};
use futures::TryStreamExt;
use mailfunction_core::compose::{self, Attachment, Compose, reply};
use mailfunction_core::engine::{Engine, Event};
use mailfunction_core::imap::{Credential, Imap};
use mailfunction_core::ops::Triage;
use mailfunction_core::secrets;
use mailfunction_core::settings::Settings;
use mailfunction_core::store::{Account, AuthKind, Folder, LabelRow, MessageRow, Reader, Role, Security, Store};
use mailfunction_core::sync::{folders_to_sync, label_of, sync_account, sync_folder};
use tokio::net::TcpStream;

const PASSWORD: &str = "pass";

/// Another session may be using the usual port, so it can be moved.
fn port() -> u16 {
    std::env::var("MAILFUNCTION_IMAP_PORT").map_or(14143, |port| port.parse().unwrap())
}

struct Harness {
    dir: PathBuf,
    store: Arc<Store>,
    reader: Reader,
    account: Account,
    admin: Session<TcpStream>,
}

impl Drop for Harness {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn unique(name: &str) -> String {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    format!("{name}-{nanos}")
}

async fn harness(name: &str) -> Harness {
    let user = format!("{}@example.com", unique(name));
    let dir = std::env::temp_dir().join(format!("mailfunction-{user}"));
    let store = Arc::new(Store::open(&dir).unwrap());
    let mut account = Account {
        id: 0,
        email: user.clone(),
        host: "127.0.0.1".into(),
        port: port(),
        security: Security::Plain,
        auth: AuthKind::Password,
        smtp_host: "localhost".into(),
        smtp_port: 25,
    };
    account.id = store.add_account(&account).unwrap();
    let reader = Reader::open(&dir).unwrap();
    let tcp = TcpStream::connect(("127.0.0.1", port())).await.expect("start Dovecot: scripts/test-imap.ps1");
    let mut client = Client::new(tcp);
    client.read_response().await.unwrap();
    let admin = client.login(&user, PASSWORD).await.map_err(|(e, _)| e).unwrap();
    Harness { dir, store, reader, account, admin }
}

fn raw_message(id: &str, subject: &str, body: &str) -> Vec<u8> {
    format!(
        "From: =?UTF-8?Q?Nguy=E1=BB=85n_An?= <an@example.com>\r\nTo: me@example.com\r\nSubject: {subject}\r\n\
         Message-ID: <{id}@example.com>\r\nDate: Wed, 04 Nov 2026 10:00:00 +0700\r\n\
         Content-Type: text/plain; charset=utf-8\r\n\r\n{body}\r\n"
    )
    .into_bytes()
}

impl Harness {
    async fn connect(&self) -> Imap {
        Imap::connect(&self.account, &Credential::Password(PASSWORD.into())).await.unwrap()
    }

    async fn deliver(&mut self, folder: &str, id: &str, subject: &str) {
        let raw = raw_message(id, subject, "body text");
        self.admin.append(folder, None, None, &raw).await.unwrap();
    }

    async fn folders(&self, imap: &mut Imap) -> Vec<Folder> {
        folders_to_sync(imap.list().await.unwrap(), imap.gmail)
            .iter()
            .map(|f| self.store.folder(self.account.id, &f.path, &label_of(f)).unwrap())
            .collect()
    }

    /// One complete pass over every folder, on a fresh connection, as an app start would be.
    async fn sync(&self) {
        let mut imap = self.connect().await;
        sync_account(&mut imap, &self.store, self.account.id, |_| {}).await.unwrap();
    }

    fn label(&self, name: &str) -> LabelRow {
        self.reader.labels(self.account.id).unwrap().into_iter().find(|l| l.name == name).unwrap()
    }

    fn rows(&self, name: &str) -> Vec<MessageRow> {
        self.reader.messages(self.label(name).id, 0, 10_000).unwrap()
    }

    fn subjects(&self, name: &str) -> Vec<String> {
        let mut subjects: Vec<_> = self.rows(name).into_iter().map(|m| m.subject).collect();
        subjects.sort();
        subjects
    }

    async fn admin_store(&mut self, folder: &str, query: &str) {
        self.admin.select(folder).await.unwrap();
        let updates: Vec<_> = self.admin.store("1:*", query).await.unwrap().try_collect().await.unwrap();
        drop(updates);
    }
}

#[tokio::test]
#[ignore = "needs a local Dovecot: scripts/test-imap.ps1"]
async fn first_sync_decodes_headers_and_later_syncs_add_only_new_mail() {
    let mut h = harness("first").await;
    h.deliver("INBOX", "a", "=?UTF-8?B?Q2jDoG8gYuG6oW4=?=").await;
    h.deliver("INBOX", "b", "Second").await;
    h.sync().await;

    assert_eq!(h.subjects("Inbox"), ["Chào bạn", "Second"]);
    let row = &h.rows("Inbox")[0];
    assert_eq!(row.from_name, "Nguyễn An");
    assert_eq!(row.from_addr, "an@example.com");
    assert!(!row.seen);
    assert_eq!(h.label("Inbox").unread, 2);
    let ids: Vec<_> = h.rows("Inbox").iter().map(|m| m.id).collect();

    h.deliver("INBOX", "c", "Third").await;
    h.sync().await;
    assert_eq!(h.subjects("Inbox"), ["Chào bạn", "Second", "Third"]);
    let after: Vec<_> = h.rows("Inbox").iter().map(|m| m.id).collect();
    assert!(ids.iter().all(|id| after.contains(id)), "existing messages keep their identity");
}

#[tokio::test]
#[ignore = "needs a local Dovecot: scripts/test-imap.ps1"]
async fn flag_changes_arrive_through_condstore() {
    let mut h = harness("flags").await;
    h.deliver("INBOX", "a", "One").await;
    h.sync().await;
    assert_eq!(h.label("Inbox").unread, 1);

    h.admin_store("INBOX", "+FLAGS (\\Seen \\Flagged)").await;
    h.sync().await;
    let row = &h.rows("Inbox")[0];
    assert!(row.seen && row.flagged);
    assert_eq!(h.label("Inbox").unread, 0);
    assert_eq!(h.subjects("Starred"), ["One"]);

    h.admin_store("INBOX", "-FLAGS (\\Flagged)").await;
    h.sync().await;
    assert!(h.rows("Starred").is_empty());
    assert!(h.rows("Inbox")[0].seen);
}

#[tokio::test]
#[ignore = "needs a local Dovecot: scripts/test-imap.ps1"]
async fn expunged_mail_disappears_and_takes_its_body() {
    let mut h = harness("expunge").await;
    h.deliver("INBOX", "a", "Keep").await;
    h.deliver("INBOX", "b", "Delete").await;
    h.sync().await;
    let doomed = h.rows("Inbox").into_iter().find(|m| m.subject == "Delete").unwrap().id;
    h.store.save_body(doomed, b"raw").unwrap();

    h.admin.select("INBOX").await.unwrap();
    let _: Vec<_> = h.admin.store("2", "+FLAGS (\\Deleted)").await.unwrap().try_collect().await.unwrap();
    let _: Vec<_> = h.admin.expunge().await.unwrap().try_collect().await.unwrap();

    h.sync().await;
    assert_eq!(h.subjects("Inbox"), ["Keep"]);
    assert_eq!(h.reader.body(doomed).unwrap(), None);
}

#[tokio::test]
#[ignore = "needs a local Dovecot: scripts/test-imap.ps1"]
async fn a_moved_message_keeps_its_identity_and_body() {
    let mut h = harness("move").await;
    h.admin.create("Work").await.unwrap();
    h.deliver("INBOX", "a", "Moving").await;
    h.sync().await;
    let id = h.rows("Inbox")[0].id;
    h.store.save_body(id, b"raw").unwrap();

    h.admin.select("INBOX").await.unwrap();
    h.admin.mv("1", "Work").await.unwrap();
    h.sync().await;

    assert!(h.rows("Inbox").is_empty());
    let moved = h.rows("Work");
    assert_eq!(moved.len(), 1);
    assert_eq!(moved[0].id, id);
    assert!(h.reader.body(id).unwrap().is_some());
}

#[tokio::test]
#[ignore = "needs a local Dovecot: scripts/test-imap.ps1"]
async fn a_uidvalidity_change_rescans_without_losing_what_survived() {
    let mut h = harness("validity").await;
    h.admin.create("Work").await.unwrap();
    h.deliver("Work", "a", "Survivor").await;
    h.deliver("Work", "b", "Lost").await;
    h.sync().await;
    let survivor = h.rows("Work").into_iter().find(|m| m.subject == "Survivor").unwrap().id;
    h.store.save_body(survivor, b"raw").unwrap();

    // Dovecot derives UIDVALIDITY from the clock, so let it tick.
    tokio::time::sleep(Duration::from_millis(1100)).await;
    h.admin.delete("Work").await.unwrap();
    h.admin.create("Work").await.unwrap();
    h.deliver("Work", "new", "Newcomer").await;
    h.deliver("Work", "a", "Survivor").await;
    h.sync().await;

    assert_eq!(h.subjects("Work"), ["Newcomer", "Survivor"]);
    let row = h.rows("Work").into_iter().find(|m| m.subject == "Survivor").unwrap();
    assert_eq!(row.id, survivor);
    assert!(h.reader.body(survivor).unwrap().is_some());
}

#[tokio::test]
#[ignore = "needs a local Dovecot: scripts/test-imap.ps1"]
async fn a_folder_recreated_empty_loses_its_old_messages() {
    let mut h = harness("emptied").await;
    h.admin.create("Work").await.unwrap();
    h.deliver("Work", "a", "Old").await;
    h.sync().await;
    assert_eq!(h.subjects("Work"), ["Old"]);

    tokio::time::sleep(Duration::from_millis(1100)).await;
    h.admin.delete("Work").await.unwrap();
    h.admin.create("Work").await.unwrap();
    h.sync().await;
    assert!(h.rows("Work").is_empty());
}

#[tokio::test]
#[ignore = "needs a local Dovecot: scripts/test-imap.ps1"]
async fn an_interrupted_backfill_resumes_newest_first_with_nothing_missing() {
    let mut h = harness("backfill").await;
    for i in 0..1205 {
        h.deliver("INBOX", &format!("m{i}"), &format!("Message {i:04}")).await;
    }

    // One step, then the connection is gone.
    {
        let mut imap = h.connect().await;
        let mut folder = h.folders(&mut imap).await.remove(0);
        assert!(sync_folder(&mut imap, &h.store, &mut folder).await.unwrap().more_history);
    }
    let partial = h.subjects("Inbox");
    assert_eq!(partial.len(), 1000);
    assert_eq!(partial[0], "Message 0205", "the newest mail comes first");

    // New mail during the gap must not be missed either.
    h.deliver("INBOX", "late", "Late arrival").await;
    h.sync().await;
    let all = h.subjects("Inbox");
    assert_eq!(all.len(), 1206);
    assert_eq!(all[0], "Late arrival");
    assert_eq!(all[1], "Message 0000");
}

#[tokio::test]
#[ignore = "needs a local Dovecot: scripts/test-imap.ps1"]
async fn nothing_we_do_changes_the_mailbox() {
    let mut h = harness("readonly").await;
    h.deliver("INBOX", "a", "Untouched").await;
    h.sync().await;

    let mut imap = h.connect().await;
    let status = imap.examine("INBOX").await.unwrap();
    let raw = imap.fetch_body(1).await.unwrap().unwrap();
    assert!(String::from_utf8_lossy(&raw).contains("body text"));

    h.admin.select("INBOX").await.unwrap();
    let fetched: Vec<_> = h.admin.fetch("1", "FLAGS").await.unwrap().try_collect().await.unwrap();
    let flags: Vec<_> = fetched[0].flags().collect();
    assert!(!flags.contains(&Flag::Seen), "reading must not mark the message as seen: {flags:?}");
    assert_eq!(imap.examine("INBOX").await.unwrap().highest_modseq, status.highest_modseq);
}

async fn wait_for(events: &mut tokio::sync::mpsc::UnboundedReceiver<Event>, wanted: Event) {
    tokio::time::timeout(Duration::from_secs(20), async {
        while events.recv().await.expect("engine stopped") != wanted {}
    })
    .await
    .unwrap_or_else(|_| panic!("timed out waiting for {wanted:?}"));
}

#[tokio::test]
#[ignore = "needs a local Dovecot: scripts/test-imap.ps1"]
async fn the_engine_syncs_hears_new_mail_through_idle_and_fetches_bodies() {
    let mut h = harness("engine").await;
    h.deliver("INBOX", "a", "Existing").await;
    secrets::save(&h.account.email, PASSWORD).unwrap();

    let (tx, mut events) = tokio::sync::mpsc::unbounded_channel();
    let mut engine = Engine::start(h.dir.clone(), h.store.clone(), None, move |_, e| {
        let _ = tx.send(e);
    });
    engine.add(h.account.clone());
    wait_for(&mut events, Event::UpToDate).await;
    assert_eq!(h.subjects("Inbox"), ["Existing"]);

    // Give the engine a moment to enter IDLE, so this exercises the push rather than a poll.
    tokio::time::sleep(Duration::from_millis(500)).await;
    h.deliver("INBOX", "b", "Pushed").await;
    wait_for(&mut events, Event::Changed).await;
    assert_eq!(h.subjects("Inbox"), ["Existing", "Pushed"]);

    let id = h.rows("Inbox").into_iter().find(|m| m.subject == "Pushed").unwrap().id;
    assert!(engine.fetch_body(h.account.id, id));
    wait_for(&mut events, Event::BodyReady(id)).await;
    assert!(String::from_utf8_lossy(&h.reader.body(id).unwrap().unwrap()).contains("body text"));

    assert!(engine.fetch_body(h.account.id, 999_999));
    wait_for(&mut events, Event::BodyFailed(999_999)).await;

    secrets::delete(&h.account.email).unwrap();
}

#[tokio::test]
#[ignore = "needs a local Dovecot: scripts/test-imap.ps1"]
async fn the_setting_decides_how_much_mail_is_fetched_ahead() {
    let mut h = harness("offline").await;
    for (id, subject) in [("a", "One"), ("b", "Two"), ("c", "Three")] {
        h.deliver("INBOX", id, subject).await;
    }
    h.store.save_settings(&Settings { offline_messages: 1, ..Settings::default() }).unwrap();
    secrets::save(&h.account.email, PASSWORD).unwrap();

    let (tx, mut events) = tokio::sync::mpsc::unbounded_channel();
    let mut engine = Engine::start(h.dir.clone(), h.store.clone(), None, move |_, e| {
        let _ = tx.send(e);
    });
    engine.add(h.account.clone());
    wait_for(&mut events, Event::UpToDate).await;
    let stored = |h: &Harness| h.rows("Inbox").iter().filter(|m| h.reader.has_body(m.id).unwrap()).count();
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert_eq!(stored(&h), 1);

    // Raised, it takes effect with no restart, at the next mail.
    h.store.save_settings(&Settings { offline_messages: 3, ..Settings::default() }).unwrap();
    tokio::time::sleep(Duration::from_millis(500)).await;
    h.deliver("INBOX", "d", "Four").await;
    wait_for(&mut events, Event::Changed).await;
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert_eq!(stored(&h), 3);

    secrets::delete(&h.account.email).unwrap();
}

#[tokio::test]
#[ignore = "needs a local Dovecot: scripts/test-imap.ps1"]
async fn mail_that_arrives_after_the_body_connection_opened_its_folder_can_be_read() {
    let mut h = harness("late-body").await;
    h.deliver("INBOX", "a", "Existing").await;
    secrets::save(&h.account.email, PASSWORD).unwrap();

    let (tx, mut events) = tokio::sync::mpsc::unbounded_channel();
    let mut engine = Engine::start(h.dir.clone(), h.store.clone(), None, move |_, e| {
        let _ = tx.send(e);
    });
    engine.add(h.account.clone());
    wait_for(&mut events, Event::UpToDate).await;
    let first = h.rows("Inbox")[0].id;
    assert!(engine.fetch_body(h.account.id, first));
    wait_for(&mut events, Event::BodyReady(first)).await;

    tokio::time::sleep(Duration::from_millis(500)).await;
    h.deliver("INBOX", "b", "Just arrived").await;
    wait_for(&mut events, Event::Changed).await;
    let late = h.rows("Inbox").into_iter().find(|m| m.subject == "Just arrived").unwrap().id;
    assert!(engine.fetch_body(h.account.id, late));
    wait_for(&mut events, Event::BodyReady(late)).await;

    secrets::delete(&h.account.email).unwrap();
}

/// More than one command's worth, which a change of mind then has to chase through the queue.
#[tokio::test]
#[ignore = "needs a local Dovecot: scripts/test-imap.ps1"]
async fn marking_everything_read_reaches_the_server_in_bulk() {
    let mut h = harness("mark-all").await;
    const MAIL: usize = 1200;
    for n in 0..MAIL {
        h.deliver("INBOX", &format!("m{n}"), &format!("Mail {n}")).await;
    }
    h.sync().await;
    let inbox = h.reader.labels(h.account.id).unwrap().into_iter().find(|l| l.role == Role::Inbox).unwrap();
    let unread = h.reader.unread_in_label(inbox.id).unwrap();
    assert_eq!(unread.len(), MAIL);

    let undo = h.store.act(h.account.id, &unread, Triage::Seen(true)).unwrap();
    let started = std::time::Instant::now();
    h.sync().await;
    println!("{MAIL} marked read and synced in {:?}", started.elapsed());
    assert_eq!(h.store.pending_count(h.account.id).unwrap(), 0);
    h.admin.select("INBOX").await.unwrap();
    assert_eq!(h.admin.uid_search("UNSEEN").await.unwrap().len(), 0);

    // Both ways in one pass, in the order they were asked for.
    h.store.undo(h.account.id, &undo).unwrap();
    let undo = h.store.act(h.account.id, &unread[..10], Triage::Seen(true)).unwrap();
    assert!(!undo.is_empty());
    h.sync().await;
    assert_eq!(h.store.pending_count(h.account.id).unwrap(), 0);
    h.admin.noop().await.unwrap();
    assert_eq!(h.admin.uid_search("UNSEEN").await.unwrap().len(), MAIL - 10);
    assert_eq!(h.reader.unread_in_label(inbox.id).unwrap().len(), MAIL - 10);
}

#[tokio::test]
#[ignore = "needs a local Dovecot: scripts/test-imap.ps1"]
async fn only_mail_that_really_arrives_counts_as_arrived() {
    let mut h = harness("arrivals").await;
    h.deliver("INBOX", "old", "History").await;
    h.admin.create("Archive").await.unwrap();
    secrets::save(&h.account.email, PASSWORD).unwrap();

    let (tx, mut events) = tokio::sync::mpsc::unbounded_channel();
    let mut engine = Engine::start(h.dir.clone(), h.store.clone(), None, move |_, e| {
        let _ = tx.send(e);
    });
    engine.add(h.account.clone());
    let mut first_sync = Vec::new();
    tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            let event = events.recv().await.expect("engine stopped");
            let done = event == Event::UpToDate;
            first_sync.push(event);
            if done {
                break;
            }
        }
    })
    .await
    .unwrap();
    assert!(!first_sync.iter().any(|e| matches!(e, Event::Arrived(_))), "history is not news: {first_sync:?}");

    tokio::time::sleep(Duration::from_millis(500)).await;
    h.deliver("INBOX", "new", "Fresh").await;
    let arrived = tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            if let Event::Arrived(ids) = events.recv().await.expect("engine stopped") {
                return ids;
            }
        }
    })
    .await
    .unwrap();
    let rows = h.reader.unread_in_inbox(&arrived).unwrap();
    assert_eq!(rows.iter().map(|m| m.subject.as_str()).collect::<Vec<_>>(), ["Fresh"]);

    // Filed mail gets a new UID in its new folder, but it is the same message.
    wait_for(&mut events, Event::UpToDate).await;
    h.admin.select("INBOX").await.unwrap();
    h.admin.uid_copy("1:*", "Archive").await.unwrap();
    engine.sync_now(h.account.id);
    wait_for(&mut events, Event::Syncing).await;
    let mut after_copy = Vec::new();
    while let Ok(Some(event)) = tokio::time::timeout(Duration::from_secs(2), events.recv()).await {
        after_copy.push(event);
    }
    assert!(!after_copy.iter().any(|e| matches!(e, Event::Arrived(_))), "a copy is not news: {after_copy:?}");
    assert_eq!(h.rows("Archive").len(), 2, "the copies were never synced, so this proves nothing");

    secrets::delete(&h.account.email).unwrap();
}

#[tokio::test]
#[ignore = "needs a local Dovecot: scripts/test-imap.ps1"]
async fn accounts_sync_side_by_side_and_stop_one_at_a_time() {
    let mut h = harness("multi").await;
    h.deliver("INBOX", "a", "For the first").await;
    secrets::save(&h.account.email, PASSWORD).unwrap();

    // A second mailbox in the same store, and a third whose password is wrong.
    let second_user = format!("{}@example.com", unique("multi-second"));
    let second = Account { email: second_user.clone(), ..h.account.clone() };
    let second = Account { id: h.store.add_account(&second).unwrap(), ..second };
    secrets::save(&second.email, PASSWORD).unwrap();
    let third = Account { email: format!("{}@example.com", unique("multi-third")), ..h.account.clone() };
    let third = Account { id: h.store.add_account(&third).unwrap(), ..third };
    secrets::save(&third.email, "wrong").unwrap();

    let tcp = TcpStream::connect(("127.0.0.1", port())).await.unwrap();
    let mut client = Client::new(tcp);
    client.read_response().await.unwrap();
    let mut second_admin = client.login(&second_user, PASSWORD).await.map_err(|(e, _)| e).unwrap();
    second_admin.append("INBOX", None, None, &raw_message("b", "For the second", "words")).await.unwrap();

    let (tx, mut events) = tokio::sync::mpsc::unbounded_channel();
    let mut engine = Engine::start(h.dir.clone(), h.store.clone(), None, move |account, e| {
        let _ = tx.send((account, e));
    });
    for account in [&h.account, &second, &third] {
        engine.add(account.clone());
    }

    let (mut quiet, mut refused) = (HashSet::new(), false);
    tokio::time::timeout(Duration::from_secs(20), async {
        while quiet.len() < 2 || !refused {
            match events.recv().await.unwrap() {
                (account, Event::UpToDate) => drop(quiet.insert(account)),
                (account, Event::SignInRequired(_)) => {
                    assert_eq!(account, third.id);
                    refused = true;
                }
                _ => {}
            }
        }
    })
    .await
    .expect("both good accounts finish, and the bad one asks for sign-in");
    assert_eq!(quiet, HashSet::from([h.account.id, second.id]));

    // Each account's mail stays in that account, even with one Inbox name between them.
    let inbox = |id| h.reader.labels(id).unwrap().into_iter().find(|l| l.name == "Inbox").unwrap().id;
    let rows = |id| h.reader.messages(inbox(id), 0, 10).unwrap();
    assert_eq!(rows(h.account.id)[0].subject, "For the first");
    assert_eq!(rows(second.id)[0].subject, "For the second");

    // Bodies are fetched through the account that owns them, by ask or by prefetch.
    let id = rows(second.id)[0].id;
    if !h.reader.has_body(id).unwrap() {
        assert!(engine.fetch_body(second.id, id));
    }
    tokio::time::timeout(Duration::from_secs(10), async {
        while !h.reader.has_body(id).unwrap() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the body arrives");

    // The account that failed has stopped, so nobody waits on an answer that will not come.
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(!engine.fetch_body(third.id, 1));

    // Dropping one account leaves the other running.
    engine.remove(second.id);
    assert!(!engine.fetch_body(second.id, id));
    h.admin.append("INBOX", None, None, &raw_message("c", "Still here", "words")).await.unwrap();
    engine.sync_now(h.account.id);
    tokio::time::timeout(Duration::from_secs(20), async {
        while events.recv().await.unwrap() != (h.account.id, Event::Changed) {}
    })
    .await
    .expect("the remaining account still syncs");
    assert_eq!(rows(h.account.id).len(), 2);

    // Mail delivered to the removed account goes nowhere.
    second_admin.append("INBOX", None, None, &raw_message("d", "Too late", "words")).await.unwrap();
    tokio::time::sleep(Duration::from_millis(800)).await;
    assert_eq!(rows(second.id).len(), 1);

    for account in [&h.account, &second, &third] {
        secrets::delete(&account.email).unwrap();
    }
}

#[tokio::test]
#[ignore = "needs a local Dovecot: scripts/test-imap.ps1"]
async fn a_wrong_password_asks_for_sign_in_instead_of_retrying_forever() {
    let h = harness("badpass").await;
    let account = Account { email: format!("{}@", h.account.email), ..h.account.clone() };
    let error = Imap::connect(&Account { port: port(), ..account }, &Credential::Password(String::new())).await;
    assert!(matches!(error, Err(mailfunction_core::Error::Auth(_))), "{:?}", error.err());
}

// Everything below changes the mailbox, and an admin connection checks what really happened.

const SMTP_PORT: u16 = 11025;
const MAILPIT_API: &str = "http://127.0.0.1:18025/api/v1";

/// A mailbox with the folders a daily driver needs, and mail in its Inbox.
async fn mailbox(name: &str) -> Harness {
    let mut h = harness(name).await;
    h.account.smtp_host = "127.0.0.1".into();
    h.account.smtp_port = SMTP_PORT;
    h.store.add_account(&h.account).unwrap();
    for folder in ["Archive", "Trash", "Junk", "Sent", "Drafts"] {
        h.admin.create(folder).await.unwrap();
    }
    h
}

impl Harness {
    /// Subjects in a server folder, sorted, with whether each is read and starred.
    async fn server(&mut self, folder: &str) -> Vec<(String, bool, bool)> {
        self.admin.select(folder).await.unwrap();
        let fetched: Vec<_> = self
            .admin
            .fetch("1:*", "(FLAGS BODY.PEEK[HEADER.FIELDS (SUBJECT)])")
            .await
            .unwrap()
            .try_collect()
            .await
            .unwrap_or_default();
        let mut rows: Vec<_> = fetched
            .iter()
            .map(|f| {
                let header = String::from_utf8_lossy(f.header().unwrap_or_default()).into_owned();
                let subject = header.trim().trim_start_matches("Subject:").trim().to_string();
                let has = |wanted: Flag| f.flags().any(|flag| flag == wanted);
                (subject, has(Flag::Seen), has(Flag::Flagged))
            })
            .collect();
        rows.sort();
        rows
    }

    async fn server_subjects(&mut self, folder: &str) -> Vec<String> {
        self.server(folder).await.into_iter().map(|(subject, ..)| subject).collect()
    }

    fn id(&self, label: &str, subject: &str) -> i64 {
        self.rows(label).iter().find(|m| m.subject == subject).unwrap_or_else(|| panic!("{subject} not in {label}")).id
    }

    fn account_id(&self) -> i64 {
        self.account.id
    }
}

#[tokio::test]
#[ignore = "needs a local Dovecot: scripts/test-imap.ps1"]
async fn read_state_and_stars_reach_the_server_and_come_back_from_it() {
    let mut h = mailbox("flags").await;
    h.deliver("INBOX", "a", "One").await;
    h.deliver("INBOX", "b", "Two").await;
    h.sync().await;
    let one = h.id("Inbox", "One");

    h.store.act(h.account_id(), &[one], Triage::Seen(true)).unwrap();
    h.store.act(h.account_id(), &[one], Triage::Starred(true)).unwrap();
    assert_eq!(h.server("INBOX").await[0], ("One".to_string(), false, false), "nothing goes until the queue runs");
    h.sync().await;
    assert_eq!(h.server("INBOX").await, [("One".to_string(), true, true), ("Two".to_string(), false, false)]);
    assert_eq!(h.store.pending_count(h.account_id()).unwrap(), 0);
    assert_eq!(h.label("Inbox").unread, 1);
    assert_eq!(h.subjects("Starred"), ["One"]);

    h.store.act(h.account_id(), &[one], Triage::Seen(false)).unwrap();
    h.store.act(h.account_id(), &[one], Triage::Starred(false)).unwrap();
    h.sync().await;
    assert_eq!(h.server("INBOX").await[0], ("One".to_string(), false, false));
    assert_eq!(h.label("Inbox").unread, 2);
}

#[tokio::test]
#[ignore = "needs a local Dovecot: scripts/test-imap.ps1"]
async fn a_message_moves_and_comes_back_as_itself_with_its_body() {
    let mut h = mailbox("moves").await;
    h.deliver("INBOX", "a", "Mover").await;
    h.sync().await;
    let id = h.id("Inbox", "Mover");
    h.store.save_body(id, b"Subject: Mover\r\n\r\nkept\r\n").unwrap();

    for (triage, label, folder) in [(Triage::Archive, "Archive", "Archive"), (Triage::Trash, "Trash", "Trash"), (Triage::Spam, "Spam", "Junk")] {
        let undo = h.store.act(h.account_id(), &[id], triage).unwrap();
        assert!(h.subjects("Inbox").is_empty(), "{label}: gone from the Inbox before the server hears");
        assert_eq!(h.subjects(label), ["Mover"]);
        h.sync().await;
        assert!(h.server_subjects("INBOX").await.is_empty(), "{label}");
        assert_eq!(h.server_subjects(folder).await, ["Mover"], "{label}");
        assert_eq!(h.subjects(label), ["Mover"]);
        assert_eq!(h.id(label, "Mover"), id, "{label}: the same message");

        h.store.undo(h.account_id(), &undo).unwrap();
        assert_eq!(h.subjects("Inbox"), ["Mover"]);
        h.sync().await;
        assert_eq!(h.server_subjects("INBOX").await, ["Mover"], "{label}");
        assert!(h.server_subjects(folder).await.is_empty(), "{label}");
        assert_eq!(h.id("Inbox", "Mover"), id, "{label}: back as itself");
        assert!(h.reader.body(id).unwrap().is_some(), "{label}: and it keeps its body");
    }
}

#[tokio::test]
#[ignore = "needs a local Dovecot: scripts/test-imap.ps1"]
async fn undoing_before_the_server_hears_leaves_it_where_it_began() {
    let mut h = mailbox("quick-undo").await;
    h.deliver("INBOX", "a", "Quick").await;
    h.sync().await;
    let id = h.id("Inbox", "Quick");
    let undo = h.store.act(h.account_id(), &[id], Triage::Trash).unwrap();
    h.store.undo(h.account_id(), &undo).unwrap();
    h.sync().await;
    assert_eq!(h.server_subjects("INBOX").await, ["Quick"]);
    assert!(h.server_subjects("Trash").await.is_empty());
    assert_eq!(h.subjects("Inbox"), ["Quick"]);
    assert_eq!(h.store.pending_count(h.account_id()).unwrap(), 0);
}

#[tokio::test]
#[ignore = "needs a local Dovecot: scripts/test-imap.ps1"]
async fn actions_taken_offline_survive_a_restart_and_change_made_elsewhere_still_lands() {
    let mut h = mailbox("offline").await;
    h.deliver("INBOX", "a", "Keep").await;
    h.deliver("INBOX", "b", "Bin").await;
    h.sync().await;
    let (keep, bin) = (h.id("Inbox", "Keep"), h.id("Inbox", "Bin"));

    // No connection here. A store reopened from disk is what a restart sees.
    h.store.act(h.account_id(), &[keep], Triage::Seen(true)).unwrap();
    h.store.act(h.account_id(), &[bin], Triage::Trash).unwrap();
    let reopened = Arc::new(Store::open(&h.dir).unwrap());
    assert_eq!(reopened.pending_count(h.account_id()).unwrap(), 2);

    // Meanwhile another client stars the other message.
    h.admin.select("INBOX").await.unwrap();
    let updates: Vec<_> = h.admin.store("1", "+FLAGS (\\Flagged)").await.unwrap().try_collect().await.unwrap();
    drop(updates);

    h.store = reopened;
    h.sync().await;
    assert_eq!(h.server("INBOX").await, [("Keep".to_string(), true, true)], "both sides' changes hold");
    assert_eq!(h.server_subjects("Trash").await, ["Bin"]);
    assert_eq!(h.subjects("Starred"), ["Keep"]);
}

#[tokio::test]
#[ignore = "needs a local Dovecot: scripts/test-imap.ps1"]
async fn a_refused_action_puts_the_message_back_and_says_so() {
    let mut h = mailbox("refused").await;
    h.deliver("INBOX", "a", "Stuck").await;
    h.sync().await;
    let id = h.id("Inbox", "Stuck");
    h.store.act(h.account_id(), &[id], Triage::Archive).unwrap();
    assert!(h.subjects("Inbox").is_empty());

    // The folder goes behind our back, so the server can only say no.
    h.admin.delete("Archive").await.unwrap();
    let mut refused = 0;
    let mut imap = h.connect().await;
    sync_account(&mut imap, &h.store, h.account_id(), |event| refused += usize::from(event == Event::ActionRefused)).await.unwrap();
    assert_eq!(refused, 1);
    assert_eq!(h.subjects("Inbox"), ["Stuck"], "it comes back rather than vanishing");
    assert_eq!(h.server_subjects("INBOX").await, ["Stuck"]);
    assert_eq!(h.store.pending_count(h.account_id()).unwrap(), 0, "one bad action cannot block the queue");
}

#[tokio::test]
#[ignore = "needs a local Dovecot: scripts/test-imap.ps1"]
async fn an_action_never_lands_on_another_message_after_a_renumbering() {
    let mut h = mailbox("renumbered").await;
    h.admin.create("Work").await.unwrap();
    h.deliver("Work", "old", "Old").await;
    h.sync().await;
    let id = h.id("Work", "Old");
    h.store.act(h.account_id(), &[id], Triage::Seen(true)).unwrap();

    // The folder is rebuilt, and a different message now has the UID we hold.
    h.admin.delete("Work").await.unwrap();
    h.admin.create("Work").await.unwrap();
    h.deliver("Work", "new", "Innocent").await;
    h.sync().await;
    assert_eq!(h.server("Work").await, [("Innocent".to_string(), false, false)]);
}

#[tokio::test]
#[ignore = "needs a local Dovecot: scripts/test-imap.ps1"]
async fn an_action_that_keeps_failing_does_not_hold_up_new_mail() {
    let mut h = mailbox("stubborn").await;
    h.deliver("INBOX", "a", "Slow").await;
    h.sync().await;
    let id = h.id("Inbox", "Slow");
    h.store.act(h.account_id(), &[id], Triage::Seen(true)).unwrap();
    let conn = rusqlite::Connection::open(h.dir.join("meta.db")).unwrap();
    conn.execute("UPDATE pending_op SET attempts = 3", []).unwrap();

    // It waits for the end of the pass, by which time the new mail is in, and still gets done.
    h.deliver("INBOX", "b", "New").await;
    h.sync().await;
    assert_eq!(h.subjects("Inbox"), ["New", "Slow"]);
    assert_eq!(h.server("INBOX").await[1], ("Slow".to_string(), true, false));
    assert_eq!(h.store.pending_count(h.account_id()).unwrap(), 0);
}

#[tokio::test]
#[ignore = "needs a local Dovecot: scripts/test-imap.ps1"]
async fn a_thread_comes_together_from_references() {
    let mut h = mailbox("threads").await;
    h.deliver("INBOX", "root", "Plan").await;
    let reply = "From: b@example.com\r\nTo: me@example.com\r\nSubject: Re: Plan\r\nMessage-ID: <reply@example.com>\r\n\
                 In-Reply-To: <root@example.com>\r\nReferences: <root@example.com>\r\n\
                 Date: Wed, 04 Nov 2026 11:00:00 +0700\r\n\r\nyes\r\n";
    h.admin.append("INBOX", None, None, reply.as_bytes()).await.unwrap();
    h.deliver("INBOX", "other", "Unrelated").await;
    h.sync().await;

    let rows = h.rows("Inbox");
    assert_eq!(rows.len(), 2, "{rows:?}");
    let thread = rows.iter().find(|m| m.count == 2).expect("two messages in one row");
    let subjects: Vec<_> = h.reader.thread(h.account_id(), thread.thread_id, false).unwrap().into_iter().map(|m| m.subject).collect();
    assert_eq!(subjects.len(), 2);
    assert!(subjects.contains(&"Plan".to_string()) && subjects.contains(&"Re: Plan".to_string()));
}

#[tokio::test]
#[ignore = "needs a local Dovecot: scripts/test-imap.ps1"]
async fn a_draft_is_one_copy_on_the_server_however_often_it_is_saved() {
    let mut h = mailbox("drafts").await;
    let mut compose = Compose { to: "a@example.com".into(), subject: "Version one".into(), body: "Draft text".into(), ..Compose::default() };
    let id = h.store.create_draft(h.account_id(), &compose, None, &[]).unwrap();
    h.store.upload_draft(id).unwrap();
    h.sync().await;
    assert_eq!(h.server("Drafts").await, [("Version one".to_string(), true, false)]);
    assert_eq!(h.rows("Drafts").len(), 1, "our row and the server's copy are one message");
    assert_eq!(h.rows("Drafts")[0].id, id);

    compose.subject = "Version two".into();
    h.store.update_draft(id, &compose).unwrap();
    h.store.upload_draft(id).unwrap();
    h.sync().await;
    assert_eq!(h.server_subjects("Drafts").await, ["Version two"]);
    assert_eq!(h.subjects("Drafts"), ["Version two"]);
    assert_eq!(h.rows("Drafts")[0].id, id);

    // A crash after the upload but before it was recorded repeats it, and that must not double up.
    h.store.upload_draft(id).unwrap();
    let raw = h.reader.body(id).unwrap().unwrap();
    h.admin.append("Drafts", Some("(\\Draft)"), None, &raw).await.unwrap();
    h.sync().await;
    assert_eq!(h.server_subjects("Drafts").await, ["Version two"]);

    h.store.discard_draft(id).unwrap();
    assert!(h.subjects("Drafts").is_empty());
    h.sync().await;
    assert!(h.server_subjects("Drafts").await.is_empty());
    assert!(h.subjects("Drafts").is_empty(), "and it does not come back");
    assert_eq!(h.store.pending_count(h.account_id()).unwrap(), 0);
}

#[tokio::test]
#[ignore = "needs a local Dovecot: scripts/test-imap.ps1"]
async fn a_draft_from_another_client_is_replaced_not_copied() {
    let mut h = mailbox("foreign").await;
    let raw = "From: me@example.com\r\nTo: a@example.com\r\nSubject: From the web\r\nMessage-ID: <web@example.com>\r\n\r\nStarted there\r\n";
    h.admin.append("Drafts", Some("(\\Draft \\Seen)"), None, raw.as_bytes()).await.unwrap();
    h.sync().await;
    let id = h.id("Drafts", "From the web");

    let mut fields = h.store.adopt_draft(id, raw.as_bytes()).unwrap().unwrap();
    fields.body = "Finished here".into();
    h.store.update_draft(id, &fields).unwrap();
    h.store.upload_draft(id).unwrap();
    h.sync().await;
    assert_eq!(h.server_subjects("Drafts").await, ["From the web"], "one copy");
    assert_eq!(h.rows("Drafts").len(), 1);
    let on_server = h.server_body("Drafts").await;
    assert!(on_server.contains("Finished here"), "{on_server}");
}

impl Harness {
    /// The first message of a folder as the server holds it.
    async fn server_body(&mut self, folder: &str) -> String {
        self.admin.select(folder).await.unwrap();
        let fetched: Vec<_> = self.admin.fetch("1", "BODY.PEEK[]").await.unwrap().try_collect().await.unwrap();
        String::from_utf8_lossy(fetched[0].body().unwrap()).into_owned()
    }
}

/// Messages mailpit took delivery of with this word in the subject: each one's summary.
fn delivered(word: &str) -> Vec<serde_json::Value> {
    let url = format!("{MAILPIT_API}/search?query=subject:{word}");
    let mut response = ureq::get(&url).call().expect("start mailpit: scripts/test-imap.ps1");
    let body: serde_json::Value = response.body_mut().read_json().unwrap();
    body["messages"].as_array().cloned().unwrap_or_default()
}

fn delivered_raw(id: &str) -> String {
    ureq::get(format!("{MAILPIT_API}/message/{id}/raw")).call().unwrap().body_mut().read_to_string().unwrap()
}

fn engine(h: &Harness) -> (Engine, tokio::sync::mpsc::UnboundedReceiver<Event>) {
    secrets::save(&h.account.email, PASSWORD).unwrap();
    let (tx, events) = tokio::sync::mpsc::unbounded_channel();
    let mut engine = Engine::start(h.dir.clone(), h.store.clone(), None, move |_, e| {
        let _ = tx.send(e);
    });
    engine.add(h.account.clone());
    (engine, events)
}

async fn expect_event(events: &mut tokio::sync::mpsc::UnboundedReceiver<Event>, what: &str, mut wanted: impl FnMut(&Event) -> bool) -> Event {
    tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            let event = events.recv().await.unwrap();
            if wanted(&event) {
                return event;
            }
        }
    })
    .await
    .unwrap_or_else(|_| panic!("waited for {what}"))
}

fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64
}

#[tokio::test]
#[ignore = "needs Dovecot and mailpit: scripts/test-imap.ps1"]
async fn a_reply_goes_out_once_and_files_itself_in_its_thread() {
    let mut h = mailbox("send").await;
    let word = unique("sendword").replace('-', "");
    h.deliver("INBOX", "orig", &format!("Hello {word}")).await;
    let (engine, mut events) = engine(&h);
    expect_event(&mut events, "first sync", |e| *e == Event::UpToDate).await;

    let subject = format!("Hello {word}");
    let original = h.id("Inbox", &subject);
    let raw = raw_message("orig", &subject, "the question");
    let mut reply = reply(&raw, &h.account.email, false).unwrap();
    reply.bcc = "hidden@example.com".into();
    reply.body = format!("the answer{}", reply.body);
    let file = Attachment { name: "ghi-chú.txt".into(), mime: "text/plain".into(), data: "nội dung".as_bytes().to_vec() };
    let id = h.store.create_draft(h.account_id(), &reply, Some(original), std::slice::from_ref(&file)).unwrap();
    h.store.queue_send(id, now()).unwrap();
    engine.flush(h.account_id());
    assert_eq!(expect_event(&mut events, "the send", |e| matches!(e, Event::Sent(_) | Event::SendFailed { .. })).await, Event::Sent(id));

    // Out through real SMTP, once, with everything a reply needs and no trace of the Bcc.
    let copies = delivered(&word);
    assert_eq!(copies.len(), 1, "{copies:?}");
    let bytes = delivered_raw(copies[0]["ID"].as_str().unwrap());
    let parsed = mail_parser::MessageParser::default().parse(bytes.as_bytes()).unwrap();
    let m = parsed.root();
    assert_eq!(m.in_reply_to().unwrap().first(), Some("orig@example.com"));
    assert_eq!(m.to().unwrap().mailboxes().next().unwrap().address(), Some("an@example.com"));
    assert!(m.body_text(0).unwrap().contains("the answer"));
    assert!(m.body_text(0).unwrap().contains("> the question"));
    // Mailpit writes its own Bcc line above Return-Path, to show the envelope. What we sent follows.
    let (_, sent) = bytes.split_once("Return-Path").unwrap();
    assert!(!sent.to_lowercase().contains("hidden@example.com"), "{bytes}");
    assert_eq!(compose::attachments(bytes.as_bytes()), [file]);
    let envelope = copies[0]["Bcc"].as_array().unwrap();
    assert!(envelope.iter().any(|b| b["Address"] == "hidden@example.com"), "{copies:?}");

    // It reads as sent mail at once, and one copy lands on the server, adopted by the same row.
    assert_eq!(h.rows("Sent").len(), 1);
    assert_eq!(h.rows("Sent")[0].id, id);
    assert!(h.reader.body(id).unwrap().is_some());
    let mut server_copies = Vec::new();
    for _ in 0..200 {
        server_copies = h.server("Sent").await;
        if !server_copies.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert_eq!(server_copies.len(), 1, "{server_copies:?}");
    assert!(server_copies[0].1, "sent mail is read");
    engine.sync_now(h.account_id());
    expect_event(&mut events, "the next pass", |e| *e == Event::UpToDate).await;
    assert_eq!(h.rows("Sent").len(), 1, "our copy and the server's are one message");
    assert_eq!(h.rows("Sent")[0].id, id);
    assert_eq!(h.reader.location(id).unwrap().unwrap().0, "Sent");

    // The conversation now holds both, and nothing is left waiting.
    let thread = h.reader.thread(h.account_id(), h.rows("Inbox")[0].thread_id, false).unwrap();
    assert_eq!(thread.len(), 2, "{thread:?}");
    assert_eq!(h.store.pending_count(h.account_id()).unwrap(), 0);
    assert!(h.reader.draft(id).unwrap().is_none());
    secrets::delete(&h.account.email).unwrap();
}

#[tokio::test]
#[ignore = "needs Dovecot and mailpit: scripts/test-imap.ps1"]
async fn undo_send_means_it_never_left() {
    let h = mailbox("undo-send").await;
    let word = unique("undoword").replace('-', "");
    let (engine, mut events) = engine(&h);
    expect_event(&mut events, "first sync", |e| *e == Event::UpToDate).await;
    let compose = Compose { to: "a@example.com".into(), subject: word.clone(), body: "b".into(), ..Compose::default() };
    let id = h.store.create_draft(h.account_id(), &compose, None, &[]).unwrap();
    h.store.queue_send(id, now() + 2).unwrap();
    engine.flush(h.account_id());
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(h.store.cancel_send(id).unwrap());
    tokio::time::sleep(Duration::from_millis(2500)).await;
    assert!(delivered(&word).is_empty());
    assert_eq!(h.rows("Drafts").len(), 1);
    secrets::delete(&h.account.email).unwrap();
}

#[tokio::test]
#[ignore = "needs Dovecot and mailpit: scripts/test-imap.ps1"]
async fn mail_that_cannot_leave_waits_and_goes_when_it_can() {
    let h = mailbox("offline-send").await;
    let word = unique("waitword").replace('-', "");
    // Nothing listens here, as if the network were down.
    let mut down = h.account.clone();
    down.smtp_port = 9;
    h.store.add_account(&down).unwrap();
    secrets::save(&h.account.email, PASSWORD).unwrap();
    let (tx, mut events) = tokio::sync::mpsc::unbounded_channel();
    let mut engine = Engine::start(h.dir.clone(), h.store.clone(), None, move |_, e| {
        let _ = tx.send(e);
    });
    engine.add(down);
    expect_event(&mut events, "first sync", |e| *e == Event::UpToDate).await;

    let compose = Compose { to: "a@example.com".into(), subject: word.clone(), body: "b".into(), ..Compose::default() };
    let id = h.store.create_draft(h.account_id(), &compose, None, &[]).unwrap();
    h.store.queue_send(id, now()).unwrap();
    engine.flush(h.account_id());
    // A refused connection takes a couple of seconds to give up on Windows.
    for _ in 0..200 {
        if h.reader.labels(h.account_id()).unwrap().iter().any(|l| l.name == "Outbox") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(h.rows("Outbox").len(), 1, "it stays where its author can see it");

    // Back online, which for a stored send time means the next attempt finds a server.
    engine.remove(h.account_id());
    h.store.add_account(&h.account).unwrap();
    h.store.retry_now(id).unwrap();
    engine.add(h.account.clone());
    expect_event(&mut events, "the send", |e| *e == Event::Sent(id)).await;
    assert_eq!(delivered(&word).len(), 1);
    secrets::delete(&h.account.email).unwrap();
}

#[tokio::test]
#[ignore = "needs Dovecot and mailpit: scripts/test-imap.ps1"]
async fn mail_the_server_refuses_returns_to_drafts_and_says_why() {
    let h = mailbox("rejected").await;
    let (engine, mut events) = engine(&h);
    expect_event(&mut events, "first sync", |e| *e == Event::UpToDate).await;
    // The test server takes mail for example.com only.
    let compose = Compose { to: "a@refused.example".into(), subject: "No".into(), body: "b".into(), ..Compose::default() };
    let id = h.store.create_draft(h.account_id(), &compose, None, &[]).unwrap();
    h.store.queue_send(id, now()).unwrap();
    engine.flush(h.account_id());
    let event = expect_event(&mut events, "the refusal", |e| matches!(e, Event::SendFailed { .. })).await;
    assert!(matches!(event, Event::SendFailed { message, maybe: false, .. } if message == id), "{event:?}");
    assert_eq!(h.rows("Drafts")[0].id, id);
    assert_eq!(h.reader.draft(id).unwrap().unwrap().subject, "No", "nothing the author wrote is lost");
    secrets::delete(&h.account.email).unwrap();
}

#[tokio::test]
#[ignore = "needs Dovecot and mailpit: scripts/test-imap.ps1"]
async fn mail_caught_mid_send_by_a_crash_is_never_sent_again_by_itself() {
    let h = mailbox("crash").await;
    let word = unique("crashword").replace('-', "");
    let compose = Compose { to: "a@example.com".into(), subject: word.clone(), body: "b".into(), ..Compose::default() };
    let id = h.store.create_draft(h.account_id(), &compose, None, &[]).unwrap();
    h.store.queue_send(id, 0).unwrap();
    h.store.simulate_crash_during_send(id).unwrap();

    let (_engine, mut events) = engine(&h);
    let event = expect_event(&mut events, "the warning", |e| matches!(e, Event::SendFailed { .. })).await;
    assert!(matches!(event, Event::SendFailed { maybe: true, .. }), "{event:?}");
    assert_eq!(h.rows("Drafts")[0].id, id);
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(delivered(&word).is_empty(), "the author decides, not a retry");
    secrets::delete(&h.account.email).unwrap();
}

#[tokio::test]
#[ignore = "needs Dovecot and mailpit: scripts/test-imap.ps1"]
async fn mail_that_went_after_all_stops_being_a_draft_when_its_copy_appears() {
    let mut h = mailbox("went-anyway").await;
    let compose = Compose { to: "a@example.com".into(), subject: "Went".into(), body: "b".into(), ..Compose::default() };
    let id = h.store.create_draft(h.account_id(), &compose, None, &[]).unwrap();
    h.store.queue_send(id, 0).unwrap();
    h.store.simulate_crash_during_send(id).unwrap();

    let (engine, mut events) = engine(&h);
    expect_event(&mut events, "the warning", |e| matches!(e, Event::SendFailed { maybe: true, .. })).await;
    expect_event(&mut events, "first sync", |e| *e == Event::UpToDate).await;
    assert_eq!(h.rows("Drafts")[0].id, id);

    // It had gone: the server has it, with the Message-ID we gave it.
    let conn = rusqlite::Connection::open(h.dir.join("meta.db")).unwrap();
    let mid: String = conn.query_row("SELECT mid FROM message WHERE id = ?1", [id], |r| r.get(0)).unwrap();
    let raw = format!("From: me@example.com\r\nTo: a@example.com\r\nSubject: Went\r\nMessage-ID: <{mid}>\r\n\r\nb\r\n");
    h.admin.append("Sent", Some("(\\Seen)"), None, raw.as_bytes()).await.unwrap();
    engine.sync_now(h.account_id());
    expect_event(&mut events, "the next pass", |e| *e == Event::UpToDate).await;

    assert_eq!(h.rows("Sent")[0].id, id);
    assert!(h.rows("Drafts").is_empty());
    assert_eq!(h.reader.draft(id).unwrap(), None, "there is nothing left to edit");
    secrets::delete(&h.account.email).unwrap();
}

#[tokio::test]
#[ignore = "needs a local Dovecot: scripts/test-imap.ps1"]
async fn the_server_finds_text_in_mail_whose_body_is_not_here() {
    let mut h = mailbox("search").await;
    let raw = "From: a@example.com\r\nSubject: Plain subject\r\nMessage-ID: <s1@example.com>\r\n\
               Content-Type: text/plain; charset=utf-8\r\n\r\nBáo cáo doanh thu quý ba\r\n";
    h.admin.append("INBOX", None, None, raw.as_bytes()).await.unwrap();
    h.deliver("INBOX", "s2", "Something else").await;
    let (engine, mut events) = engine(&h);
    expect_event(&mut events, "first sync", |e| *e == Event::UpToDate).await;
    let id = h.id("Inbox", "Plain subject");

    for (key, query) in [(1, "doanh"), (2, "doanh thu")] {
        assert!(engine.search(h.account_id(), key, query.to_string()));
        let event = expect_event(&mut events, "results", |e| matches!(e, Event::SearchDone { .. })).await;
        assert_eq!(event, Event::SearchDone { key, ids: vec![id] }, "{query}");
    }
    // Text outside ASCII goes through as UTF-8.
    assert!(engine.search(h.account_id(), 3, "Báo cáo".to_string()));
    let event = expect_event(&mut events, "results", |e| matches!(e, Event::SearchDone { .. })).await;
    assert_eq!(event, Event::SearchDone { key: 3, ids: vec![id] });
    secrets::delete(&h.account.email).unwrap();
}

#[tokio::test]
#[ignore = "needs a local Dovecot: scripts/test-imap.ps1"]
async fn a_large_message_arrives_whole_in_pieces() {
    let mut h = mailbox("large").await;
    let body = "0123456789abcdef".repeat(600_000);
    let raw = format!("From: a@example.com\r\nSubject: Large\r\nMessage-ID: <big@example.com>\r\n\r\n{body}\r\n");
    h.admin.append("INBOX", None, None, raw.as_bytes()).await.unwrap();
    h.sync().await;
    let mut imap = h.connect().await;
    imap.examine("INBOX").await.unwrap();
    let id = h.id("Inbox", "Large");
    let (_, uid) = h.reader.location(id).unwrap().unwrap();
    let fetched = imap.fetch_body(uid).await.unwrap().unwrap();
    assert_eq!(fetched.len(), raw.len(), "more than two chunks of it");
    assert_eq!(fetched, raw.as_bytes());
}
