//! Gmail's extensions over a real socket. Dovecot has no X-GM-*, so a scripted server plays
//! Gmail here, using the response shapes from Google's IMAP extension docs. It proves we parse
//! them and map them onto the store; it cannot prove real Gmail behaves the same way.

use std::sync::{Arc, Mutex};

use mailfunction_core::compose::Compose;
use mailfunction_core::imap::{Credential, Imap};
use mailfunction_core::ops::Triage;
use mailfunction_core::store::{Account, AuthKind, Reader, Role, Security, Store};
use mailfunction_core::sync::sync_account;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};

const THREAD_ID: u64 = 1_278_455_344_230_334_865;

#[derive(Default)]
struct Script {
    commands: Vec<String>,
    /// Second phase: message 1 has lost a label and been starred.
    changed: bool,
    /// The folder last opened.
    open: String,
    /// Messages added with APPEND.
    appended: Vec<String>,
    /// Message 2 has been moved to Trash, where it is UID 50.
    trashed: bool,
}

fn fetch_line(seq: u32, uid: u32, labels: &str, flags: &str, subject: &str) -> String {
    let header = format!("From: An <an@example.com>\r\nSubject: {subject}\r\nMessage-ID: <{uid}@x>\r\n\r\n");
    format!(
        "* {seq} FETCH (X-GM-THRID {} X-GM-MSGID {} X-GM-LABELS ({labels}) UID {uid} MODSEQ (100) \
         FLAGS ({flags}) INTERNALDATE \"04-Nov-2026 10:00:0{uid} +0700\" RFC822.SIZE 500 \
         BODY[HEADER.FIELDS (FROM SUBJECT MESSAGE-ID)] {{{}}}\r\n{header})\r\n",
        THREAD_ID + u64::from(uid) - 1,
        9000 + uid,
        header.len(),
    )
}

fn respond(tag: &str, command: &str, script: &mut Script) -> String {
    let upper = command.to_ascii_uppercase();
    let ok = format!("{tag} OK done\r\n");
    if upper.starts_with("CAPABILITY") {
        return format!("* CAPABILITY IMAP4rev1 CONDSTORE IDLE X-GM-EXT-1\r\n{ok}");
    }
    if upper.starts_with("LIST") {
        return format!(
            "* LIST (\\HasNoChildren) \"/\" \"INBOX\"\r\n\
             * LIST (\\HasNoChildren) \"/\" \"Receipts\"\r\n\
             * LIST (\\Noselect \\HasChildren) \"/\" \"[Gmail]\"\r\n\
             * LIST (\\All \\HasNoChildren) \"/\" \"[Gmail]/All Mail\"\r\n\
             * LIST (\\Drafts \\HasNoChildren) \"/\" \"[Gmail]/Drafts\"\r\n\
             * LIST (\\HasNoChildren \\Junk) \"/\" \"[Gmail]/Spam\"\r\n\
             * LIST (\\HasNoChildren \\Trash) \"/\" \"[Gmail]/Trash\"\r\n{ok}"
        );
    }
    if upper.starts_with("EXAMINE") {
        let all_mail = command.contains("All Mail");
        script.open = command.to_string();
        let modseq = if script.changed && all_mail { 200 } else { 100 };
        return format!(
            "* {} EXISTS\r\n* OK [UIDVALIDITY 7] x\r\n* OK [UIDNEXT 3] x\r\n* OK [HIGHESTMODSEQ {modseq}] x\r\n\
             {tag} OK [READ-ONLY] done\r\n",
            if all_mail { 2 } else { u32::from(script.trashed && command.contains("Trash")) }
        );
    }
    if upper.contains("CHANGEDSINCE") {
        return format!(
            "* 1 FETCH (X-GM-LABELS (\"\\\\Inbox\") UID 1 MODSEQ (200) FLAGS (\\Seen \\Flagged))\r\n{ok}"
        );
    }
    if upper.starts_with("SELECT") {
        script.open = command.to_string();
        return format!("* OK [UIDVALIDITY 7] x\r\n{ok}");
    }
    if upper.starts_with("UID MOVE") {
        script.trashed |= upper.starts_with("UID MOVE 2 ");
        return format!("* OK [COPYUID 7 1 50] moved\r\n{tag} OK done\r\n");
    }
    if upper.starts_with("UID SEARCH") {
        let found = if script.open.contains("Drafts") {
            "5"
        } else if script.open.contains("Trash") && script.trashed {
            "50"
        } else {
            ""
        };
        return format!("* SEARCH {found}\r\n{ok}");
    }
    if upper.starts_with("UID FETCH") && !script.open.contains("All Mail") {
        return ok;
    }
    if upper.starts_with("UID FETCH") {
        // Gmail quotes system labels and escapes the backslash, unlike its documentation's examples.
        let inbox = fetch_line(1, 1, "\"\\\\Inbox\" \"\\\\Important\" \"H&APM-a &AREBoQ-n\" Receipts", "\\Seen", "First");
        let sent = fetch_line(2, 2, "\"\\\\Sent\"", "", "Second");
        return format!("{inbox}{sent}{ok}");
    }
    ok
}

