//! The local store. `meta.db` holds what lists need; `bodies.db` holds compressed raw messages
//! so that the hot file stays small. Identity lives in `message`, location in `placement`:
//! server UIDs are never a key, so a UIDVALIDITY change cannot lose a message.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Mutex;

use rusqlite::{Connection, OptionalExtension, Transaction, params};

use crate::Result;

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
"];

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
}

/// What a label means to the app. Anything without a role is a user label.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Role {
    Inbox,
    Starred,
    Sent,
    Drafts,
    All,
    Spam,
    Trash,
    None,
}

impl Role {
    fn as_str(self) -> &'static str {
        match self {
            Role::Inbox => "inbox",
            Role::Starred => "starred",
            Role::Sent => "sent",
            Role::Drafts => "drafts",
            Role::All => "all",
            Role::Spam => "spam",
            Role::Trash => "trash",
            Role::None => "",
        }
    }

    fn parse(s: &str) -> Self {
        match s {
            "inbox" => Role::Inbox,
            "starred" => Role::Starred,
            "sent" => Role::Sent,
            "drafts" => Role::Drafts,
            "all" => Role::All,
            "spam" => Role::Spam,
            "trash" => Role::Trash,
            _ => Role::None,
        }
    }

    /// Display name, which is also the stored name for a system label.
    pub fn title(self) -> &'static str {
        match self {
            Role::Inbox => "Inbox",
            Role::Starred => "Starred",
            Role::Sent => "Sent",
            Role::Drafts => "Drafts",
            Role::All => "All Mail",
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessageRow {
    pub id: i64,
    pub from_name: String,
    pub from_addr: String,
    pub subject: String,
    pub date: i64,
    pub seen: bool,
    pub flagged: bool,
}

fn message_row(r: &rusqlite::Row) -> rusqlite::Result<MessageRow> {
    Ok(MessageRow {
        id: r.get(0)?,
        from_name: r.get(1)?,
        from_addr: r.get(2)?,
        subject: r.get(3)?,
        date: r.get(4)?,
        seen: r.get(5)?,
        flagged: r.get(6)?,
    })
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
        Ok(Self { conn: Mutex::new(conn) })
    }

    fn with_tx<T>(&self, f: impl FnOnce(&Transaction) -> Result<T>) -> Result<T> {
        let mut conn = self.conn.lock().expect("store lock");
        let tx = conn.transaction()?;
        let out = f(&tx)?;
        tx.commit()?;
        Ok(out)
    }

    pub fn add_account(&self, a: &Account) -> Result<i64> {
        self.with_tx(|tx| {
            tx.execute(
                "INSERT INTO account (email, host, port, security, auth) VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT (email) DO UPDATE SET host = ?2, port = ?3, security = ?4, auth = ?5",
                params![
                    a.email,
                    a.host,
                    a.port,
                    if a.security == Security::Tls { "tls" } else { "plain" },
                    if a.auth == AuthKind::Google { "google" } else { "password" },
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
            let mut labels = HashMap::new();
            let mut added = Vec::new();
            for m in messages {
                let known: Option<i64> = tx
                    .query_row(
                        "SELECT id FROM message WHERE account_id = ?1 AND remote_id = ?2",
                        params![folder.account_id, m.remote_id],
                        |r| r.get(0),
                    )
                    .optional()?;
                let id = if let Some(id) = known {
                    tx.execute("UPDATE message SET seen = ?2, flagged = ?3 WHERE id = ?1", params![id, m.seen, m.flagged])?;
                    id
                } else {
                    tx.execute(
                        "INSERT INTO message
                           (account_id, remote_id, gm_thread_id, subject, from_name, from_addr, date, size, seen, flagged)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                        params![
                            folder.account_id,
                            m.remote_id,
                            m.gm_thread_id.map(|t| t as i64),
                            m.subject,
                            m.from_name,
                            m.from_addr,
                            m.date,
                            m.size,
                            m.seen,
                            m.flagged,
                        ],
                    )?;
                    let id = tx.last_insert_rowid();
                    added.push(id);
                    id
                };
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
                set_labels(tx, folder.account_id, id, m.flagged, Some(&m.labels), &mut labels)?;
            }
            Ok(added)
        })
    }

    /// Applies flag and label changes. UIDs we do not hold yet are skipped: backfill will
    /// fetch those messages with their current state.
    pub fn apply_changes(&self, folder: &Folder, changes: &[Change]) -> Result<()> {
        self.with_tx(|tx| {
            let mut labels = HashMap::new();
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
                set_labels(tx, folder.account_id, id, c.flagged, c.labels.as_deref(), &mut labels)?;
            }
            Ok(())
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
                }
            }
            Ok(())
        })
    }

    /// After a complete scan, anything still without a UID is really gone from the server.
    pub fn remove_unmatched(&self, folder: &Folder) -> Result<()> {
        self.with_tx(|tx| {
            let mut stmt = tx.prepare("SELECT message_id FROM placement WHERE folder_id = ?1 AND uid IS NULL")?;
            let gone: Vec<i64> = stmt.query_map([folder.id], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
            for id in gone {
                remove_placement(tx, folder, id)?;
            }
            Ok(())
        })
    }

    /// Deletes messages that no folder claimed. Call only after a pass over every folder.
    pub fn purge_orphans(&self, account_id: i64) -> Result<()> {
        const ORPHANS: &str = "SELECT id FROM message m WHERE account_id = ?1
                               AND NOT EXISTS (SELECT 1 FROM placement p WHERE p.message_id = m.id)";
        self.with_tx(|tx| {
            tx.execute(&format!("DELETE FROM bodies.body WHERE message_id IN ({ORPHANS})"), [account_id])?;
            tx.execute(&format!("DELETE FROM message WHERE id IN ({ORPHANS})"), [account_id])?;
            Ok(())
        })
    }

    /// Keeps a message's body. One that arrives after its account was removed is dropped, since
    /// `bodies.db` is a separate file and no foreign key can refuse it.
    pub fn save_body(&self, message_id: i64, raw: &[u8]) -> Result<()> {
        let packed = zstd::bulk::compress(raw, 3)?;
        self.with_tx(|tx| {
            tx.execute(
                "INSERT OR REPLACE INTO bodies.body (message_id, raw)
                 SELECT ?1, ?2 WHERE EXISTS (SELECT 1 FROM message WHERE id = ?1)",
                params![message_id, packed],
            )?;
            Ok(())
        })
    }
}

