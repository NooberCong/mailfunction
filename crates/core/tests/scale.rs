//! The report's budgets on a 100k-message mailbox. Timings only mean something in release:
//! `cargo test --release -p mailfunction-core --test scale -- --ignored --nocapture`

use std::time::{Duration, Instant};

use mailfunction_core::ops::Triage;
use mailfunction_core::store::{Account, AuthKind, LabelSpec, NewMessage, Reader, Role, Security, Store};

const MESSAGES: u32 = 100_000;

fn timed<T>(name: &str, budget_ms: u64, f: impl FnOnce() -> T) -> T {
    let started = Instant::now();
    let out = f();
    let elapsed = started.elapsed();
    println!("{name}: {:.2} ms (budget {budget_ms} ms)", elapsed.as_secs_f64() * 1000.0);
    assert!(elapsed < Duration::from_millis(budget_ms), "{name} took {elapsed:?}");
    out
}

#[test]
#[ignore = "seeds 100k messages; run in release"]
fn a_large_mailbox_stays_within_budget() {
    let dir = std::env::temp_dir().join(format!("mailfunction-scale-{}", std::process::id()));
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
            smtp_port: 25,
        })
        .unwrap();
    let all = store.folder(account, "[Gmail]/All Mail", &LabelSpec::system(Role::All)).unwrap();

    let started = Instant::now();
    for chunk in (1..=MESSAGES).collect::<Vec<_>>().chunks(1000) {
        let batch: Vec<_> = chunk
            .iter()
            .map(|&uid| NewMessage {
                uid,
                remote_id: format!("gm:{uid}"),
                gm_thread_id: Some(uid as u64 / 3),
                mid: None,
                references: vec![],
                subject: format!("Subject line number {uid} about something"),
                from_name: format!("Sender {}", uid % 500),
                from_addr: format!("sender{}@example.com", uid % 500),
                date: 1_700_000_000 + uid as i64 * 60,
                size: 40_000,
                seen: uid % 50 != 0,
                flagged: uid % 200 == 0,
                labels: if uid % 2 == 0 {
                    vec![LabelSpec::system(Role::Inbox), LabelSpec::user(format!("Label {}", uid % 20))]
                } else {
                    vec![]
                },
            })
            .collect();
        store.save_messages(&all, &batch).unwrap();
    }
    println!("stored {MESSAGES} messages in {:.1} s", started.elapsed().as_secs_f64());
    let size = std::fs::metadata(dir.join("meta.db")).unwrap().len();
    println!("meta.db: {:.1} MB", size as f64 / 1_048_576.0);

    let reader = Reader::open(&dir).unwrap();
    let labels = timed("sidebar with unread counts", 100, || reader.labels(account).unwrap());
    timed("unread count of every account", 50, || reader.inbox_unread().unwrap());
    let inbox = labels.iter().find(|l| l.role == Role::Inbox).unwrap().id;
    let everything = labels.iter().find(|l| l.role == Role::All).unwrap().id;

    // Three messages to a thread, the first of which is one short.
    let threads = (MESSAGES as usize) / 3 + 1;
    assert_eq!(timed("count", 50, || reader.message_count(everything).unwrap()), threads);
    let first = timed("first page", 50, || reader.messages(inbox, 0, 100).unwrap());
    assert_eq!(first.len(), 100);
    let last = timed("deepest page", 50, || reader.messages(everything, threads - 100, 100).unwrap());
    assert_eq!(last.len(), 100);
    let position = timed("position of the oldest thread", 50, || {
        reader.position(everything, last[99].thread_id).unwrap().unwrap()
    });
    assert_eq!(position, threads - 1);
    let thread = timed("opening a thread", 50, || reader.thread(account, first[0].thread_id, false).unwrap());
    assert!(!thread.is_empty());

    // The budgets are the plan's: first results in 100 ms, and never past 250 ms.
    assert_eq!(timed("search, a rare word", 100, || reader.search(account, "number 54321", 200).unwrap()).len(), 1);
    assert_eq!(timed("search, a word in everything", 250, || reader.search(account, "something", 200).unwrap()).len(), 200);
    timed("search, a sender", 100, || reader.search(account, "sender42", 200).unwrap());
    assert!(timed("contacts, on a pause in typing", 100, || reader.contacts(account, "sender4", 5).unwrap()).len() == 5);

    // One keypress: the store, the queue and the sidebar, on the UI thread.
    let ids: Vec<i64> = thread.iter().map(|m| m.id).collect();
    let undo = timed("archiving a thread", 20, || store.act(account, &ids, Triage::Archive).unwrap());
    timed("taking it back", 20, || store.undo(account, &undo).unwrap());
    // The button: one click, but it writes a row to the queue for each message, on the UI thread.
    let unread = timed("finding what is unread", 100, || reader.unread_in_label(everything).unwrap());
    println!("{} unread", unread.len());
    let undo = timed("marking all of it read", 500, || store.act(account, &unread, Triage::Seen(true)).unwrap());
    timed("taking that back", 500, || store.undo(account, &undo).unwrap());
    let body = "Subject: x\r\nContent-Type: text/plain\r\n\r\nSome words to index.\r\n".repeat(50);
    timed("storing and indexing a body", 20, || store.save_body(ids[0], body.as_bytes()).unwrap());

    timed("resync of 1000 flag changes", 500, || {
        let changes: Vec<_> = (1..=1000)
            .map(|uid| mailfunction_core::store::Change { uid, seen: true, flagged: false, labels: None })
            .collect();
        store.apply_changes(&all, &changes).unwrap();
    });
    timed("expunge scan of local UIDs", 100, || store.uids(&all).unwrap());
    timed("orphan purge", 500, || store.purge_orphans(account).unwrap());
    timed("removing the whole account", 60_000, || store.remove_account(account).unwrap());

    drop(reader);
    drop(store);
    let _ = std::fs::remove_dir_all(&dir);
}