async fn serve(stream: TcpStream, script: Arc<Mutex<Script>>) {
    let (read, mut write) = stream.into_split();
    let mut reader = BufReader::new(read);
    write.write_all(b"* OK Gimap ready\r\n").await.unwrap();
    let mut line = String::new();
    loop {
        line.clear();
        if !matches!(reader.read_line(&mut line).await, Ok(n) if n > 0) {
            return;
        }
        let (tag, command) = line.trim_end().split_once(' ').unwrap();
        // A literal: the message itself follows once the server says to go on.
        if let Some(size) = command.strip_suffix('}').and_then(|c| c.rsplit_once('{')).and_then(|(_, n)| n.parse::<usize>().ok()) {
            write.write_all(b"+ go ahead\r\n").await.unwrap();
            let mut message = vec![0; size + 2];
            reader.read_exact(&mut message).await.unwrap();
            {
                let mut script = script.lock().unwrap();
                script.commands.push(command.to_string());
                script.appended.push(String::from_utf8_lossy(&message).into_owned());
            }
            write.write_all(format!("{tag} OK [APPENDUID 7 60] done\r\n").as_bytes()).await.unwrap();
            continue;
        }
        let reply = {
            let mut script = script.lock().unwrap();
            script.commands.push(command.to_string());
            respond(tag, command, &mut script)
        };
        write.write_all(reply.as_bytes()).await.unwrap();
    }
}

async fn start_server() -> (u16, Arc<Mutex<Script>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let script = Arc::new(Mutex::new(Script::default()));
    let server_script = script.clone();
    tokio::spawn(async move {
        loop {
            let (stream, _) = listener.accept().await.unwrap();
            tokio::spawn(serve(stream, server_script.clone()));
        }
    });
    (port, script)
}