fn label_id(tx: &Transaction, account_id: i64, spec: &LabelSpec) -> Result<i64> {
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
/// With `extra` as `None`, labels that came from the server are left alone.
fn set_labels(
    tx: &Transaction,
    account_id: i64,
    message_id: i64,
    flagged: bool,
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
    let date: i64 = tx.query_row("SELECT date FROM message WHERE id = ?1", [message_id], |r| r.get(0))?;
    let add = |label: i64| {
        tx.execute("INSERT OR IGNORE INTO message_label VALUES (?1, ?2, ?3)", params![label, date, message_id])
    };
    if flagged {
        add(starred)?;
    }
    for spec in extra.unwrap_or_default() {
        add(id_of(spec)?)?;
    }
    tx.execute(
        "INSERT OR IGNORE INTO message_label
         SELECT f.label_id, ?2, ?1 FROM placement p JOIN folder f ON f.id = p.folder_id WHERE p.message_id = ?1",
        params![message_id, date],
    )?;
    Ok(())
}

fn remove_placement(tx: &Transaction, folder: &Folder, message_id: i64) -> Result<()> {
    tx.execute("DELETE FROM placement WHERE folder_id = ?1 AND message_id = ?2", params![folder.id, message_id])?;
    tx.execute("DELETE FROM message_label WHERE label_id = ?1 AND message_id = ?2", params![folder.label_id, message_id])?;
    let remaining: i64 = tx.query_row("SELECT count(*) FROM placement WHERE message_id = ?1", [message_id], |r| r.get(0))?;
    if remaining == 0 {
        tx.execute("DELETE FROM message_label WHERE message_id = ?1", [message_id])?;
    }
    Ok(())
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
        let mut stmt = self.conn.prepare_cached("SELECT id, email, host, port, security, auth FROM account ORDER BY id")?;
        let rows = stmt.query_map([], |r| {
            Ok(Account {
                id: r.get(0)?,
                email: r.get(1)?,
                host: r.get(2)?,
                port: r.get(3)?,
                security: if r.get::<_, String>(4)? == "tls" { Security::Tls } else { Security::Plain },
                auth: if r.get::<_, String>(5)? == "google" { AuthKind::Google } else { AuthKind::Password },
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
            "SELECT m.id, m.from_name, m.from_addr, m.subject, m.date, m.seen, m.flagged FROM message m
             WHERE m.id = ?1 AND m.seen = 0 AND EXISTS (
                 SELECT 1 FROM message_label ml JOIN label l ON l.id = ml.label_id
                 WHERE ml.message_id = m.id AND l.role = 'inbox')",
        )?;
        let mut rows = Vec::new();
        for id in ids {
            rows.extend(stmt.query_row([id], message_row).optional()?);
        }
        rows.sort_by_key(|m| std::cmp::Reverse((m.date, m.id)));
        Ok(rows)
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
        const ORDER: [Role; 8] =
            [Role::Inbox, Role::Starred, Role::Sent, Role::Drafts, Role::All, Role::Spam, Role::Trash, Role::None];
        labels.sort_by_cached_key(|l| (ORDER.iter().position(|r| *r == l.role), l.name.to_lowercase()));
        Ok(labels)
    }

    pub fn message_count(&self, label_id: i64) -> Result<usize> {
        let count: i64 = self.conn.query_row("SELECT count(*) FROM message_label WHERE label_id = ?1", [label_id], |r| r.get(0))?;
        Ok(count as usize)
    }

    /// Where a message sits in a label's list, or `None` if it is not there.
    pub fn position(&self, label_id: i64, message_id: i64) -> Result<Option<usize>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT (SELECT count(*) FROM message_label b
                     WHERE b.label_id = a.label_id AND (b.date, b.message_id) > (a.date, a.message_id))
             FROM message_label a WHERE a.label_id = ?1 AND a.message_id = ?2",
        )?;
        let position: Option<i64> = stmt.query_row(params![label_id, message_id], |r| r.get(0)).optional()?;
        Ok(position.map(|p| p as usize))
    }

    /// One window of a label, newest first. Never load a whole mailbox.
    pub fn messages(&self, label_id: i64, offset: usize, limit: usize) -> Result<Vec<MessageRow>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT m.id, m.from_name, m.from_addr, m.subject, m.date, m.seen, m.flagged
             FROM message_label ml JOIN message m ON m.id = ml.message_id
             WHERE ml.label_id = ?1
             ORDER BY ml.date DESC, ml.message_id DESC
             LIMIT ?2 OFFSET ?3",
        )?;
        let rows = stmt.query_map(params![label_id, limit as i64, offset as i64], message_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
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
        assert_eq!(f.reader.message_count(label).unwrap(), 5);
        let page: Vec<_> = f.reader.messages(label, 1, 2).unwrap();
        assert_eq!(page.iter().map(|m| m.date).collect::<Vec<_>>(), [4, 3]);
        assert_eq!(f.reader.position(label, page[0].id).unwrap(), Some(1));
        assert_eq!(f.reader.position(label, page[1].id).unwrap(), Some(2));
        assert_eq!(f.reader.position(label, -1).unwrap(), None);
    }

    #[test]
    fn a_message_in_two_folders_is_one_message() {
        let f = fixture();
        let work = f.store.folder(f.account, "Work", &LabelSpec::user("Work")).unwrap();
        f.store.save_messages(&f.inbox, &[message(1, "same", 1)]).unwrap();
        f.store.save_messages(&work, &[message(9, "same", 1)]).unwrap();
        let id = f.reader.messages(f.inbox.label_id, 0, 1).unwrap()[0].id;
        assert_eq!(f.reader.messages(work.label_id, 0, 1).unwrap()[0].id, id);

        f.store.remove_uids(&f.inbox, &[1]).unwrap();
        assert_eq!(f.reader.message_count(f.inbox.label_id).unwrap(), 0);
        assert_eq!(f.reader.messages(work.label_id, 0, 1).unwrap()[0].id, id);
    }

    #[test]
    fn removing_the_last_location_deletes_message_and_body() {
        let f = fixture();
        f.store.save_messages(&f.inbox, &[message(1, "m", 1)]).unwrap();
        let id = f.reader.messages(f.inbox.label_id, 0, 1).unwrap()[0].id;
        f.store.save_body(id, b"raw").unwrap();
        assert_eq!(f.reader.body(id).unwrap().as_deref(), Some(&b"raw"[..]));

        f.store.remove_uids(&f.inbox, &[1]).unwrap();
        assert_eq!(f.reader.message_count(f.inbox.label_id).unwrap(), 0, "hidden at once");
        assert!(f.reader.body(id).unwrap().is_some(), "kept in case it moved to a later folder");

        f.store.purge_orphans(f.account).unwrap();
        assert_eq!(f.reader.body(id).unwrap(), None);
    }

    #[test]
    fn a_message_rediscovered_in_another_folder_keeps_its_identity_and_body() {
        let f = fixture();
        let work = f.store.folder(f.account, "Work", &LabelSpec::user("Work")).unwrap();
        f.store.save_messages(&f.inbox, &[message(1, "m", 1)]).unwrap();
        let id = f.reader.messages(f.inbox.label_id, 0, 1).unwrap()[0].id;
        f.store.save_body(id, b"raw").unwrap();

        f.store.remove_uids(&f.inbox, &[1]).unwrap();
        f.store.save_messages(&work, &[message(5, "m", 1)]).unwrap();
        f.store.purge_orphans(f.account).unwrap();

        assert_eq!(f.reader.messages(work.label_id, 0, 1).unwrap()[0].id, id);
        assert!(f.reader.body(id).unwrap().is_some());
    }

    #[test]
    fn a_uid_rescan_keeps_message_and_body_and_drops_only_what_is_gone() {
        let f = fixture();
        f.store.save_messages(&f.inbox, &[message(1, "kept", 1), message(2, "gone", 2)]).unwrap();
        let kept = f.reader.messages(f.inbox.label_id, 1, 1).unwrap()[0].id;
        f.store.save_body(kept, b"raw").unwrap();

        f.store.forget_uids(&f.inbox).unwrap();
        assert_eq!(f.reader.message_count(f.inbox.label_id).unwrap(), 2, "still visible during the rescan");

        f.store.save_messages(&f.inbox, &[message(70, "kept", 1)]).unwrap();
        f.store.remove_unmatched(&f.inbox).unwrap();
        f.store.purge_orphans(f.account).unwrap();

        let rows = f.reader.messages(f.inbox.label_id, 0, 10).unwrap();
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
        let row = &f.reader.messages(f.inbox.label_id, 0, 1).unwrap()[0];
        assert!(row.seen && row.flagged);
        let starred = f.reader.labels(f.account).unwrap().into_iter().find(|l| l.role == Role::Starred).unwrap();
        assert_eq!(f.reader.message_count(starred.id).unwrap(), 1);

        f.store.apply_changes(&f.inbox, &[Change { uid: 1, seen: true, flagged: false, labels: None }]).unwrap();
        assert_eq!(f.reader.message_count(starred.id).unwrap(), 0);
        assert_eq!(f.reader.message_count(f.inbox.label_id).unwrap(), 1, "the folder label survives");
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
        assert_eq!(f.reader.message_count(receipts.id).unwrap(), 0);
        assert_eq!(receipts.unread, 0);
    }

    #[test]
    fn prefetch_covers_only_the_newest_inbox_mail_of_a_sane_size() {
        let f = fixture();
        let mut huge = message(3, "huge", 3);
        huge.size = 10_000_000;
        f.store.save_messages(&f.inbox, &[message(1, "old", 1), message(2, "new", 2), huge]).unwrap();
        let rows = f.reader.messages(f.inbox.label_id, 0, 3).unwrap();
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
            })
            .unwrap();
        let other_inbox = f.store.folder(other, "INBOX", &LabelSpec::system(Role::Inbox)).unwrap();
        // The same Message-ID in both accounts must stay two messages.
        f.store.save_messages(&f.inbox, &[message(1, "same", 1), message(2, "mine", 2)]).unwrap();
        f.store.save_messages(&other_inbox, &[message(1, "same", 1)]).unwrap();
        let mine = f.reader.messages(f.inbox.label_id, 0, 1).unwrap()[0].id;
        let theirs = f.reader.messages(other_inbox.label_id, 0, 1).unwrap()[0].id;
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
