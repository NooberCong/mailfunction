//! The local store. `meta.db` holds what lists need; `bodies.db` holds compressed raw messages
//! so that the hot file stays small. Identity lives in `message`, location in `placement`:
//! server UIDs are never a key, so a UIDVALIDITY change cannot lose a message.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::{LazyLock, Mutex};

use mail_parser::MessageParser;
use rusqlite::{Connection, OptionalExtension, Row, Transaction, params, params_from_iter};

use crate::Result;
use crate::draft::finished_elsewhere;
use crate::ops::reapply_pending;

/// Applied in order; `PRAGMA user_version` records how many have run.
const MIGRATIONS: &[&str] = &["
    CREATE TABLE account (
        id INTEGER PRIMARY KEY,
        email TEXT NOT NULL UNIQUE,
        host TEXT NOT NULL,
        port INTEGER NOT NULL,
        security TEXT NOT NULL,
        auth TEXT NOT NULL
    );
    CREATE TABLE label (
        id INTEGER PRIMARY KEY,
        account_id INTEGER NOT NULL REFERENCES account ON DELETE CASCADE,
        name TEXT NOT NULL,
        role TEXT NOT NULL,
        UNIQUE (account_id, name)
    );
    CREATE TABLE folder (
        id INTEGER PRIMARY KEY,
        account_id INTEGER NOT NULL REFERENCES account ON DELETE CASCADE,
        path TEXT NOT NULL,
        label_id INTEGER NOT NULL REFERENCES label,
        uid_validity INTEGER NOT NULL DEFAULT 0,
        next_uid INTEGER NOT NULL DEFAULT 0,
        backfill_uid INTEGER NOT NULL DEFAULT 0,
        modseq INTEGER NOT NULL DEFAULT 0,
        UNIQUE (account_id, path)
    );
    CREATE TABLE message (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        account_id INTEGER NOT NULL REFERENCES account ON DELETE CASCADE,
        remote_id TEXT NOT NULL,
        gm_thread_id INTEGER,
        subject TEXT NOT NULL,
        from_name TEXT NOT NULL,
        from_addr TEXT NOT NULL,
        date INTEGER NOT NULL,
        size INTEGER NOT NULL,
        seen INTEGER NOT NULL,
        flagged INTEGER NOT NULL,
        UNIQUE (account_id, remote_id)
    );
    CREATE INDEX message_unseen ON message (id) WHERE seen = 0;
    CREATE TABLE placement (
        folder_id INTEGER NOT NULL REFERENCES folder ON DELETE CASCADE,
        message_id INTEGER NOT NULL REFERENCES message ON DELETE CASCADE,
        uid INTEGER,
        PRIMARY KEY (folder_id, message_id)
    ) WITHOUT ROWID;
    CREATE UNIQUE INDEX placement_uid ON placement (folder_id, uid);
    CREATE INDEX placement_message ON placement (message_id);
    -- `date` is copied here so that a label's list is a plain index walk, with no sort.
    CREATE TABLE message_label (
        label_id INTEGER NOT NULL REFERENCES label ON DELETE CASCADE,
        date INTEGER NOT NULL,
        message_id INTEGER NOT NULL REFERENCES message ON DELETE CASCADE,
        PRIMARY KEY (label_id, date, message_id)
    ) WITHOUT ROWID;
    CREATE INDEX message_label_message ON message_label (message_id);
    CREATE TABLE bodies.body (
        message_id INTEGER PRIMARY KEY,
        raw BLOB NOT NULL
    );
", "
    CREATE TABLE image_sender (
        account_id INTEGER NOT NULL REFERENCES account ON DELETE CASCADE,
        address TEXT NOT NULL,
        PRIMARY KEY (account_id, address)
    ) WITHOUT ROWID;
    -- `image` is NULL for a domain that was asked and has no logo.
    CREATE TABLE logo (
        domain TEXT PRIMARY KEY,
        image BLOB,
        checked INTEGER NOT NULL
    );
", "
    ALTER TABLE account ADD COLUMN smtp_host TEXT NOT NULL DEFAULT 'smtp.gmail.com';
    ALTER TABLE account ADD COLUMN smtp_port INTEGER NOT NULL DEFAULT 465;

    ALTER TABLE message RENAME COLUMN gm_thread_id TO thread_id;
    UPDATE message SET thread_id = id WHERE thread_id IS NULL;
    -- The Message-ID, without its angle brackets. Mail synced before this column stays NULL.
    ALTER TABLE message ADD COLUMN mid TEXT;
    CREATE INDEX message_thread ON message (account_id, thread_id);
    CREATE INDEX message_date ON message (account_id, date);
    CREATE INDEX message_mid ON message (account_id, mid) WHERE mid IS NOT NULL;
    -- Which thread a Message-ID belongs to, on servers that do not say. Gmail does.
    CREATE TABLE thread_ref (
        account_id INTEGER NOT NULL REFERENCES account ON DELETE CASCADE,
        mid TEXT NOT NULL,
        thread_id INTEGER NOT NULL,
        PRIMARY KEY (account_id, mid)
    ) WITHOUT ROWID;

    -- `head` marks a thread's newest message in a label, so a list of threads is an index walk.
    ALTER TABLE message_label ADD COLUMN head INTEGER NOT NULL DEFAULT 0;
    UPDATE message_label SET head = 1 WHERE (label_id, date, message_id) IN (
        SELECT label_id, date, message_id FROM (
            SELECT ml.label_id, ml.date, ml.message_id, row_number() OVER (
                PARTITION BY ml.label_id, m.thread_id ORDER BY ml.date DESC, ml.message_id DESC
            ) AS position
            FROM message_label ml JOIN message m ON m.id = ml.message_id
        ) WHERE position = 1
    );
    CREATE INDEX message_label_head ON message_label (label_id, date, message_id) WHERE head = 1;

    -- What the user did that the server has not heard yet. `message_id` is no foreign key,
    -- because discarding a draft deletes the message while its server copy is still to go.
    CREATE TABLE pending_op (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        account_id INTEGER NOT NULL REFERENCES account ON DELETE CASCADE,
        message_id INTEGER NOT NULL,
        action TEXT NOT NULL,
        undo TEXT,
        attempts INTEGER NOT NULL DEFAULT 0
    );
    CREATE INDEX pending_op_message ON pending_op (message_id);

    -- `old_mid` is the Message-ID of the copy on the server, which the next upload replaces.
    CREATE TABLE draft (
        message_id INTEGER PRIMARY KEY REFERENCES message ON DELETE CASCADE,
        recipients TEXT NOT NULL DEFAULT '',
        cc TEXT NOT NULL DEFAULT '',
        bcc TEXT NOT NULL DEFAULT '',
        body TEXT NOT NULL DEFAULT '',
        in_reply_to TEXT NOT NULL DEFAULT '',
        refs TEXT NOT NULL DEFAULT '',
        old_mid TEXT,
        state TEXT NOT NULL DEFAULT 'editing',
        send_at INTEGER NOT NULL DEFAULT 0
    );
    CREATE INDEX draft_send ON draft (state, send_at);
    CREATE TABLE bodies.attachment (
        message_id INTEGER NOT NULL,
        position INTEGER NOT NULL,
        name TEXT NOT NULL,
        mime TEXT NOT NULL,
        data BLOB NOT NULL,
        PRIMARY KEY (message_id, position)
    );

    -- Search ignores accents, but đ is a letter of its own that the tokenizer keeps, so it is
    -- folded by hand. People type Vietnamese without its accents, and `don` must find `đơn`.
    CREATE VIRTUAL TABLE fts USING fts5 (
        subject, sender, body, content = '', contentless_delete = 1,
        tokenize = 'unicode61 remove_diacritics 2'
    );
    INSERT INTO fts (rowid, subject, sender, body)
        SELECT id, replace(replace(subject, 'đ', 'd'), 'Đ', 'd'), replace(replace(from_name || ' ' || from_addr, 'đ', 'd'), 'Đ', 'd'), '' FROM message;
    CREATE TRIGGER message_indexed AFTER INSERT ON message BEGIN
        INSERT INTO fts (rowid, subject, sender, body)
        VALUES (new.id, replace(replace(new.subject, 'đ', 'd'), 'Đ', 'd'), replace(replace(new.from_name || ' ' || new.from_addr, 'đ', 'd'), 'Đ', 'd'), '');
    END;
    CREATE TRIGGER message_unindexed AFTER DELETE ON message BEGIN
        DELETE FROM fts WHERE rowid = old.id;
    END;
", "
    CREATE TABLE settings (
        id INTEGER PRIMARY KEY CHECK (id = 1),
        json TEXT NOT NULL
    );
    ALTER TABLE account ADD COLUMN name TEXT NOT NULL DEFAULT '';
    ALTER TABLE account ADD COLUMN signature TEXT NOT NULL DEFAULT '';
    ALTER TABLE account ADD COLUMN notify INTEGER NOT NULL DEFAULT 1;
", "
    -- Empty for a draft written before there was formatting: `body` is all of it.
    ALTER TABLE draft ADD COLUMN html TEXT NOT NULL DEFAULT '';
    -- Set for a picture in the text of a draft, which `cid:` in its HTML points to.
    ALTER TABLE bodies.attachment ADD COLUMN content_id TEXT;
", "
    CREATE TABLE window_placement (
        id INTEGER PRIMARY KEY CHECK (id = 1),
        json TEXT NOT NULL
    );
"];

/// The migration that added search. Bodies stored before it are indexed once, on opening.
const FTS_MIGRATION: i64 = 3;

/// The most of a message's text that goes into the search index.
const MAX_INDEXED_TEXT: usize = 15_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Security {
    Tls,
    /// Loopback only, for tests against a local server.
    Plain,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthKind {
    Google,
    Password,
}

#[derive(Debug, Clone)]
pub struct Account {
    pub id: i64,
    pub email: String,
    pub host: String,
    pub port: u16,
    pub security: Security,
    pub auth: AuthKind,
    pub smtp_host: String,
    pub smtp_port: u16,
}

/// What a label means to the app. Anything without a role is a user label.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Role {
    Inbox,
    Starred,
    Sent,
    Drafts,
    /// Mail waiting to go. Local only, and shown only while there is some.
    Outbox,
    All,
    /// Where archived mail goes on servers without Gmail's All Mail.
    Archive,
    Spam,
    Trash,
    None,
}

impl Role {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Role::Inbox => "inbox",
            Role::Starred => "starred",
            Role::Sent => "sent",
            Role::Drafts => "drafts",
            Role::Outbox => "outbox",
            Role::All => "all",
            Role::Archive => "archive",
            Role::Spam => "spam",
            Role::Trash => "trash",
            Role::None => "",
        }
    }

    pub(crate) fn parse(s: &str) -> Self {
        match s {
            "inbox" => Role::Inbox,
            "starred" => Role::Starred,
            "sent" => Role::Sent,
            "drafts" => Role::Drafts,
            "outbox" => Role::Outbox,
            "all" => Role::All,
            "archive" => Role::Archive,
            "spam" => Role::Spam,
            "trash" => Role::Trash,
            _ => Role::None,
        }
    }

    /// Where a role comes in the sidebar.
    fn rank(self) -> usize {
        const ORDER: [Role; 10] = [
            Role::Inbox,
            Role::Starred,
            Role::Sent,
            Role::Drafts,
            Role::Outbox,
            Role::All,
            Role::Archive,
            Role::Spam,
            Role::Trash,
            Role::None,
        ];
        ORDER.iter().position(|role| *role == self).unwrap_or(ORDER.len())
    }

    /// Display name, which is also the stored name for a system label.
    pub fn title(self) -> &'static str {
        match self {
            Role::Inbox => "Inbox",
            Role::Starred => "Starred",
            Role::Sent => "Sent",
            Role::Drafts => "Drafts",
            Role::Outbox => "Outbox",
            Role::All => "All Mail",
            Role::Archive => "Archive",
            Role::Spam => "Spam",
            Role::Trash => "Trash",
            Role::None => "",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct LabelSpec {
    pub name: String,
    pub role: Role,
}

impl LabelSpec {
    pub fn system(role: Role) -> Self {
        Self { name: role.title().into(), role }
    }

    pub fn user(name: impl Into<String>) -> Self {
        Self { name: name.into(), role: Role::None }
    }
}

#[derive(Debug, Clone)]
pub struct Folder {
    pub id: i64,
    pub account_id: i64,
    pub path: String,
    pub label_id: i64,
    pub uid_validity: u32,
    /// Everything at or above this UID has not been looked at yet.
    pub next_uid: u32,
    /// Backfill walks down from `next_uid`; everything below this UID is still to fetch.
    /// 1 means the folder is complete.
    pub backfill_uid: u32,
    pub modseq: u64,
}

impl Folder {
    pub fn backfilled(&self) -> bool {
        self.backfill_uid <= 1
    }
}

/// A message as a server reports it, ready to be stored.
#[derive(Debug, Clone)]
pub struct NewMessage {
    pub uid: u32,
    pub remote_id: String,
    pub gm_thread_id: Option<u64>,
    /// The Message-ID, without its angle brackets.
    pub mid: Option<String>,
    /// The Message-IDs it answers, which place it in a thread where the server has no threads.
    pub references: Vec<String>,
    pub subject: String,
    pub from_name: String,
    pub from_addr: String,
    pub date: i64,
    pub size: u32,
    pub seen: bool,
    pub flagged: bool,
    /// Labels beyond the folder's own. Gmail's X-GM-LABELS land here.
    pub labels: Vec<LabelSpec>,
}

/// A flag or label change to a message that is already stored.
#[derive(Debug, Clone)]
pub struct Change {
    pub uid: u32,
    pub seen: bool,
    pub flagged: bool,
    /// `None` when the server has no labels of its own, so only flags change.
    pub labels: Option<Vec<LabelSpec>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LabelRow {
    pub id: i64,
    pub name: String,
    pub role: Role,
    pub unread: i64,
}

/// A thread, which only means something in its own account: (account id, thread id). Gmail's
/// thread ids come from each mailbox, so two accounts can hold the same number.
pub type ThreadKey = (i64, i64);

/// A line of the sidebar: one label, or with several accounts on screen, the same system label
/// of each of them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SidebarLabel {
    /// Each label behind this line, as (account id, label id).
    pub held: Vec<(i64, i64)>,
    pub name: String,
    pub role: Role,
    pub unread: i64,
}

impl SidebarLabel {
    pub fn ids(&self) -> Vec<i64> {
        self.held.iter().map(|(_, label)| *label).collect()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessageRow {
    pub id: i64,
    pub thread_id: i64,
    pub from_name: String,
    pub from_addr: String,
    pub subject: String,
    pub date: i64,
    /// In a list these two speak for the thread: read only if all of it is, and starred if any
    /// of it is.
    pub seen: bool,
    pub flagged: bool,
    /// Messages in the thread, as [`Reader::thread`] would list them.
    pub count: i64,
    pub account_id: i64,
}

impl MessageRow {
    pub fn thread_key(&self) -> ThreadKey {
        (self.account_id, self.thread_id)
    }
}

fn configure(conn: &Connection, dir: &Path, cache_kib: i64) -> Result<()> {
    conn.busy_timeout(std::time::Duration::from_secs(5))?;
    conn.execute("ATTACH DATABASE ?1 AS bodies", [dir.join("bodies.db").to_string_lossy()])?;
    conn.execute_batch(&format!(
        "PRAGMA journal_mode = WAL;
         PRAGMA bodies.journal_mode = WAL;
         PRAGMA synchronous = NORMAL;
         PRAGMA bodies.synchronous = NORMAL;
         PRAGMA foreign_keys = ON;
         PRAGMA cache_size = -{cache_kib};"
    ))?;
    Ok(())
}

/// The one writer. Every write in the app goes through here.
pub struct Store {
    conn: Mutex<Connection>,
}

impl Store {
    pub fn open(dir: &Path) -> Result<Self> {
        std::fs::create_dir_all(dir)?;
        let mut conn = Connection::open(dir.join("meta.db"))?;
        configure(&conn, dir, 8192)?;
        let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        for (i, sql) in MIGRATIONS.iter().enumerate().skip(version as usize) {
            let tx = conn.transaction()?;
            tx.execute_batch(sql)?;
            tx.execute_batch(&format!("PRAGMA user_version = {}", i + 1))?;
            tx.commit()?;
        }
        if version > 0 && version < FTS_MIGRATION {
            index_stored_bodies(&conn)?;
        }
        Ok(Self { conn: Mutex::new(conn) })
    }

    pub(crate) fn with_tx<T>(&self, f: impl FnOnce(&Transaction) -> Result<T>) -> Result<T> {
        let mut conn = self.conn.lock().expect("store lock");
        let tx = conn.transaction()?;
        let out = f(&tx)?;
        tx.commit()?;
        Ok(out)
    }

    /// As [`Store::with_tx`], but the commit survives power loss. WAL's normal setting can lose
    /// the latest commits in a power cut, which is fine for anything sync can fetch again.
    pub(crate) fn with_durable_tx<T>(&self, f: impl FnOnce(&Transaction) -> Result<T>) -> Result<T> {
        let mut conn = self.conn.lock().expect("store lock");
        conn.execute_batch("PRAGMA synchronous = FULL")?;
        let tx = conn.transaction()?;
        let out = f(&tx).and_then(|out| {
            tx.commit()?;
            Ok(out)
        });
        conn.execute_batch("PRAGMA synchronous = NORMAL")?;
        out
    }

    pub fn add_account(&self, a: &Account) -> Result<i64> {
        self.with_tx(|tx| {
            tx.execute(
                "INSERT INTO account (email, host, port, security, auth, smtp_host, smtp_port)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                 ON CONFLICT (email) DO UPDATE
                 SET host = ?2, port = ?3, security = ?4, auth = ?5, smtp_host = ?6, smtp_port = ?7",
                params![
                    a.email,
                    a.host,
                    a.port,
                    if a.security == Security::Tls { "tls" } else { "plain" },
                    if a.auth == AuthKind::Google { "google" } else { "password" },
                    a.smtp_host,
                    a.smtp_port,
                ],
            )?;
            Ok(tx.query_row("SELECT id FROM account WHERE email = ?1", [&a.email], |r| r.get(0))?)
        })
    }

    /// Deletes an account and everything stored for it.
    pub fn remove_account(&self, account_id: i64) -> Result<()> {
        self.with_tx(|tx| {
            tx.execute(
                "DELETE FROM bodies.body WHERE message_id IN (SELECT id FROM message WHERE account_id = ?1)",
                [account_id],
            )?;
            tx.execute(
                "DELETE FROM bodies.attachment WHERE message_id IN (SELECT id FROM message WHERE account_id = ?1)",
                [account_id],
            )?;
            tx.execute(
                "DELETE FROM logo WHERE domain = 'account:' || (SELECT lower(email) FROM account WHERE id = ?1)",
                [account_id],
            )?;
            tx.execute("DELETE FROM account WHERE id = ?1", [account_id])?;
            // Logos are shared between accounts, but the last one out leaves no trace of who wrote.
            tx.execute("DELETE FROM logo WHERE NOT EXISTS (SELECT 1 FROM account)", [])?;
            Ok(())
        })
    }

    /// Loads remote images in this sender's mail from now on, or stops.
    pub fn set_images_allowed(&self, account_id: i64, address: &str, allowed: bool) -> Result<()> {
        let address = address.to_lowercase();
        self.with_tx(|tx| {
            let sql = if allowed {
                "INSERT OR IGNORE INTO image_sender (account_id, address) VALUES (?1, ?2)"
            } else {
                "DELETE FROM image_sender WHERE account_id = ?1 AND address = ?2"
            };
            tx.execute(sql, params![account_id, address])?;
            Ok(())
        })
    }

    /// Records what a domain's logo is, or that it has none, as of `checked` (Unix seconds).
    pub fn save_logo(&self, domain: &str, image: Option<&[u8]>, checked: i64) -> Result<()> {
        self.with_tx(|tx| {
            tx.execute(
                "INSERT INTO logo (domain, image, checked) VALUES (?1, ?2, ?3)
                 ON CONFLICT (domain) DO UPDATE SET image = ?2, checked = ?3",
                params![domain, image, checked],
            )?;
            Ok(())
        })
    }

    /// Makes sure a label exists, so the sidebar can show it before any mail has arrived.
    pub fn ensure_label(&self, account_id: i64, label: &LabelSpec) -> Result<()> {
        self.with_tx(|tx| label_id(tx, account_id, label).map(drop))
    }

    /// Creates the folder and its label on first sight and returns its sync state.
    pub fn folder(&self, account_id: i64, path: &str, label: &LabelSpec) -> Result<Folder> {
        self.with_tx(|tx| {
            let label_id = label_id(tx, account_id, label)?;
            tx.execute(
                "INSERT INTO folder (account_id, path, label_id) VALUES (?1, ?2, ?3)
                 ON CONFLICT (account_id, path) DO NOTHING",
                params![account_id, path, label_id],
            )?;
            Ok(tx.query_row(
                "SELECT id, account_id, path, label_id, uid_validity, next_uid, backfill_uid, modseq
                 FROM folder WHERE account_id = ?1 AND path = ?2",
                params![account_id, path],
                |r| {
                    Ok(Folder {
                        id: r.get(0)?,
                        account_id: r.get(1)?,
                        path: r.get(2)?,
                        label_id: r.get(3)?,
                        uid_validity: r.get(4)?,
                        next_uid: r.get(5)?,
                        backfill_uid: r.get(6)?,
                        modseq: r.get::<_, i64>(7)? as u64,
                    })
                },
            )?)
        })
    }

    /// Stores a batch of new or rediscovered messages, and returns the ids of those never seen
    /// before. Safe to repeat, so a crash before the folder's progress is saved only means
    /// fetching the same batch again.
    pub fn save_messages(&self, folder: &Folder, messages: &[NewMessage]) -> Result<Vec<i64>> {
        self.with_tx(|tx| {
            let account = folder.account_id;
            let role = folder_role(tx, folder)?;
            let mut labels = HashMap::new();
            let mut threads = HashSet::new();
            let mut added = Vec::new();
            for m in messages {
                // Our own sent mail or draft, back from the server: it stays one message.
                let mut adopted = false;
                if let Some(mid) = &m.mid {
                    let mut adopt = tx.prepare_cached(
                        "UPDATE message SET remote_id = ?3
                         WHERE account_id = ?1 AND mid = ?2 AND remote_id <> ?3
                           AND (remote_id LIKE 'local:%' OR id IN (SELECT message_id FROM draft))",
                    )?;
                    adopted = adopt.execute(params![account, mid, m.remote_id])? > 0;
                }
                // Before the write, because a new thread id leaves the old thread needing a head.
                let mut previous = tx.prepare_cached("SELECT thread_id FROM message WHERE account_id = ?1 AND remote_id = ?2")?;
                let known = previous.query_row(params![account, m.remote_id], |r| r.get::<_, Option<i64>>(0)).optional()?;
                threads.extend(known.flatten());
                tx.execute(
                    "INSERT INTO message
                       (account_id, remote_id, thread_id, subject, from_name, from_addr, date, size, seen, flagged, mid)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
                     ON CONFLICT (account_id, remote_id) DO UPDATE
                     SET seen = ?9, flagged = ?10, thread_id = coalesce(?3, thread_id), mid = coalesce(?11, mid)",
                    params![
                        account,
                        m.remote_id,
                        m.gm_thread_id.map(|t| t as i64),
                        m.subject,
                        m.from_name,
                        m.from_addr,
                        m.date,
                        m.size,
                        m.seen,
                        m.flagged,
                        m.mid,
                    ],
                )?;
                let (id, thread): (i64, Option<i64>) = tx.query_row(
                    "SELECT id, thread_id FROM message WHERE account_id = ?1 AND remote_id = ?2",
                    params![account, m.remote_id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )?;
                threads.extend(thread);
                // Adopted mail counts too: sent to oneself, it reached the Inbox like any other.
                if known.is_none() || adopted {
                    added.push(id);
                }
                if m.gm_thread_id.is_none() {
                    threads.insert(place_in_thread(tx, account, id, thread, m.mid.as_deref(), &m.references)?);
                }
                // A reused UID can only mean a stale row from before a rescan.
                tx.execute(
                    "UPDATE placement SET uid = NULL WHERE folder_id = ?1 AND uid = ?2 AND message_id <> ?3",
                    params![folder.id, m.uid, id],
                )?;
                tx.execute(
                    "INSERT INTO placement (folder_id, message_id, uid) VALUES (?1, ?2, ?3)
                     ON CONFLICT (folder_id, message_id) DO UPDATE SET uid = ?3",
                    params![folder.id, id, m.uid],
                )?;
                set_labels(tx, account, id, m.flagged, in_bin(role), Some(&m.labels), &mut labels)?;
                if role == Role::Sent || m.labels.iter().any(|l| l.role == Role::Sent) {
                    finished_elsewhere(tx, account, id)?;
                }
                reapply_pending(tx, id)?;
            }
            refresh_heads(tx, account, threads)?;
            Ok(added)
        })
    }

    /// Applies flag and label changes. UIDs we do not hold yet are skipped: backfill will
    /// fetch those messages with their current state.
    pub fn apply_changes(&self, folder: &Folder, changes: &[Change]) -> Result<()> {
        self.with_tx(|tx| {
            let binned = in_bin(folder_role(tx, folder)?);
            let mut labels = HashMap::new();
            let mut threads = HashSet::new();
            for c in changes {
                let id: Option<i64> = tx
                    .query_row(
                        "SELECT message_id FROM placement WHERE folder_id = ?1 AND uid = ?2",
                        params![folder.id, c.uid],
                        |r| r.get(0),
                    )
                    .optional()?;
                let Some(id) = id else { continue };
                tx.execute(
                    "UPDATE message SET seen = ?2, flagged = ?3 WHERE id = ?1",
                    params![id, c.seen, c.flagged],
                )?;
                set_labels(tx, folder.account_id, id, c.flagged, binned, c.labels.as_deref(), &mut labels)?;
                reapply_pending(tx, id)?;
                threads.insert(thread_of(tx, id)?);
            }
            refresh_heads(tx, folder.account_id, threads)
        })
    }

    /// Records sync progress. Call only once the data it describes is committed.
    pub fn save_progress(&self, folder: &Folder) -> Result<()> {
        self.with_tx(|tx| {
            tx.execute(
                "UPDATE folder SET uid_validity = ?2, next_uid = ?3, backfill_uid = ?4, modseq = ?5 WHERE id = ?1",
                params![folder.id, folder.uid_validity, folder.next_uid, folder.backfill_uid, folder.modseq as i64],
            )?;
            Ok(())
        })
    }

    /// The server renumbered this folder. Messages stay visible with no UID until a full
    /// rescan either finds them again or proves they are gone.
    pub fn forget_uids(&self, folder: &Folder) -> Result<()> {
        self.with_tx(|tx| {
            tx.execute("UPDATE placement SET uid = NULL WHERE folder_id = ?1", [folder.id])?;
            Ok(())
        })
    }

    pub fn uids(&self, folder: &Folder) -> Result<HashSet<u32>> {
        let conn = self.conn.lock().expect("store lock");
        let mut stmt = conn.prepare_cached("SELECT uid FROM placement WHERE folder_id = ?1 AND uid IS NOT NULL")?;
        let uids = stmt.query_map([folder.id], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
        Ok(uids)
    }

    pub fn uid_count(&self, folder: &Folder) -> Result<u32> {
        let conn = self.conn.lock().expect("store lock");
        Ok(conn.query_row(
            "SELECT count(*) FROM placement WHERE folder_id = ?1 AND uid IS NOT NULL",
            [folder.id],
            |r| r.get(0),
        )?)
    }

    /// Drops these UIDs from the folder. A message left with no location is hidden but kept
    /// until [`Store::purge_orphans`], because it may simply have moved to a later folder.
    pub fn remove_uids(&self, folder: &Folder, uids: &[u32]) -> Result<()> {
        self.with_tx(|tx| {
            let mut threads = HashSet::new();
            for uid in uids {
                let id: Option<i64> = tx
                    .query_row(
                        "SELECT message_id FROM placement WHERE folder_id = ?1 AND uid = ?2",
                        params![folder.id, uid],
                        |r| r.get(0),
                    )
                    .optional()?;
                if let Some(id) = id {
                    remove_placement(tx, folder, id)?;
                    threads.insert(thread_of(tx, id)?);
                }
            }
            refresh_heads(tx, folder.account_id, threads)
        })
    }

    /// After a complete scan, anything still without a UID is really gone from the server.
    pub fn remove_unmatched(&self, folder: &Folder) -> Result<()> {
        self.with_tx(|tx| {
            let mut stmt = tx.prepare("SELECT message_id FROM placement WHERE folder_id = ?1 AND uid IS NULL")?;
            let gone: Vec<i64> = stmt.query_map([folder.id], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
            let mut threads = HashSet::new();
            for id in gone {
                remove_placement(tx, folder, id)?;
                threads.insert(thread_of(tx, id)?);
            }
            refresh_heads(tx, folder.account_id, threads)
        })
    }

    /// Deletes messages that no folder claimed. Call only after a pass over every folder. A
    /// message that still has labels is on its way somewhere: a move the server has done and
    /// sync has yet to see, or mail that exists only here so far.
    pub fn purge_orphans(&self, account_id: i64) -> Result<()> {
        const ORPHANS: &str = "SELECT id FROM message m WHERE account_id = ?1
                               AND NOT EXISTS (SELECT 1 FROM placement p WHERE p.message_id = m.id)
                               AND NOT EXISTS (SELECT 1 FROM message_label l WHERE l.message_id = m.id)";
        self.with_tx(|tx| {
            tx.execute(&format!("DELETE FROM bodies.body WHERE message_id IN ({ORPHANS})"), [account_id])?;
            tx.execute(&format!("DELETE FROM bodies.attachment WHERE message_id IN ({ORPHANS})"), [account_id])?;
            tx.execute(&format!("DELETE FROM message WHERE id IN ({ORPHANS})"), [account_id])?;
            Ok(())
        })
    }

    /// Keeps a message's body and indexes its text. One that arrives after its account was
    /// removed is dropped, since `bodies.db` is a separate file and no foreign key can refuse it.
    pub fn save_body(&self, message_id: i64, raw: &[u8]) -> Result<()> {
        let packed = zstd::bulk::compress(raw, 3)?;
        let text = indexed_text(raw);
        self.with_tx(|tx| {
            tx.execute(
                "INSERT OR REPLACE INTO bodies.body (message_id, raw)
                 SELECT ?1, ?2 WHERE EXISTS (SELECT 1 FROM message WHERE id = ?1)",
                params![message_id, packed],
            )?;
            index_message(tx, message_id, &text)
        })
    }
}

/// A message's text as search sees it.
fn indexed_text(raw: &[u8]) -> String {
    let parsed = MessageParser::default().parse(raw);
    let text = parsed.as_ref().and_then(|m| m.root().body_text(0).map(|t| t.chars().take(MAX_INDEXED_TEXT).collect()));
    text.unwrap_or_default()
}

const INDEX_MESSAGE: &str = "INSERT OR REPLACE INTO fts (rowid, subject, sender, body)
                             SELECT id, replace(replace(subject, 'đ', 'd'), 'Đ', 'd'), replace(replace(from_name || ' ' || from_addr, 'đ', 'd'), 'Đ', 'd'), ?2
                             FROM message WHERE id = ?1";

/// As the index and its queries see text. See the note on the `fts` table.
fn folded(text: &str) -> String {
    text.replace(['đ', 'Đ'], "d")
}

/// Replaces a message's search entry. The index keeps no text of its own, so all of it goes in.
pub(crate) fn index_message(tx: &Transaction, message_id: i64, body: &str) -> Result<()> {
    tx.execute(INDEX_MESSAGE, params![message_id, folded(body)])?;
    Ok(())
}

fn index_stored_bodies(conn: &Connection) -> Result<()> {
    let mut stmt = conn.prepare("SELECT message_id, raw FROM bodies.body")?;
    let rows = stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, Vec<u8>>(1)?)))?;
    for row in rows {
        let (id, packed) = row?;
        conn.execute(INDEX_MESSAGE, params![id, folded(&indexed_text(&zstd::decode_all(packed.as_slice())?))])?;
    }
    Ok(())
}

pub(crate) fn label_id(tx: &Transaction, account_id: i64, spec: &LabelSpec) -> Result<i64> {
    tx.execute(
        "INSERT INTO label (account_id, name, role) VALUES (?1, ?2, ?3) ON CONFLICT (account_id, name) DO NOTHING",
        params![account_id, spec.name, spec.role.as_str()],
    )?;
    Ok(tx.query_row(
        "SELECT id FROM label WHERE account_id = ?1 AND name = ?2",
        params![account_id, spec.name],
        |r| r.get(0),
    )?)
}

/// Rebuilds a message's labels: its folders' labels, plus Starred from the flag, plus `extra`.
/// With `extra` as `None`, labels that came from the server are left alone. Mail in Trash or Spam
/// is out of circulation, so that is the only place it shows, whatever labels it still carries.
/// `binned` says which side of that the folder being read is on. It decides, rather than the
/// message's other locations, because one of those may be a stale copy that a later folder in
/// the same pass will find has gone.
fn set_labels(
    tx: &Transaction,
    account_id: i64,
    message_id: i64,
    flagged: bool,
    binned: bool,
    extra: Option<&[LabelSpec]>,
    cache: &mut HashMap<LabelSpec, i64>,
) -> Result<()> {
    let mut id_of = |spec: &LabelSpec| -> Result<i64> {
        if let Some(id) = cache.get(spec) {
            return Ok(*id);
        }
        let id = label_id(tx, account_id, spec)?;
        cache.insert(spec.clone(), id);
        Ok(id)
    };
    let starred = id_of(&LabelSpec::system(Role::Starred))?;
    if extra.is_some() {
        tx.execute("DELETE FROM message_label WHERE message_id = ?1", [message_id])?;
    } else {
        tx.execute("DELETE FROM message_label WHERE message_id = ?1 AND label_id = ?2", params![message_id, starred])?;
    }
    let mut folders = tx.prepare_cached(
        "SELECT f.label_id, l.role IN ('trash', 'spam') FROM placement p
         JOIN folder f ON f.id = p.folder_id JOIN label l ON l.id = f.label_id
         WHERE p.message_id = ?1",
    )?;
    let folders: Vec<(i64, bool)> =
        folders.query_map([message_id], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<_>>()?;
    if !binned {
        if flagged {
            add_label(tx, starred, message_id)?;
        }
        for spec in extra.unwrap_or_default() {
            add_label(tx, id_of(spec)?, message_id)?;
        }
    }
    for (label, _) in folders.iter().filter(|(_, in_bin)| *in_bin == binned) {
        add_label(tx, *label, message_id)?;
    }
    Ok(())
}

pub(crate) fn add_label(tx: &Transaction, label: i64, message_id: i64) -> Result<()> {
    let mut add = tx.prepare_cached(
        "INSERT OR IGNORE INTO message_label (label_id, date, message_id)
         SELECT ?1, date, id FROM message WHERE id = ?2",
    )?;
    add.execute(params![label, message_id])?;
    Ok(())
}

pub(crate) fn thread_of(tx: &Transaction, message_id: i64) -> Result<i64> {
    let mut stmt = tx.prepare_cached("SELECT thread_id FROM message WHERE id = ?1")?;
    Ok(stmt.query_row([message_id], |r| r.get(0))?)
}

/// Puts a message in the thread of whatever it mentions, joining threads it turns out to link,
/// and returns the thread. Only the nearest references count, because a hostile message can list
/// thousands.
pub(crate) fn place_in_thread(
    tx: &Transaction,
    account_id: i64,
    message_id: i64,
    current: Option<i64>,
    mid: Option<&str>,
    references: &[String],
) -> Result<i64> {
    const MAX_REFERENCES: usize = 50;
    let nearest = references.iter().rev().take(MAX_REFERENCES).map(String::as_str);
    let ids: Vec<&str> = mid.into_iter().chain(nearest).collect();
    let mut known: Vec<i64> = current.into_iter().collect();
    {
        let mut find = tx.prepare_cached("SELECT thread_id FROM thread_ref WHERE account_id = ?1 AND mid = ?2")?;
        for id in &ids {
            known.extend(find.query_row(params![account_id, id], |r| r.get::<_, i64>(0)).optional()?);
        }
    }
    let thread = known.iter().copied().min().unwrap_or(message_id);
    for other in known.iter().filter(|t| **t != thread) {
        tx.execute(
            "UPDATE message SET thread_id = ?3 WHERE account_id = ?1 AND thread_id = ?2",
            params![account_id, other, thread],
        )?;
        tx.execute(
            "UPDATE thread_ref SET thread_id = ?3 WHERE account_id = ?1 AND thread_id = ?2",
            params![account_id, other, thread],
        )?;
    }
    tx.execute("UPDATE message SET thread_id = ?2 WHERE id = ?1", params![message_id, thread])?;
    let mut record = tx.prepare_cached(
        "INSERT INTO thread_ref (account_id, mid, thread_id) VALUES (?1, ?2, ?3)
         ON CONFLICT (account_id, mid) DO UPDATE SET thread_id = ?3",
    )?;
    for id in ids {
        record.execute(params![account_id, id, thread])?;
    }
    Ok(thread)
}

/// Marks each thread's newest message in every label it is in. Call once a batch has settled.
/// A thread is a few rows, so they are compared here, and only what changed is written. This runs
/// for every batch of sync, so the plan is pinned rather than left to the query planner.
pub(crate) fn refresh_heads(tx: &Transaction, account_id: i64, threads: impl IntoIterator<Item = i64>) -> Result<()> {
    let mut rows = tx.prepare_cached(
        "SELECT ml.label_id, ml.date, ml.message_id, ml.head
         FROM message m INDEXED BY message_thread CROSS JOIN message_label ml ON ml.message_id = m.id
         WHERE m.account_id = ?1 AND m.thread_id = ?2",
    )?;
    let mut write = tx.prepare_cached("UPDATE message_label SET head = ?4 WHERE label_id = ?1 AND date = ?2 AND message_id = ?3")?;
    for thread in threads {
        let rows: Vec<(i64, i64, i64, bool)> = rows
            .query_map(params![account_id, thread], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
            .collect::<rusqlite::Result<_>>()?;
        let mut newest: HashMap<i64, (i64, i64)> = HashMap::new();
        for &(label, date, message, _) in &rows {
            let best = newest.entry(label).or_insert((date, message));
            *best = (*best).max((date, message));
        }
        for (label, date, message, head) in rows {
            let is_newest = newest[&label] == (date, message);
            if head != is_newest {
                write.execute(params![label, date, message, is_newest])?;
            }
        }
    }
    Ok(())
}

fn remove_placement(tx: &Transaction, folder: &Folder, message_id: i64) -> Result<()> {
    tx.execute("DELETE FROM placement WHERE folder_id = ?1 AND message_id = ?2", params![folder.id, message_id])?;
    tx.execute("DELETE FROM message_label WHERE label_id = ?1 AND message_id = ?2", params![folder.label_id, message_id])?;
    let remaining: i64 = tx.query_row("SELECT count(*) FROM placement WHERE message_id = ?1", [message_id], |r| r.get(0))?;
    // A draft is what the user is writing, whatever became of its copy on the server.
    let draft = tx.prepare_cached("SELECT 1 FROM draft WHERE message_id = ?1")?.exists([message_id])?;
    if remaining == 0 && !draft {
        tx.execute("DELETE FROM message_label WHERE message_id = ?1", [message_id])?;
    }
    Ok(())
}

fn folder_role(tx: &Transaction, folder: &Folder) -> Result<Role> {
    let mut stmt = tx.prepare_cached("SELECT role FROM label WHERE id = ?1")?;
    let role: String = stmt.query_row([folder.label_id], |r| r.get(0))?;
    Ok(Role::parse(&role))
}

fn in_bin(role: Role) -> bool {
    matches!(role, Role::Trash | Role::Spam)
}

/// Whether a message `t` is in Trash or Spam, as an SQL condition.
const IN_BIN: &str = "EXISTS (SELECT 1 FROM message_label x JOIN label l ON l.id = x.label_id
                              WHERE x.message_id = t.id AND l.role IN ('trash', 'spam'))";
/// Drafts and mail waiting to go are not part of a conversation yet.
const READABLE: &str = "NOT EXISTS (SELECT 1 FROM message_label y JOIN label k ON k.id = y.label_id
                                    WHERE y.message_id = t.id AND k.role IN ('drafts', 'outbox'))";

/// The columns [`thread_row`] reads, for a message `m` that stands for its thread. A thread's
/// count and state cover the messages on the same side of the bin as `m`.
static THREAD_COLUMNS: LazyLock<String> = LazyLock::new(|| {
    let peers = format!(
        "FROM message t WHERE t.account_id = m.account_id AND t.thread_id = m.thread_id AND {READABLE} AND {IN_BIN} = {}",
        IN_BIN.replace("t.id", "m.id")
    );
    format!(
        "m.id, m.thread_id, m.from_name, m.from_addr, m.subject, m.date,
         coalesce((SELECT min(t.seen) {peers}), m.seen),
         coalesce((SELECT max(t.flagged) {peers}), m.flagged),
         (SELECT count(*) {peers}),
         m.account_id"
    )
});

fn thread_row(r: &Row) -> rusqlite::Result<MessageRow> {
    Ok(MessageRow {
        id: r.get(0)?,
        thread_id: r.get(1)?,
        from_name: r.get(2)?,
        from_addr: r.get(3)?,
        subject: r.get(4)?,
        date: r.get(5)?,
        seen: r.get(6)?,
        flagged: r.get(7)?,
        count: r.get(8)?,
        account_id: r.get(9)?,
    })
}

/// `count` numbered SQL parameters, from `?first` on.
fn placeholders(first: usize, count: usize) -> String {
    (first..first + count).map(|i| format!("?{i}")).collect::<Vec<_>>().join(", ")
}

/// Someone mail can be addressed to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Contact {
    pub name: String,
    pub address: String,
}

/// Turns what someone typed into an FTS5 query: each word a quoted prefix, so nothing they type
/// can be read as query syntax. `None` if there is nothing to search for.
fn fts_query(text: &str) -> Option<String> {
    let text = folded(text);
    let words: Vec<String> = text.split_whitespace().map(|word| format!("\"{}\"*", word.replace('"', "\"\""))).collect();
    (!words.is_empty()).then(|| words.join(" "))
}

/// A read connection, one per thread that needs it. It cannot write.
pub struct Reader {
    conn: Connection,
}

impl Reader {
    /// The store must already exist: open a [`Store`] first.
    pub fn open(dir: &Path) -> Result<Self> {
        let conn = Connection::open(dir.join("meta.db"))?;
        configure(&conn, dir, 2048)?;
        conn.execute_batch("PRAGMA query_only = ON")?;
        Ok(Self { conn })
    }

    pub(crate) fn conn(&self) -> &Connection {
        &self.conn
    }

    pub fn images_allowed(&self, account_id: i64, address: &str) -> Result<bool> {
        let mut stmt = self.conn.prepare_cached("SELECT 1 FROM image_sender WHERE account_id = ?1 AND address = ?2")?;
        Ok(stmt.exists(params![account_id, address.to_lowercase()])?)
    }

    /// What is known of a domain's logo and when it was learned, if anyone has asked.
    pub fn logo(&self, domain: &str) -> Result<Option<(Option<Vec<u8>>, i64)>> {
        let mut stmt = self.conn.prepare_cached("SELECT image, checked FROM logo WHERE domain = ?1")?;
        Ok(stmt.query_row([domain], |r| Ok((r.get(0)?, r.get(1)?))).optional()?)
    }

    pub fn accounts(&self) -> Result<Vec<Account>> {
        let mut stmt = self.conn.prepare_cached("SELECT id, email, host, port, security, auth, smtp_host, smtp_port FROM account ORDER BY id")?;
        let rows = stmt.query_map([], |r| {
            Ok(Account {
                id: r.get(0)?,
                email: r.get(1)?,
                host: r.get(2)?,
                port: r.get(3)?,
                security: if r.get::<_, String>(4)? == "tls" { Security::Tls } else { Security::Plain },
                auth: if r.get::<_, String>(5)? == "google" { AuthKind::Google } else { AuthKind::Password },
                smtp_host: r.get(6)?,
                smtp_port: r.get(7)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Unread Inbox messages for every account that has any, by account id.
    pub fn inbox_unread(&self) -> Result<HashMap<i64, i64>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT m.account_id, count(*) FROM message m
             JOIN message_label ml ON ml.message_id = m.id
             JOIN label l ON l.id = ml.label_id
             WHERE m.seen = 0 AND l.role = 'inbox' GROUP BY m.account_id",
        )?;
        let counts = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
        Ok(counts.collect::<rusqlite::Result<_>>()?)
    }

    /// Those of `ids` that are unread in the Inbox, newest first.
    pub fn unread_in_inbox(&self, ids: &[i64]) -> Result<Vec<MessageRow>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT m.id, m.thread_id, m.from_name, m.from_addr, m.subject, m.date, m.seen, m.flagged, 1, m.account_id FROM message m
             WHERE m.id = ?1 AND m.seen = 0 AND EXISTS (
                 SELECT 1 FROM message_label ml JOIN label l ON l.id = ml.label_id
                 WHERE ml.message_id = m.id AND l.role = 'inbox')",
        )?;
        let mut rows = Vec::new();
        for id in ids {
            rows.extend(stmt.query_row([id], thread_row).optional()?);
        }
        rows.sort_by_key(|m| std::cmp::Reverse((m.date, m.id)));
        Ok(rows)
    }

    /// Every unread message with this label, loaded in the list or not.
    pub fn unread_in_label(&self, label: i64) -> Result<Vec<i64>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT m.id FROM message_label ml JOIN message m ON m.id = ml.message_id
             WHERE ml.label_id = ?1 AND m.seen = 0",
        )?;
        let ids = stmt.query_map([label], |r| r.get(0))?;
        Ok(ids.collect::<rusqlite::Result<_>>()?)
    }

    /// Labels in sidebar order: system labels first, then user labels by name.
    pub fn labels(&self, account_id: i64) -> Result<Vec<LabelRow>> {
        let mut unread: HashMap<i64, i64> = HashMap::new();
        let mut stmt = self.conn.prepare_cached(
            "SELECT ml.label_id, count(*) FROM message m
             JOIN message_label ml ON ml.message_id = m.id
             WHERE m.seen = 0 AND m.account_id = ?1 GROUP BY ml.label_id",
        )?;
        for row in stmt.query_map([account_id], |r| Ok((r.get(0)?, r.get(1)?)))? {
            let (id, count) = row?;
            unread.insert(id, count);
        }
        let mut stmt = self.conn.prepare_cached("SELECT id, name, role FROM label WHERE account_id = ?1")?;
        let mut labels = stmt
            .query_map([account_id], |r| {
                let id = r.get(0)?;
                Ok(LabelRow {
                    id,
                    name: r.get(1)?,
                    role: Role::parse(&r.get::<_, String>(2)?),
                    unread: unread.get(&id).copied().unwrap_or(0),
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        // Mail in its few seconds of undo is not stuck, and is not worth a line in the sidebar.
        let mut stuck = self.conn.prepare_cached(
            "SELECT 1 FROM draft d JOIN message m ON m.id = d.message_id WHERE m.account_id = ?1 AND d.state = 'retrying'",
        )?;
        let stuck = stuck.exists([account_id])?;
        labels.retain(|l| l.role != Role::Outbox || stuck);
        labels.sort_by_cached_key(|l| (l.role.rank(), l.name.to_lowercase()));
        Ok(labels)
    }

    /// The sidebar for these accounts. Several accounts share their system labels, one line for
    /// each role. User labels mean something only in their own account, so they stay there.
    pub fn sidebar(&self, accounts: &[i64]) -> Result<Vec<SidebarLabel>> {
        let mut lines: Vec<SidebarLabel> = Vec::new();
        for &account in accounts {
            for label in self.labels(account)? {
                match lines.iter_mut().find(|line| line.role == label.role && label.role != Role::None) {
                    Some(line) => {
                        line.held.push((account, label.id));
                        line.unread += label.unread;
                    }
                    None if label.role != Role::None || accounts.len() == 1 => lines.push(SidebarLabel {
                        held: vec![(account, label.id)],
                        name: label.name,
                        role: label.role,
                        unread: label.unread,
                    }),
                    None => {}
                }
            }
        }
        // A role only a later account has joins the lines out of order. The sort is stable, so
        // user labels keep the order they came in.
        lines.sort_by_key(|line| line.role.rank());
        Ok(lines)
    }

    /// Whether the account has a label with this role.
    pub fn has_role(&self, account_id: i64, role: Role) -> Result<bool> {
        let mut stmt = self.conn.prepare_cached("SELECT 1 FROM label WHERE account_id = ?1 AND role = ?2")?;
        Ok(stmt.exists(params![account_id, role.as_str()])?)
    }

    /// The account a message is in.
    pub fn account_of(&self, message_id: i64) -> Result<Option<i64>> {
        let mut stmt = self.conn.prepare_cached("SELECT account_id FROM message WHERE id = ?1")?;
        Ok(stmt.query_row([message_id], |r| r.get(0)).optional()?)
    }

    /// Threads in these labels. No labels, as for an account yet to sync, is no mail.
    pub fn message_count(&self, labels: &[i64]) -> Result<usize> {
        if labels.is_empty() {
            return Ok(0);
        }
        let mut stmt = self.conn.prepare_cached(&format!(
            "SELECT count(*) FROM message_label WHERE label_id IN ({}) AND head = 1",
            placeholders(1, labels.len())
        ))?;
        let count: i64 = stmt.query_row(params_from_iter(labels), |r| r.get(0))?;
        Ok(count as usize)
    }

    /// Where a thread sits in the list of these labels, or `None` if it is not there.
    pub fn position(&self, labels: &[i64], (account_id, thread_id): ThreadKey) -> Result<Option<usize>> {
        if labels.is_empty() {
            return Ok(None);
        }
        let held = placeholders(3, labels.len());
        let mut stmt = self.conn.prepare_cached(&format!(
            "SELECT (SELECT count(*) FROM message_label b
                     WHERE b.label_id IN ({held}) AND b.head = 1 AND (b.date, b.message_id) > (a.date, a.message_id))
             FROM message_label a JOIN message m ON m.id = a.message_id
             WHERE a.label_id IN ({held}) AND a.head = 1 AND m.account_id = ?1 AND m.thread_id = ?2"
        ))?;
        let values = [account_id, thread_id].into_iter().chain(labels.iter().copied());
        let position: Option<i64> = stmt.query_row(params_from_iter(values), |r| r.get(0)).optional()?;
        Ok(position.map(|p| p as usize))
    }

    /// One window of these labels' threads, newest first. Never load a whole mailbox. Each label
    /// is its own arm of the union because SQLite merges arms that are already in order, walking
    /// the indexes side by side, where `label_id IN` would sort all of them for every page.
    pub fn messages(&self, labels: &[i64], offset: usize, limit: usize) -> Result<Vec<MessageRow>> {
        if labels.is_empty() {
            return Ok(Vec::new());
        }
        let arms: Vec<String> = (0..labels.len())
            .map(|i| format!("SELECT date, message_id FROM message_label WHERE label_id = ?{} AND head = 1", i + 3))
            .collect();
        let mut stmt = self.conn.prepare_cached(&format!(
            "SELECT {}
             FROM ({} ORDER BY 1 DESC, 2 DESC LIMIT ?1 OFFSET ?2) page
             JOIN message m ON m.id = page.message_id
             ORDER BY page.date DESC, page.message_id DESC",
            *THREAD_COLUMNS,
            arms.join(" UNION ALL ")
        ))?;
        let values = [limit as i64, offset as i64].into_iter().chain(labels.iter().copied());
        let rows = stmt.query_map(params_from_iter(values), thread_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// A thread's messages, oldest first, as its reader sees them. `binned` asks for those in
    /// Trash or Spam instead of the rest, because a thread can be split between the two.
    pub fn thread(&self, account_id: i64, thread_id: i64, binned: bool) -> Result<Vec<MessageRow>> {
        let mut stmt = self.conn.prepare_cached(&format!(
            "SELECT t.id, t.thread_id, t.from_name, t.from_addr, t.subject, t.date, t.seen, t.flagged, 1, t.account_id
             FROM message t INDEXED BY message_thread
             WHERE t.account_id = ?1 AND t.thread_id = ?2 AND {READABLE} AND {IN_BIN} = ?3
             ORDER BY t.date, t.id"
        ))?;
        let rows = stmt.query_map(params![account_id, thread_id, binned], thread_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Threads matching a local search, newest first, leaving out Trash and Spam. Every word
    /// must match, as a prefix, in any part of the message that is stored here. Walks the mail
    /// from the newest and stops at `limit`, so a word in everything costs no more than a rare one.
    pub fn search(&self, account_id: i64, query: &str, limit: usize) -> Result<Vec<MessageRow>> {
        let Some(query) = fts_query(query) else { return Ok(Vec::new()) };
        let mut stmt = self.conn.prepare_cached(&format!(
            "SELECT t.id, t.thread_id FROM message t
             WHERE t.account_id = ?1 AND t.id IN (SELECT rowid FROM fts WHERE fts MATCH ?2) AND NOT {IN_BIN}
             ORDER BY t.date DESC, t.id DESC"
        ))?;
        let mut rows = stmt.query(params![account_id, query])?;
        let mut threads = HashSet::new();
        let mut newest = Vec::new();
        while newest.len() < limit
            && let Some(row) = rows.next()?
        {
            if threads.insert(row.get::<_, i64>(1)?) {
                newest.push(row.get(0)?);
            }
        }
        drop(rows);
        self.rows(&newest)
    }

    /// These messages as rows of their threads, in the order given. Unknown ids are left out.
    pub fn rows(&self, ids: &[i64]) -> Result<Vec<MessageRow>> {
        let mut stmt = self.conn.prepare_cached(&format!("SELECT {} FROM message m WHERE m.id = ?1", *THREAD_COLUMNS))?;
        let mut rows = Vec::with_capacity(ids.len());
        for id in ids {
            rows.extend(stmt.query_row([id], thread_row).optional()?);
        }
        Ok(rows)
    }

    /// The message at a UID, if we hold it.
    pub fn message_at(&self, account_id: i64, path: &str, uid: u32) -> Result<Option<i64>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT p.message_id FROM placement p JOIN folder f ON f.id = p.folder_id
             WHERE f.account_id = ?1 AND f.path = ?2 AND p.uid = ?3",
        )?;
        Ok(stmt.query_row(params![account_id, path, uid], |r| r.get(0)).optional()?)
    }

    /// The paths of the folders a search of the server should cover.
    pub fn searchable_folders(&self, account_id: i64) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT f.path FROM folder f JOIN label l ON l.id = f.label_id
             WHERE f.account_id = ?1 AND l.role NOT IN ('trash', 'spam')",
        )?;
        let paths = stmt.query_map([account_id], |r| r.get(0))?;
        Ok(paths.collect::<rusqlite::Result<_>>()?)
    }

    /// People whose name or address starts with `prefix`, most frequent first, from the mail
    /// they sent. Spam and machines that never read their mail are no use here.
    pub fn contacts(&self, account_id: i64, prefix: &str, limit: usize) -> Result<Vec<Contact>> {
        if prefix.is_empty() {
            return Ok(Vec::new());
        }
        let mut stmt = self.conn.prepare_cached(
            r"SELECT max(m.from_name), m.from_addr FROM message m
              WHERE m.account_id = ?1 AND m.from_addr <> ''
                AND (m.from_addr LIKE ?2 ESCAPE '\' OR m.from_name LIKE ?2 ESCAPE '\' OR m.from_name LIKE ?3 ESCAPE '\')
                AND m.from_addr NOT LIKE 'no%reply%' AND m.from_addr NOT LIKE 'notifications@%'
                AND NOT EXISTS (SELECT 1 FROM message_label x JOIN label l ON l.id = x.label_id
                                WHERE x.message_id = m.id AND l.role = 'spam')
              GROUP BY lower(m.from_addr) ORDER BY count(*) DESC LIMIT ?4",
        )?;
        let escaped = prefix.replace('\\', r"\\").replace('%', r"\%").replace('_', r"\_");
        let rows = stmt.query_map(
            params![account_id, format!("{escaped}%"), format!("% {escaped}%"), limit as i64],
            |r| Ok(Contact { name: r.get(0)?, address: r.get(1)? }),
        )?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// The labels a message has, by id.
    pub fn message_labels(&self, message_id: i64) -> Result<Vec<i64>> {
        let mut stmt = self.conn.prepare_cached("SELECT label_id FROM message_label WHERE message_id = ?1")?;
        let labels = stmt.query_map([message_id], |r| r.get(0))?;
        Ok(labels.collect::<rusqlite::Result<_>>()?)
    }

    pub fn body(&self, message_id: i64) -> Result<Option<Vec<u8>>> {
        let packed: Option<Vec<u8>> = self
            .conn
            .query_row("SELECT raw FROM bodies.body WHERE message_id = ?1", [message_id], |r| r.get(0))
            .optional()?;
        Ok(packed.map(|p| zstd::decode_all(p.as_slice())).transpose()?)
    }

    /// Where to fetch a message from: its folder path and UID, if it still has one.
    pub fn location(&self, message_id: i64) -> Result<Option<(String, u32)>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT f.path, p.uid FROM placement p JOIN folder f ON f.id = p.folder_id
             WHERE p.message_id = ?1 AND p.uid IS NOT NULL LIMIT 1",
        )?;
        Ok(stmt.query_row([message_id], |r| Ok((r.get(0)?, r.get(1)?))).optional()?)
    }

    pub fn has_body(&self, message_id: i64) -> Result<bool> {
        let mut stmt = self.conn.prepare_cached("SELECT 1 FROM bodies.body WHERE message_id = ?1")?;
        Ok(stmt.exists([message_id])?)
    }

    /// Messages without a local body among the `newest` in the Inbox, newest first.
    pub fn missing_bodies(&self, account_id: i64, max_size: u32, newest: usize) -> Result<Vec<i64>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT recent.message_id FROM (
                 SELECT ml.message_id, ml.date FROM message_label ml
                 JOIN label l ON l.id = ml.label_id
                 WHERE l.account_id = ?1 AND l.role = 'inbox'
                 ORDER BY ml.date DESC LIMIT ?3
             ) recent
             JOIN message m ON m.id = recent.message_id
             WHERE m.size <= ?2 AND recent.message_id NOT IN (SELECT message_id FROM bodies.body)
             ORDER BY recent.date DESC",
        )?;
        let ids = stmt.query_map(params![account_id, max_size, newest as i64], |r| r.get(0))?;
        Ok(ids.collect::<rusqlite::Result<_>>()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TempDir(std::path::PathBuf);

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    struct Fixture {
        store: Store,
        reader: Reader,
        account: i64,
        inbox: Folder,
        _dir: TempDir,
    }

    #[test]
    fn only_a_message_never_seen_before_is_new_and_only_unread_inbox_mail_is_news() {
        let f = fixture();
        let added = f.store.save_messages(&f.inbox, &[message(1, "first", 1), message(2, "second", 2)]).unwrap();
        assert_eq!(added.len(), 2);
        let work = f.store.folder(f.account, "Work", &LabelSpec::user("Work")).unwrap();
        assert!(f.store.save_messages(&work, &[message(9, "first", 1)]).unwrap().is_empty(), "the same mail, filed");

        let news = f.reader.unread_in_inbox(&added).unwrap();
        assert_eq!(news.iter().map(|m| m.id).collect::<Vec<_>>(), [added[1], added[0]], "newest first");

        let read = NewMessage { seen: true, ..message(3, "read", 3) };
        let added = f.store.save_messages(&f.inbox, &[read]).unwrap();
        assert!(f.reader.unread_in_inbox(&added).unwrap().is_empty());
        let filed = f.store.save_messages(&work, &[message(10, "filed", 4)]).unwrap();
        assert!(f.reader.unread_in_inbox(&filed).unwrap().is_empty());
    }

    fn fixture() -> Fixture {
        static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("mailfunction-store-{}-{n}", std::process::id()));
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
        let inbox = store.folder(account, "INBOX", &LabelSpec::system(Role::Inbox)).unwrap();
        Fixture { store, reader, account, inbox, _dir: TempDir(dir) }
    }

    fn message(uid: u32, remote_id: &str, date: i64) -> NewMessage {
        NewMessage {
            uid,
            remote_id: remote_id.into(),
            gm_thread_id: None,
            mid: None,
            references: vec![],
            subject: format!("Subject {uid}"),
            from_name: "Ann".into(),
            from_addr: "ann@example.com".into(),
            date,
            size: 100,
            seen: false,
            flagged: false,
            labels: vec![],
        }
    }

    #[test]
    fn lists_newest_first_and_pages() {
        let f = fixture();
        let batch: Vec<_> = (1..=5).map(|i| message(i, &format!("m{i}"), i as i64)).collect();
        f.store.save_messages(&f.inbox, &batch).unwrap();
        let label = f.inbox.label_id;
        assert_eq!(f.reader.message_count(&[label]).unwrap(), 5);
        let page: Vec<_> = f.reader.messages(&[label], 1, 2).unwrap();
        assert_eq!(page.iter().map(|m| m.date).collect::<Vec<_>>(), [4, 3]);
        assert_eq!(f.reader.position(&[label], page[0].thread_key()).unwrap(), Some(1));
        assert_eq!(f.reader.position(&[label], page[1].thread_key()).unwrap(), Some(2));
        assert_eq!(f.reader.position(&[label], (f.account, -1)).unwrap(), None);
    }

    /// A second account, with an Inbox of its own.
    fn other_account(f: &Fixture) -> (i64, Folder) {
        let email = "other@example.com".to_string();
        let other = f.store.add_account(&Account { email, ..f.reader.accounts().unwrap()[0].clone() }).unwrap();
        (other, f.store.folder(other, "INBOX", &LabelSpec::system(Role::Inbox)).unwrap())
    }

    #[test]
    fn accounts_share_a_list_and_keep_their_threads_to_themselves() {
        let f = fixture();
        let (other, theirs) = other_account(&f);
        // Gmail's thread ids come from each mailbox, so these two are strangers.
        let threaded = |uid, id, date| NewMessage { gm_thread_id: Some(7), ..message(uid, id, date) };
        f.store.save_messages(&f.inbox, &[threaded(1, "a", 10), threaded(2, "b", 30)]).unwrap();
        f.store.save_messages(&theirs, &[threaded(1, "c", 20), message(2, "d", 40)]).unwrap();
        let labels = [f.inbox.label_id, theirs.label_id];

        assert_eq!(f.reader.message_count(&labels).unwrap(), 3);
        let rows = f.reader.messages(&labels, 0, 10).unwrap();
        assert_eq!(rows.iter().map(|m| m.date).collect::<Vec<_>>(), [40, 30, 20], "newest first, whoever it is for");
        assert_eq!(rows.iter().map(|m| m.account_id).collect::<Vec<_>>(), [other, f.account, other]);
        assert_eq!(rows.iter().map(|m| m.count).collect::<Vec<_>>(), [1, 2, 1], "no thread reaches across");
        assert_eq!(f.reader.messages(&labels, 1, 1).unwrap()[0].date, 30, "pages run through the merged list");
        for (position, row) in rows.iter().enumerate() {
            assert_eq!(f.reader.position(&labels, row.thread_key()).unwrap(), Some(position));
        }

        assert_eq!(f.reader.message_count(&[]).unwrap(), 0, "an account yet to sync has no labels");
        assert!(f.reader.messages(&[], 0, 10).unwrap().is_empty());
        assert_eq!(f.reader.position(&[], rows[0].thread_key()).unwrap(), None);
    }

    #[test]
    fn the_sidebar_shares_system_labels_and_leaves_user_labels_at_home() {
        let f = fixture();
        let (other, theirs) = other_account(&f);
        let work = f.store.folder(f.account, "Work", &LabelSpec::user("Work")).unwrap();
        f.store.folder(f.account, "Trash", &LabelSpec::system(Role::Trash)).unwrap();
        f.store.folder(other, "Sent", &LabelSpec::system(Role::Sent)).unwrap();
        f.store.save_messages(&f.inbox, &[message(1, "a", 1)]).unwrap();
        f.store.save_messages(&work, &[message(2, "b", 2)]).unwrap();
        f.store.save_messages(&theirs, &[message(1, "c", 3), message(2, "d", 4)]).unwrap();

        let roles = |lines: &[SidebarLabel]| lines.iter().map(|l| l.role).collect::<Vec<_>>();
        let one = f.reader.sidebar(&[f.account]).unwrap();
        assert_eq!(roles(&one), [Role::Inbox, Role::Starred, Role::Trash, Role::None]);
        assert_eq!(one[3].name, "Work");

        let both = f.reader.sidebar(&[f.account, other]).unwrap();
        assert_eq!(roles(&both), [Role::Inbox, Role::Starred, Role::Sent, Role::Trash], "in sidebar order, wherever each came from");
        assert_eq!(both[0].held, [(f.account, f.inbox.label_id), (other, theirs.label_id)]);
        assert_eq!(both[0].unread, 3);
    }

    #[test]
    fn a_message_in_two_folders_is_one_message() {
        let f = fixture();
        let work = f.store.folder(f.account, "Work", &LabelSpec::user("Work")).unwrap();
        f.store.save_messages(&f.inbox, &[message(1, "same", 1)]).unwrap();
        f.store.save_messages(&work, &[message(9, "same", 1)]).unwrap();
        let id = f.reader.messages(&[f.inbox.label_id], 0, 1).unwrap()[0].id;
        assert_eq!(f.reader.messages(&[work.label_id], 0, 1).unwrap()[0].id, id);

        f.store.remove_uids(&f.inbox, &[1]).unwrap();
        assert_eq!(f.reader.message_count(&[f.inbox.label_id]).unwrap(), 0);
        assert_eq!(f.reader.messages(&[work.label_id], 0, 1).unwrap()[0].id, id);
    }

    #[test]
    fn removing_the_last_location_deletes_message_and_body() {
        let f = fixture();
        f.store.save_messages(&f.inbox, &[message(1, "m", 1)]).unwrap();
        let id = f.reader.messages(&[f.inbox.label_id], 0, 1).unwrap()[0].id;
        f.store.save_body(id, b"raw").unwrap();
        assert_eq!(f.reader.body(id).unwrap().as_deref(), Some(&b"raw"[..]));

        f.store.remove_uids(&f.inbox, &[1]).unwrap();
        assert_eq!(f.reader.message_count(&[f.inbox.label_id]).unwrap(), 0, "hidden at once");
        assert!(f.reader.body(id).unwrap().is_some(), "kept in case it moved to a later folder");

        f.store.purge_orphans(f.account).unwrap();
        assert_eq!(f.reader.body(id).unwrap(), None);
    }

    #[test]
    fn a_message_rediscovered_in_another_folder_keeps_its_identity_and_body() {
        let f = fixture();
        let work = f.store.folder(f.account, "Work", &LabelSpec::user("Work")).unwrap();
        f.store.save_messages(&f.inbox, &[message(1, "m", 1)]).unwrap();
        let id = f.reader.messages(&[f.inbox.label_id], 0, 1).unwrap()[0].id;
        f.store.save_body(id, b"raw").unwrap();

        f.store.remove_uids(&f.inbox, &[1]).unwrap();
        f.store.save_messages(&work, &[message(5, "m", 1)]).unwrap();
        f.store.purge_orphans(f.account).unwrap();

        assert_eq!(f.reader.messages(&[work.label_id], 0, 1).unwrap()[0].id, id);
        assert!(f.reader.body(id).unwrap().is_some());
    }

    #[test]
    fn a_uid_rescan_keeps_message_and_body_and_drops_only_what_is_gone() {
        let f = fixture();
        f.store.save_messages(&f.inbox, &[message(1, "kept", 1), message(2, "gone", 2)]).unwrap();
        let kept = f.reader.messages(&[f.inbox.label_id], 1, 1).unwrap()[0].id;
        f.store.save_body(kept, b"raw").unwrap();

        f.store.forget_uids(&f.inbox).unwrap();
        assert_eq!(f.reader.message_count(&[f.inbox.label_id]).unwrap(), 2, "still visible during the rescan");

        f.store.save_messages(&f.inbox, &[message(70, "kept", 1)]).unwrap();
        f.store.remove_unmatched(&f.inbox).unwrap();
        f.store.purge_orphans(f.account).unwrap();

        let rows = f.reader.messages(&[f.inbox.label_id], 0, 10).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, kept);
        assert_eq!(f.reader.body(kept).unwrap().as_deref(), Some(&b"raw"[..]));
        assert_eq!(f.reader.location(kept).unwrap().unwrap().1, 70);
    }

    #[test]
    fn changes_update_flags_and_starred_and_skip_unknown_uids() {
        let f = fixture();
        f.store.save_messages(&f.inbox, &[message(1, "m", 1)]).unwrap();
        f.store
            .apply_changes(
                &f.inbox,
                &[
                    Change { uid: 1, seen: true, flagged: true, labels: None },
                    Change { uid: 99, seen: true, flagged: true, labels: None },
                ],
            )
            .unwrap();
        let row = &f.reader.messages(&[f.inbox.label_id], 0, 1).unwrap()[0];
        assert!(row.seen && row.flagged);
        let starred = f.reader.labels(f.account).unwrap().into_iter().find(|l| l.role == Role::Starred).unwrap();
        assert_eq!(f.reader.message_count(&[starred.id]).unwrap(), 1);

        f.store.apply_changes(&f.inbox, &[Change { uid: 1, seen: true, flagged: false, labels: None }]).unwrap();
        assert_eq!(f.reader.message_count(&[starred.id]).unwrap(), 0);
        assert_eq!(f.reader.message_count(&[f.inbox.label_id]).unwrap(), 1, "the folder label survives");
    }

    #[test]
    fn server_labels_replace_old_ones_and_counts_track_unread() {
        let f = fixture();
        let mut m = message(1, "m", 1);
        m.labels = vec![LabelSpec::user("Receipts"), LabelSpec::user("Hóa đơn")];
        f.store.save_messages(&f.inbox, &[m]).unwrap();
        let labels = f.reader.labels(f.account).unwrap();
        let names: Vec<_> = labels.iter().map(|l| l.name.as_str()).collect();
        assert_eq!(names, ["Inbox", "Starred", "Hóa đơn", "Receipts"]);
        assert_eq!(labels.iter().find(|l| l.name == "Receipts").unwrap().unread, 1);

        let change = Change { uid: 1, seen: true, flagged: false, labels: Some(vec![LabelSpec::user("Hóa đơn")]) };
        f.store.apply_changes(&f.inbox, &[change]).unwrap();
        let labels = f.reader.labels(f.account).unwrap();
        let receipts = labels.iter().find(|l| l.name == "Receipts").unwrap();
        assert_eq!(f.reader.message_count(&[receipts.id]).unwrap(), 0);
        assert_eq!(receipts.unread, 0);
    }

    #[test]
    fn prefetch_covers_only_the_newest_inbox_mail_of_a_sane_size() {
        let f = fixture();
        let mut huge = message(3, "huge", 3);
        huge.size = 10_000_000;
        f.store.save_messages(&f.inbox, &[message(1, "old", 1), message(2, "new", 2), huge]).unwrap();
        let rows = f.reader.messages(&[f.inbox.label_id], 0, 3).unwrap();
        let (new, old) = (rows[1].id, rows[2].id);

        assert_eq!(f.reader.missing_bodies(f.account, 1000, 10).unwrap(), [new, old]);
        f.store.save_body(new, b"raw").unwrap();
        assert!(f.reader.has_body(new).unwrap());
        assert_eq!(f.reader.missing_bodies(f.account, 1000, 10).unwrap(), [old]);
        assert!(f.reader.missing_bodies(f.account, 1000, 2).unwrap().is_empty(), "old is outside the window");
    }

    #[test]
    fn removing_an_account_takes_only_its_own_data() {
        let f = fixture();
        let other = f
            .store
            .add_account(&Account {
                id: 0,
                email: "other@example.com".into(),
                host: "localhost".into(),
                port: 993,
                security: Security::Tls,
                auth: AuthKind::Password,
                smtp_host: "localhost".into(),
                smtp_port: 465,
            })
            .unwrap();
        let other_inbox = f.store.folder(other, "INBOX", &LabelSpec::system(Role::Inbox)).unwrap();
        // The same Message-ID in both accounts must stay two messages.
        f.store.save_messages(&f.inbox, &[message(1, "same", 1), message(2, "mine", 2)]).unwrap();
        f.store.save_messages(&other_inbox, &[message(1, "same", 1)]).unwrap();
        let mine = f.reader.messages(&[f.inbox.label_id], 0, 1).unwrap()[0].id;
        let theirs = f.reader.messages(&[other_inbox.label_id], 0, 1).unwrap()[0].id;
        f.store.save_body(mine, b"mine").unwrap();
        f.store.save_body(theirs, b"theirs").unwrap();
        assert_eq!(f.reader.inbox_unread().unwrap(), HashMap::from([(f.account, 2), (other, 1)]));

        f.store.remove_account(f.account).unwrap();

        assert_eq!(f.reader.accounts().unwrap().len(), 1);
        assert!(f.reader.labels(f.account).unwrap().is_empty());
        assert_eq!(f.reader.body(mine).unwrap(), None);
        assert_eq!(f.reader.body(theirs).unwrap().as_deref(), Some(&b"theirs"[..]));
        assert_eq!(f.reader.inbox_unread().unwrap(), HashMap::from([(other, 1)]));

        // A fetch that was in flight when the account went must not leave mail behind.
        f.store.save_body(mine, b"late").unwrap();
        assert_eq!(f.reader.body(mine).unwrap(), None);
    }

    #[test]
    fn the_reader_cannot_write() {
        let f = fixture();
        assert!(f.reader.conn.execute("DELETE FROM account", []).is_err());
    }

    #[test]
    fn folder_progress_survives_reopening() {
        let f = fixture();
        let mut folder = f.inbox.clone();
        folder.uid_validity = 7;
        folder.next_uid = 100;
        folder.backfill_uid = 40;
        folder.modseq = u64::from(u32::MAX) + 5;
        f.store.save_progress(&folder).unwrap();
        let again = f.store.folder(f.account, "INBOX", &LabelSpec::system(Role::Inbox)).unwrap();
        assert_eq!((again.uid_validity, again.next_uid, again.backfill_uid, again.modseq), (7, 100, 40, folder.modseq));
    }
}
