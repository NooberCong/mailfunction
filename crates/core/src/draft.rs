//! Drafts and the outbox. A draft is a message that exists here first: a `message` row with a
//! `local:` identity and a `draft` row with its fields. It autosaves locally, and goes to the
//! server when the user leaves it. When a copy comes back through sync, its Message-ID ties it to
//! the same row, so nothing appears twice. Sent mail works the same way, until the server's own
//! copy turns up.
//!
//! Every upload gets a new Message-ID, because Gmail treats a repeated one as the same message.
//! `old_mid` remembers the copy on the server, which the next upload replaces.

use lettre::address::Envelope;
use lettre::message::Mailbox;
use mail_parser::MessageParser;
use rusqlite::{Connection, OptionalExtension, Transaction, params};

use crate::compose::{self, Attachment, Compose};
use crate::imap::Imap;
use crate::ops::{Action, enqueue};
use crate::store::{LabelSpec, Reader, Role, Store, add_label, index_message, label_id, place_in_thread, refresh_heads, thread_of};
use crate::{Error, Result};

/// Gmail's limit on what can be sent, less what base64 adds.
pub const MAX_ATTACHMENTS: usize = 18 * 1024 * 1024;

/// A file as a list shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttachmentInfo {
    pub name: String,
    pub size: usize,
}

fn random_hex() -> Result<String> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).map_err(|e| Error::Protocol(e.to_string()))?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

fn new_mid(email: &str) -> Result<String> {
    Ok(format!("{}@{}", random_hex()?, email.rsplit('@').next().unwrap_or("localhost")))
}

fn set_only_label(tx: &Transaction, account_id: i64, message_id: i64, role: Role) -> Result<()> {
    tx.execute("DELETE FROM message_label WHERE message_id = ?1", [message_id])?;
    add_label(tx, label_id(tx, account_id, &LabelSpec::system(role))?, message_id)?;
    refresh_heads(tx, account_id, [thread_of(tx, message_id)?])
}

fn email_of(tx: &Transaction, account_id: i64) -> Result<String> {
    Ok(tx.query_row("SELECT email FROM account WHERE id = ?1", [account_id], |r| r.get(0))?)
}

/// The name the user gave the account, or else the one in its own sent mail.
fn sender(tx: &Transaction, account_id: i64) -> Result<Mailbox> {
    let email = email_of(tx, account_id)?;
    let chosen: String = tx.query_row("SELECT name FROM account WHERE id = ?1", [account_id], |r| r.get(0))?;
    let name: Option<String> = if chosen.trim().is_empty() {
        tx.query_row(
            "SELECT from_name FROM message WHERE account_id = ?1 AND from_addr = ?2 AND from_name <> '' LIMIT 1",
            params![account_id, email],
            |r| r.get(0),
        )
        .optional()?
    } else {
        Some(chosen)
    };
    let address = email.parse().map_err(|_| Error::Protocol(format!("{email} is not an address")))?;
    Ok(Mailbox::new(name, address))
}

fn is_gmail(tx: &Transaction, account_id: i64) -> Result<bool> {
    Ok(tx.prepare_cached("SELECT 1 FROM label WHERE account_id = ?1 AND role = 'all'")?.exists([account_id])?)
}

fn read_fields(conn: &Connection, message_id: i64) -> Result<Option<Compose>> {
    let mut stmt = conn.prepare_cached(
        "SELECT d.recipients, d.cc, d.bcc, m.subject, d.body, d.in_reply_to, d.refs
         FROM draft d JOIN message m ON m.id = d.message_id WHERE d.message_id = ?1",
    )?;
    let fields = stmt.query_row([message_id], |r| {
        Ok(Compose {
            to: r.get(0)?,
            cc: r.get(1)?,
            bcc: r.get(2)?,
            subject: r.get(3)?,
            body: r.get(4)?,
            in_reply_to: r.get(5)?,
            references: r.get(6)?,
        })
    });
    Ok(fields.optional()?)
}

