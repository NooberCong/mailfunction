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
use mailfunction_core::engine::{Engine, Event};
use mailfunction_core::imap::{Credential, Imap};
use mailfunction_core::secrets;
use mailfunction_core::store::{Account, AuthKind, Folder, LabelRow, MessageRow, Reader, Security, Store};
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
    let user = unique(name);
    let dir = std::env::temp_dir().join(format!("mailfunction-{user}"));
    let store = Arc::new(Store::open(&dir).unwrap());
    let mut account = Account {
        id: 0,
        email: user.clone(),
        host: "127.0.0.1".into(),
        port: port(),
        security: Security::Plain,
        auth: AuthKind::Password,
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
    let second_user = unique("multi-second");
    let second = Account { email: second_user.clone(), ..h.account.clone() };
    let second = Account { id: h.store.add_account(&second).unwrap(), ..second };
    secrets::save(&second.email, PASSWORD).unwrap();
    let third = Account { email: unique("multi-third"), ..h.account.clone() };
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
