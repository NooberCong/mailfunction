//! Gmail's extensions over a real socket. Dovecot has no X-GM-*, so a scripted server plays
//! Gmail here, using the response shapes from Google's IMAP extension docs. It proves we parse
//! them and map them onto the store; it cannot prove real Gmail behaves the same way.

use std::sync::{Arc, Mutex};

use mailfunction_core::imap::{Credential, Imap};
use mailfunction_core::store::{Account, AuthKind, Reader, Role, Security, Store};
use mailfunction_core::sync::sync_account;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};

const THREAD_ID: u64 = 1_278_455_344_230_334_865;

#[derive(Default)]
struct Script {
    commands: Vec<String>,
    /// Second phase: message 1 has lost a label and been starred.
    changed: bool,
}

fn fetch_line(seq: u32, uid: u32, labels: &str, flags: &str, subject: &str) -> String {
    let header = format!("From: An <an@example.com>\r\nSubject: {subject}\r\nMessage-ID: <{uid}@x>\r\n\r\n");
    format!(
        "* {seq} FETCH (X-GM-THRID {THREAD_ID} X-GM-MSGID {} X-GM-LABELS ({labels}) UID {uid} MODSEQ (100) \
         FLAGS ({flags}) INTERNALDATE \"04-Nov-2026 10:00:0{uid} +0700\" RFC822.SIZE 500 \
         BODY[HEADER.FIELDS (FROM SUBJECT MESSAGE-ID)] {{{}}}\r\n{header})\r\n",
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
             * LIST (\\HasNoChildren \\Junk) \"/\" \"[Gmail]/Spam\"\r\n\
             * LIST (\\HasNoChildren \\Trash) \"/\" \"[Gmail]/Trash\"\r\n{ok}"
        );
    }
    if upper.starts_with("EXAMINE") {
        let all_mail = command.contains("All Mail");
        let modseq = if script.changed && all_mail { 200 } else { 100 };
        return format!(
            "* {} EXISTS\r\n* OK [UIDVALIDITY 7] x\r\n* OK [UIDNEXT 3] x\r\n* OK [HIGHESTMODSEQ {modseq}] x\r\n\
             {tag} OK [READ-ONLY] done\r\n",
            if all_mail { 2 } else { 0 }
        );
    }
    if upper.contains("CHANGEDSINCE") {
        return format!(
            "* 1 FETCH (X-GM-LABELS (\\Inbox) UID 1 MODSEQ (200) FLAGS (\\Seen \\Flagged))\r\n{ok}"
        );
    }
    if upper.starts_with("UID FETCH") {
        let inbox = fetch_line(1, 1, "\\Inbox \\Important \"H&APM-a &AREBoQ-n\" Receipts", "\\Seen", "First");
        let sent = fetch_line(2, 2, "\\Sent", "", "Second");
        return format!("{inbox}{sent}{ok}");
    }
    ok
}

async fn serve(stream: TcpStream, script: Arc<Mutex<Script>>) {
    let (read, mut write) = stream.into_split();
    let mut lines = BufReader::new(read).lines();
    write.write_all(b"* OK Gimap ready\r\n").await.unwrap();
    while let Ok(Some(line)) = lines.next_line().await {
        let (tag, command) = line.split_once(' ').unwrap();
        let reply = {
            let mut script = script.lock().unwrap();
            script.commands.push(command.to_string());
            respond(tag, command, &mut script)
        };
        write.write_all(reply.as_bytes()).await.unwrap();
    }
}

#[tokio::test]
async fn gmail_labels_ids_and_folders_map_onto_the_store() {
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
    };
    account.id = store.add_account(&account).unwrap();
    let reader = Reader::open(&dir).unwrap();

    let credential = Credential::Password("x".into());
    let mut imap = Imap::connect(&account, &credential).await.unwrap();
    assert!(imap.gmail);
    sync_account(&mut imap, &store, account.id, || {}).await.unwrap();

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
        .query_row("SELECT remote_id, gm_thread_id FROM message WHERE subject = 'First'", [], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .unwrap();
    assert_eq!(remote_id, "gm:9001");
    assert_eq!(thread as u64, THREAD_ID);

    // A later pass sees a label removed and a star added through CONDSTORE.
    script.lock().unwrap().changed = true;
    sync_account(&mut imap, &store, account.id, || {}).await.unwrap();
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
