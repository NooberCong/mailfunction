//! The report's budgets on a 100k-message mailbox. Timings only mean something in release:
//! `cargo test --release -p mailfunction-core --test scale -- --ignored --nocapture`

use std::time::{Duration, Instant};

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

    assert_eq!(timed("count", 50, || reader.message_count(everything).unwrap()), MESSAGES as usize);
    let first = timed("first page", 50, || reader.messages(inbox, 0, 100).unwrap());
    assert_eq!(first.len(), 100);
    let last = timed("deepest page", 50, || reader.messages(everything, MESSAGES as usize - 100, 100).unwrap());
    assert_eq!(last.len(), 100);
    let position = timed("position of the oldest message", 50, || {
        reader.position(everything, last[99].id).unwrap().unwrap()
    });
    assert_eq!(position, MESSAGES as usize - 1);

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