fn read_attachments(tx: &Transaction, message_id: i64) -> Result<Vec<Attachment>> {
    let mut stmt = tx.prepare_cached("SELECT name, mime, data FROM bodies.attachment WHERE message_id = ?1 ORDER BY position")?;
    let files = stmt.query_map([message_id], |r| Ok(Attachment { name: r.get(0)?, mime: r.get(1)?, data: r.get(2)? }))?;
    Ok(files.collect::<rusqlite::Result<_>>()?)
}

/// Fails for anything no longer being edited: mail that is going, or has gone, cannot change.
fn write_fields(tx: &Transaction, id: i64, c: &Compose) -> Result<()> {
    let changed = tx.execute(
        "UPDATE draft SET recipients = ?2, cc = ?3, bcc = ?4, body = ?5, in_reply_to = ?6, refs = ?7
         WHERE message_id = ?1 AND state = 'editing'",
        params![id, c.to, c.cc, c.bcc, c.body, c.in_reply_to, c.references],
    )?;
    if changed == 0 {
        return Err(Error::Refused("this message can no longer be changed".into()));
    }
    tx.execute("UPDATE message SET subject = ?2 WHERE id = ?1", params![id, c.subject])?;
    index_message(tx, id, &c.body)
}

impl Store {
    /// Starts a draft, in the thread of the message it answers if there is one.
    pub fn create_draft(&self, account_id: i64, compose: &Compose, reply_to: Option<i64>, files: &[Attachment]) -> Result<i64> {
        let local = format!("local:{}", random_hex()?);
        let now = chrono::Utc::now().timestamp();
        self.with_tx(|tx| {
            let from = sender(tx, account_id)?;
            let thread: Option<i64> = reply_to.map(|id| thread_of(tx, id)).transpose()?;
            tx.execute(
                "INSERT INTO message
                   (account_id, remote_id, thread_id, subject, from_name, from_addr, date, size, seen, flagged)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 0, 1, 0)",
                params![account_id, local, thread, compose.subject, from.name.unwrap_or_default(), from.email.to_string(), now],
            )?;
            let id = tx.last_insert_rowid();
            let references: Vec<String> = compose.references.split_whitespace().map(String::from).collect();
            place_in_thread(tx, account_id, id, thread, None, &references)?;
            tx.execute("INSERT INTO draft (message_id) VALUES (?1)", [id])?;
            write_fields(tx, id, compose)?;
            for file in files {
                insert_attachment(tx, id, file)?;
            }
            set_only_label(tx, account_id, id, Role::Drafts)?;
            Ok(id)
        })
    }

    /// Takes over a message as a draft: one written in another client, on being opened. Its copy
    /// on the server is replaced at the next upload.
    pub fn adopt_draft(&self, message_id: i64, raw: &[u8]) -> Result<Option<Compose>> {
        let Some((compose, mid)) = compose::from_raw(raw) else { return Ok(None) };
        let files = compose::attachments(raw);
        self.with_tx(|tx| {
            if tx.prepare_cached("SELECT 1 FROM draft WHERE message_id = ?1")?.exists([message_id])? {
                return read_fields(tx, message_id);
            }
            // Anything else is mail someone sent, and it must never become a draft.
            let in_drafts = tx.prepare_cached(
                "SELECT 1 FROM message_label ml JOIN label l ON l.id = ml.label_id
                 WHERE ml.message_id = ?1 AND l.role = 'drafts'",
            )?;
            if !{ in_drafts }.exists([message_id])? {
                return Ok(None);
            }
            tx.execute("INSERT INTO draft (message_id, old_mid) VALUES (?1, ?2)", params![message_id, mid])?;
            tx.execute("UPDATE message SET mid = ?2 WHERE id = ?1", params![message_id, mid])?;
            write_fields(tx, message_id, &compose)?;
            for file in &files {
                insert_attachment(tx, message_id, file)?;
            }
            Ok(Some(compose))
        })
    }

    /// Autosaves. Local only, so it can run on every pause in typing.
    pub fn update_draft(&self, message_id: i64, compose: &Compose) -> Result<()> {
        self.with_tx(|tx| write_fields(tx, message_id, compose))
    }

    /// Attaches a file to a draft, or says why not.
    pub fn attach(&self, message_id: i64, file: &Attachment) -> Result<()> {
        self.with_tx(|tx| {
            let total: i64 = tx.query_row(
                "SELECT coalesce(sum(length(data)), 0) FROM bodies.attachment WHERE message_id = ?1",
                [message_id],
                |r| r.get(0),
            )?;
            if total as usize + file.data.len() > MAX_ATTACHMENTS {
                return Err(Error::Refused(format!("attachments are limited to {} MB", MAX_ATTACHMENTS / 1024 / 1024)));
            }
            insert_attachment(tx, message_id, file)
        })
    }

    pub fn remove_attachment(&self, message_id: i64, index: usize) -> Result<()> {
        self.with_tx(|tx| {
            tx.execute(
                "DELETE FROM bodies.attachment WHERE message_id = ?1 AND position =
                 (SELECT position FROM bodies.attachment WHERE message_id = ?1 ORDER BY position LIMIT 1 OFFSET ?2)",
                params![message_id, index as i64],
            )?;
            Ok(())
        })
    }

    /// The user has left the draft: its current state goes to the server.
    pub fn upload_draft(&self, message_id: i64) -> Result<()> {
        self.with_tx(|tx| {
            let (account, email): (i64, String) = tx.query_row(
                "SELECT m.account_id, a.email FROM message m JOIN account a ON a.id = m.account_id WHERE m.id = ?1",
                [message_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?;
            let editing = tx.prepare_cached("SELECT 1 FROM draft WHERE message_id = ?1 AND state = 'editing'")?;
            if !{ editing }.exists([message_id])? {
                return Err(Error::Refused("this message can no longer be changed".into()));
            }
            let Some(compose) = read_fields(tx, message_id)? else { return Ok(()) };
            let mid = new_mid(&email)?;
            tx.execute("UPDATE message SET mid = ?2 WHERE id = ?1", params![message_id, mid])?;
            // So that it opens from the list, and reads back, like any other message.
            let raw = compose::draft_bytes(&compose, &sender(tx, account)?, &mid, &read_attachments(tx, message_id)?)
                .map_err(Error::Protocol)?;
            tx.execute(
                "INSERT OR REPLACE INTO bodies.body (message_id, raw) VALUES (?1, ?2)",
                params![message_id, zstd::bulk::compress(&raw, 3)?],
            )?;
            enqueue(tx, account, message_id, &Action::SaveDraft, None)
        })
    }

    /// Throws a draft away, here and on the server.
    pub fn discard_draft(&self, message_id: i64) -> Result<()> {
        self.with_tx(|tx| {
            let found: Option<(i64, Option<String>, i64)> = tx
                .query_row(
                    "SELECT m.account_id, d.old_mid, m.thread_id FROM draft d JOIN message m ON m.id = d.message_id
                     WHERE d.message_id = ?1",
                    [message_id],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .optional()?;
            let Some((account, old_mid, thread)) = found else { return Ok(()) };
            forget_message(tx, message_id)?;
            refresh_heads(tx, account, [thread])?;
            if let Some(mid) = old_mid {
                enqueue(tx, account, message_id, &Action::DiscardDraft { mid }, None)?;
            }
            Ok(())
        })
    }

    /// Hands a finished draft to the outbox, to go at `send_at` (Unix seconds). Until then it can
    /// be called back. It stops being the server's draft now: the row leaves the server's copy
    /// behind, or a change there could put it back in Drafts.
    pub fn queue_send(&self, message_id: i64, send_at: i64) -> Result<()> {
        let local = format!("local:{}", random_hex()?);
        self.with_tx(|tx| {
            let (account, email): (i64, String) = tx.query_row(
                "SELECT m.account_id, a.email FROM message m JOIN account a ON a.id = m.account_id WHERE m.id = ?1",
                [message_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?;
            let mid = new_mid(&email)?;
            let changed = tx.execute(
                "UPDATE draft SET state = 'queued', send_at = ?2 WHERE message_id = ?1 AND state = 'editing'",
                params![message_id, send_at],
            )?;
            if changed == 0 {
                return Err(Error::Refused("this message can no longer be sent".into()));
            }
            tx.execute("UPDATE message SET remote_id = ?2, mid = ?3 WHERE id = ?1", params![message_id, local, mid])?;
            tx.execute("DELETE FROM placement WHERE message_id = ?1", [message_id])?;
            tx.execute("DELETE FROM pending_op WHERE message_id = ?1 AND action = ?2", params![message_id, "\"SaveDraft\""])?;
            set_only_label(tx, account, message_id, Role::Outbox)
        })
    }

    /// Calls mail back from the outbox. False if it is too late.
    pub fn cancel_send(&self, message_id: i64) -> Result<bool> {
        self.with_tx(|tx| {
            let changed = tx.execute(
                "UPDATE draft SET state = 'editing' WHERE message_id = ?1 AND state IN ('queued', 'retrying')",
                [message_id],
            )?;
            if changed == 0 {
                return Ok(false);
            }
            let account: i64 = tx.query_row("SELECT account_id FROM message WHERE id = ?1", [message_id], |r| r.get(0))?;
            set_only_label(tx, account, message_id, Role::Drafts)?;
            Ok(true)
        })
    }

    /// Makes a draft safe to edit, calling it back if it is waiting to go. False if it is too
    /// late, which is to say it is on its way this moment.
    pub fn take_back(&self, message_id: i64) -> Result<bool> {
        if self.cancel_send(message_id)? {
            return Ok(true);
        }
        self.with_tx(|tx| {
            Ok(tx.prepare_cached("SELECT 1 FROM draft WHERE message_id = ?1 AND state = 'editing'")?.exists([message_id])?)
        })
    }

    /// The next mail to go and when, for one account.
    pub(crate) fn next_send(&self, account_id: i64) -> Result<Option<(i64, i64)>> {
        self.with_tx(|tx| {
            let mut stmt = tx.prepare_cached(
                "SELECT d.message_id, d.send_at FROM draft d JOIN message m ON m.id = d.message_id
                 WHERE m.account_id = ?1 AND d.state IN ('queued', 'retrying') ORDER BY d.send_at LIMIT 1",
            )?;
            Ok(stmt.query_row([account_id], |r| Ok((r.get(0)?, r.get(1)?))).optional()?)
        })
    }

    /// Commits to sending: from here a crash leaves it unknown whether the mail went, so it must
    /// not go again by itself. That has to hold through a power cut too, so this commit is durable.
    /// `None` if it has been called back. The bytes are the final ones.
    pub(crate) fn begin_send(&self, message_id: i64) -> Result<Option<Prepared>> {
        self.with_durable_tx(|tx| {
            let changed = tx.execute(
                "UPDATE draft SET state = 'sending' WHERE message_id = ?1 AND state IN ('queued', 'retrying')",
                [message_id],
            )?;
            let compose = read_fields(tx, message_id)?;
            let (Some(compose), 1) = (compose, changed) else { return Ok(None) };
            let (account, mid): (i64, String) =
                tx.query_row("SELECT account_id, mid FROM message WHERE id = ?1", [message_id], |r| Ok((r.get(0)?, r.get(1)?)))?;
            let files = read_attachments(tx, message_id)?;
            let (envelope, raw) = compose::outgoing(&compose, &sender(tx, account)?, &mid, &files).map_err(Error::Protocol)?;
            Ok(Some(Prepared { envelope, raw }))
        })
    }

    /// The server has the mail. It becomes Sent mail here, with the exact bytes that went, until
    /// the server's own copy takes over the row.
    pub(crate) fn finish_send(&self, message_id: i64, raw: &[u8]) -> Result<()> {
        let packed = zstd::bulk::compress(raw, 3)?;
        self.with_tx(|tx| {
            let (account, old_mid, body): (i64, Option<String>, String) = tx.query_row(
                "SELECT m.account_id, d.old_mid, d.body FROM draft d JOIN message m ON m.id = d.message_id WHERE d.message_id = ?1",
                [message_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )?;
            tx.execute("INSERT OR REPLACE INTO bodies.body (message_id, raw) VALUES (?1, ?2)", params![message_id, packed])?;
            tx.execute("DELETE FROM bodies.attachment WHERE message_id = ?1", [message_id])?;
            tx.execute("DELETE FROM draft WHERE message_id = ?1", [message_id])?;
            tx.execute("UPDATE message SET size = ?2, date = ?3 WHERE id = ?1", params![message_id, raw.len() as i64, chrono::Utc::now().timestamp()])?;
            set_only_label(tx, account, message_id, Role::Sent)?;
            index_message(tx, message_id, &body)?;
            if let Some(mid) = old_mid {
                enqueue(tx, account, message_id, &Action::DiscardDraft { mid }, None)?;
            }
            // Gmail files sent mail itself, and a second copy would be a duplicate.
            if !is_gmail(tx, account)? {
                enqueue(tx, account, message_id, &Action::FileSent, None)?;
            }
            Ok(())
        })
    }

    /// Tries a waiting message at the next chance, for when the connection is back.
    pub fn retry_now(&self, message_id: i64) -> Result<()> {
        self.retry_send(message_id, 0)
    }

    /// Leaves mail as a crash mid-send would, for tests of what happens next.
    #[doc(hidden)]
    pub fn simulate_crash_during_send(&self, message_id: i64) -> Result<()> {
        self.begin_send(message_id).map(drop)
    }

    /// Puts mail back in the queue, to try again at `send_at`.
    pub(crate) fn retry_send(&self, message_id: i64, send_at: i64) -> Result<()> {
        self.with_tx(|tx| {
            tx.execute("UPDATE draft SET state = 'retrying', send_at = ?2 WHERE message_id = ?1", params![message_id, send_at])?;
            Ok(())
        })
    }

    /// Returns mail that did not go to Drafts, where its author can see it.
    pub(crate) fn fail_send(&self, message_id: i64) -> Result<()> {
        self.with_tx(|tx| {
            tx.execute("UPDATE draft SET state = 'editing' WHERE message_id = ?1", [message_id])?;
            let account: i64 = tx.query_row("SELECT account_id FROM message WHERE id = ?1", [message_id], |r| r.get(0))?;
            set_only_label(tx, account, message_id, Role::Drafts)
        })
    }

    /// Mail that was going when the app stopped. Nobody can say whether it went, so it returns
    /// to Drafts rather than risk sending it twice.
    pub(crate) fn interrupted_sends(&self, account_id: i64) -> Result<Vec<i64>> {
        self.with_tx(|tx| {
            let mut stmt = tx.prepare_cached(
                "SELECT d.message_id FROM draft d JOIN message m ON m.id = d.message_id
                 WHERE m.account_id = ?1 AND d.state = 'sending'",
            )?;
            let ids = stmt.query_map([account_id], |r| r.get(0))?;
            Ok(ids.collect::<rusqlite::Result<_>>()?)
        })
    }

    /// Drafts left unsaved on the server by a crash, for which an upload is queued now.
    pub fn resume_drafts(&self, account_id: i64) -> Result<()> {
        let unsaved: Vec<i64> = self.with_tx(|tx| {
            let mut stmt = tx.prepare_cached(
                "SELECT d.message_id FROM draft d JOIN message m ON m.id = d.message_id
                 WHERE m.account_id = ?1 AND d.state = 'editing' AND m.mid IS NULL",
            )?;
            let ids = stmt.query_map([account_id], |r| r.get(0))?;
            Ok(ids.collect::<rusqlite::Result<_>>()?)
        })?;
        for id in unsaved {
            self.upload_draft(id)?;
        }
        Ok(())
    }

    fn uploaded(&self, message_id: i64, mid: &str) -> Result<()> {
        self.with_tx(|tx| {
            tx.execute("UPDATE draft SET old_mid = ?2 WHERE message_id = ?1", params![message_id, mid])?;
            Ok(())
        })
    }

    fn upload_job(&self, message_id: i64) -> Result<Option<UploadJob>> {
        self.with_tx(|tx| {
            let row: Option<(String, Option<String>, Vec<u8>)> = tx
                .query_row(
                    "SELECT m.mid, d.old_mid, b.raw FROM draft d
                     JOIN message m ON m.id = d.message_id
                     JOIN bodies.body b ON b.message_id = d.message_id
                     WHERE d.message_id = ?1 AND d.state = 'editing'",
                    [message_id],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .optional()?;
            let Some((mid, old_mid, packed)) = row else { return Ok(None) };
            Ok(Some(UploadJob { mid, raw: zstd::decode_all(packed.as_slice())?, old_mid }))
        })
    }

    fn raw_body(&self, message_id: i64) -> Result<Option<Vec<u8>>> {
        self.with_tx(|tx| {
            let packed: Option<Vec<u8>> = tx
                .query_row("SELECT raw FROM bodies.body WHERE message_id = ?1", [message_id], |r| r.get(0))
                .optional()?;
            Ok(packed.map(|p| zstd::decode_all(p.as_slice())).transpose()?)
        })
    }
}

/// What an upload needs.
struct UploadJob {
    mid: String,
    raw: Vec<u8>,
    /// The copy on the server that this one replaces.
    old_mid: Option<String>,
}

/// Mail on its way out, ready for the server.
pub(crate) struct Prepared {
    pub envelope: Envelope,
    pub raw: Vec<u8>,
}

/// The server has this message as sent mail, so if it is still a draft here, it went after all:
/// the connection died after handing it over, or the app stopped. Nothing is left to edit.
pub(crate) fn finished_elsewhere(tx: &Transaction, account_id: i64, message_id: i64) -> Result<()> {
    let mut stmt = tx.prepare_cached("SELECT old_mid FROM draft WHERE message_id = ?1")?;
    let Some(old_mid) = stmt.query_row([message_id], |r| r.get::<_, Option<String>>(0)).optional()? else {
        return Ok(());
    };
    tx.execute("DELETE FROM bodies.attachment WHERE message_id = ?1", [message_id])?;
    tx.execute("DELETE FROM draft WHERE message_id = ?1", [message_id])?;
    if let Some(mid) = old_mid {
        enqueue(tx, account_id, message_id, &Action::DiscardDraft { mid }, None)?;
    }
    Ok(())
}

fn insert_attachment(tx: &Transaction, message_id: i64, file: &Attachment) -> Result<()> {
    tx.execute(
        "INSERT INTO bodies.attachment (message_id, position, name, mime, data)
         SELECT ?1, coalesce(max(position), -1) + 1, ?2, ?3, ?4 FROM bodies.attachment WHERE message_id = ?1",
        params![message_id, file.name, file.mime, file.data],
    )?;
    Ok(())
}

fn forget_message(tx: &Transaction, message_id: i64) -> Result<()> {
    tx.execute("DELETE FROM bodies.body WHERE message_id = ?1", [message_id])?;
    tx.execute("DELETE FROM bodies.attachment WHERE message_id = ?1", [message_id])?;
    tx.execute("DELETE FROM pending_op WHERE message_id = ?1", [message_id])?;
    tx.execute("DELETE FROM message WHERE id = ?1", [message_id])?;
    Ok(())
}

impl Reader {
    /// A draft's fields, or `None` if the message is not one yet.
    pub fn draft(&self, message_id: i64) -> Result<Option<Compose>> {
        read_fields(self.conn(), message_id)
    }

    pub fn draft_attachments(&self, message_id: i64) -> Result<Vec<AttachmentInfo>> {
        let mut stmt = self
            .conn()
            .prepare_cached("SELECT name, length(data) FROM bodies.attachment WHERE message_id = ?1 ORDER BY position")?;
        let files = stmt.query_map([message_id], |r| Ok(AttachmentInfo { name: r.get(0)?, size: r.get::<_, i64>(1)? as usize }))?;
        Ok(files.collect::<rusqlite::Result<_>>()?)
    }
}

/// A folder by role, as the server lists it now. Gmail's Drafts is not one we sync.
async fn folder_with_role(imap: &mut Imap, role: Role) -> Result<Option<String>> {
    Ok(imap.list().await?.into_iter().find(|f| f.role == role).map(|f| f.path))
}

/// Puts a draft on the server. The new copy goes up before the old one comes down, so at no
/// moment is it nowhere. If a crash repeats this, the extra copy goes too.
pub(crate) async fn upload(imap: &mut Imap, store: &Store, message_id: i64) -> Result<()> {
    let Some(UploadJob { mid, raw, old_mid }) = store.upload_job(message_id)? else { return Ok(()) };
    let drafts = folder_with_role(imap, Role::Drafts).await?.ok_or_else(|| Error::Refused("no Drafts folder".into()))?;
    imap.append(&drafts, "(\\Draft \\Seen)", &raw).await?;
    discard_copies(imap, &mid, Some(1)).await?;
    if let Some(old) = old_mid.filter(|old| *old != mid) {
        discard_copies(imap, &old, None).await?;
    }
    store.uploaded(message_id, &mid)
}

/// Deletes the copies of a draft from the Drafts folder, keeping the newest `keep`.
///
/// Expunging from Gmail's Drafts only takes the label off, which would leave the old version in
/// All Mail. A message is only really gone from Gmail once it is expunged from Trash.
pub(crate) async fn discard_copies(imap: &mut Imap, mid: &str, keep: Option<usize>) -> Result<()> {
    let Some(drafts) = folder_with_role(imap, Role::Drafts).await? else { return Ok(()) };
    imap.select(&drafts).await?;
    let mut uids = imap.find_message_id(mid).await?;
    uids.sort_unstable();
    uids.truncate(uids.len().saturating_sub(keep.unwrap_or(0)));
    if uids.is_empty() {
        return Ok(());
    }
    if !imap.gmail {
        return imap.delete(&uids).await;
    }
    let trash = folder_with_role(imap, Role::Trash).await?.ok_or_else(|| Error::Refused("no Trash folder".into()))?;
    let mut moved = Vec::new();
    for uid in uids {
        moved.extend(imap.move_to(uid, &trash).await?);
    }
    imap.select(&trash).await?;
    imap.delete(&moved).await
}

/// Files a sent message in Sent, marked read as sent mail should be.
pub(crate) async fn file_sent(imap: &mut Imap, store: &Store, message_id: i64) -> Result<()> {
    let Some(raw) = store.raw_body(message_id)? else { return Ok(()) };
    let sent = folder_with_role(imap, Role::Sent).await?.ok_or_else(|| Error::Refused("no Sent folder".into()))?;
    let mid = MessageParser::default().parse(&raw).and_then(|m| m.root().message_id().map(String::from));
    if let Some(mid) = mid {
        imap.select(&sent).await?;
        if !imap.find_message_id(&mid).await?.is_empty() {
            return Ok(());
        }
    }
    imap.append(&sent, "(\\Seen)", &raw).await
}