#[tokio::test]
async fn gmail_labels_ids_and_folders_map_onto_the_store() {
    let (port, script) = start_server().await;
    let dir = std::env::temp_dir().join(format!("mailfunction-gmail-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let store = Store::open(&dir).unwrap();
    let mut account = Account {
        id: 0,
        email: "me@gmail.com".into(),
        host: "127.0.0.1".into(),
        port,
        security: Security::Plain,
        auth: AuthKind::Password,
        smtp_host: "localhost".into(),
        smtp_port: 25,
    };
    account.id = store.add_account(&account).unwrap();
    let reader = Reader::open(&dir).unwrap();

    let credential = Credential::Password("x".into());
    let mut imap = Imap::connect(&account, &credential).await.unwrap();
    assert!(imap.gmail);
    sync_account(&mut imap, &store, account.id, |_| {}).await.unwrap();

    let label = |name: &str| reader.labels(account.id).unwrap().into_iter().find(|l| l.name == name);
    let subjects = |name: &str| -> Vec<String> {
        let id = label(name).unwrap().id;
        reader.messages(id, 0, 10).unwrap().into_iter().map(|m| m.subject).collect()
    };
    assert_eq!(subjects("Inbox"), ["First"]);
    assert_eq!(subjects("Sent"), ["Second"]);
    assert_eq!(subjects("All Mail"), ["Second", "First"]);
    assert_eq!(subjects("Hóa đơn"), ["First"]);
    assert_eq!(subjects("Receipts"), ["First"]);
    assert!(label("Important").is_none() && label("\\Important").is_none());
    assert_eq!(label("Inbox").unwrap().role, Role::Inbox);

    let check = rusqlite::Connection::open(dir.join("meta.db")).unwrap();
    let (remote_id, thread): (String, i64) = check
        .query_row("SELECT remote_id, thread_id FROM message WHERE subject = 'First'", [], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .unwrap();
    assert_eq!(remote_id, "gm:9001");
    assert_eq!(thread as u64, THREAD_ID);

    // A later pass sees a label removed and a star added through CONDSTORE.
    script.lock().unwrap().changed = true;
    sync_account(&mut imap, &store, account.id, |_| {}).await.unwrap();
    assert!(subjects("Receipts").is_empty());
    assert_eq!(subjects("Inbox"), ["First"]);
    assert_eq!(subjects("All Mail"), ["Second", "First"], "the folder label survives a label change");
    assert_eq!(subjects("Starred"), ["First"]);

    let commands = script.lock().unwrap().commands.clone();
    for command in &commands {
        let upper = command.to_ascii_uppercase();
        assert!(!upper.contains(" STORE ") && !upper.starts_with("SELECT"), "must stay read-only: {command}");
        assert!(!upper.starts_with("EXAMINE \"INBOX") && !upper.contains("RECEIPTS"), "label folder synced: {command}");
    }
    assert!(commands.iter().any(|c| c.contains("X-GM-THRID")));
    drop(reader);
    drop(store);
    let _ = std::fs::remove_dir_all(&dir);
}

/// What Gmail's write commands look like. Nothing here can prove Gmail accepts them, which only
/// a real account can do, but it pins exactly what goes on the wire.
#[tokio::test]
async fn actions_reach_gmail_as_flags_labels_and_moves() {
    let (port, script) = start_server().await;
    let dir = std::env::temp_dir().join(format!("mailfunction-gmail-write-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let store = Store::open(&dir).unwrap();
    let mut account = Account {
        id: 0,
        email: "me@gmail.com".into(),
        host: "127.0.0.1".into(),
        port,
        security: Security::Plain,
        auth: AuthKind::Password,
        smtp_host: "localhost".into(),
        smtp_port: 25,
    };
    account.id = store.add_account(&account).unwrap();
    let reader = Reader::open(&dir).unwrap();
    let mut imap = Imap::connect(&account, &Credential::Password("x".into())).await.unwrap();
    sync_account(&mut imap, &store, account.id, |_| {}).await.unwrap();
    let label = |name: &str| reader.labels(account.id).unwrap().into_iter().find(|l| l.name == name).unwrap();
    let first = reader.messages(label("Inbox").id, 0, 1).unwrap()[0].id;
    let second = reader.messages(label("Sent").id, 0, 1).unwrap()[0].id;

    // Read state goes out in bulk, as marking a whole label read would otherwise take minutes.
    let start = script.lock().unwrap().commands.len();
    store.act(account.id, &[first, second], Triage::Seen(false)).unwrap();
    sync_account(&mut imap, &store, account.id, |_| {}).await.unwrap();
    store.act(account.id, &[first, second], Triage::Seen(true)).unwrap();
    sync_account(&mut imap, &store, account.id, |_| {}).await.unwrap();
    let commands = script.lock().unwrap().commands[start..].to_vec();
    let flags: Vec<_> = commands.iter().filter(|c| c.starts_with("UID STORE")).map(String::as_str).collect();
    assert_eq!(flags, ["UID STORE 1 -FLAGS.SILENT (\\Seen)", "UID STORE 1,2 +FLAGS.SILENT (\\Seen)"], "the second was unread already");

    // Everything queued while offline goes out in order at the next pass.
    let start = script.lock().unwrap().commands.len();
    store.act(account.id, &[first], Triage::Seen(false)).unwrap();
    store.act(account.id, &[first], Triage::Starred(true)).unwrap();
    store.act(account.id, &[first], Triage::Archive).unwrap();
    store.act(account.id, &[first], Triage::Label { label: label("Hóa đơn").id, on: false }).unwrap();
    store.act(account.id, &[second], Triage::Trash).unwrap();
    sync_account(&mut imap, &store, account.id, |_| {}).await.unwrap();
    assert_eq!(store.pending_count(account.id).unwrap(), 0);
    let commands = script.lock().unwrap().commands[start..].to_vec();
    let writes: Vec<_> = commands.iter().filter(|c| c.starts_with("UID STORE") || c.starts_with("UID MOVE")).map(String::as_str).collect();
    assert_eq!(
        writes,
        [
            "UID STORE 1 -FLAGS.SILENT (\\Seen)",
            "UID STORE 1 +FLAGS.SILENT (\\Flagged)",
            "UID STORE 1 -X-GM-LABELS (\\Inbox)",
            "UID STORE 1 -X-GM-LABELS (\"H&APM-a &AREBoQ-n\")",
            "UID MOVE 2 \"[Gmail]/Trash\"",
        ]
    );
    assert_eq!(commands.iter().filter(|c| c.starts_with("SELECT")).count(), 1, "one folder, opened once: {commands:?}");

    // The move left the message in Trash, at the UID the server gave it.
    assert_eq!(reader.location(second).unwrap().unwrap(), ("[Gmail]/Trash".to_string(), 50));

    // A draft goes up to Drafts. A replaced copy leaves through Trash, as anything expunged from
    // Drafts alone would stay in All Mail.
    let draft = store.create_draft(account.id, &Compose { subject: "Hello".into(), ..Compose::default() }, None, &[]).unwrap();
    store.upload_draft(draft).unwrap();
    let start = script.lock().unwrap().commands.len();
    sync_account(&mut imap, &store, account.id, |_| {}).await.unwrap();
    {
        let script = script.lock().unwrap();
        let first_upload = script.commands[start..].iter().find(|c| c.starts_with("APPEND")).unwrap();
        assert!(first_upload.starts_with("APPEND \"[Gmail]/Drafts\" (\\Draft \\Seen) {"), "{first_upload}");
        assert!(script.appended.last().unwrap().contains("Subject: Hello"));
    }
    store.upload_draft(draft).unwrap();
    let start = script.lock().unwrap().commands.len();
    sync_account(&mut imap, &store, account.id, |_| {}).await.unwrap();
    let commands = script.lock().unwrap().commands[start..].to_vec();
    let at = |prefix: &str| commands.iter().position(|c| c.starts_with(prefix)).unwrap_or_else(|| panic!("no {prefix}: {commands:?}"));
    assert!(at("APPEND") < at("UID MOVE 5 \"[Gmail]/Trash\""), "the new copy lands before the old one goes");
    assert!(at("UID MOVE 5") < at("SELECT \"[Gmail]/Trash\""));
    assert!(at("SELECT \"[Gmail]/Trash\"") < at("UID STORE 50 +FLAGS.SILENT (\\Deleted)"));
    assert!(at("UID STORE 50") < at("UID EXPUNGE 50"));
    assert_eq!(store.pending_count(account.id).unwrap(), 0);

    drop(reader);
    drop(store);
    let _ = std::fs::remove_dir_all(&dir);
}
