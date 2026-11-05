//! The operation queue. Every change the user makes lands in the store and in `pending_op` in one
//! transaction, so the UI never waits, and the server hears about it later, in order, even across
//! a restart. Until then sync re-applies pending actions over whatever the server reports, so
//! nothing flickers back. If the server refuses one, its `undo` puts the message back.
//!
//! Actions say what the user meant in terms that fit any provider. How each maps to commands is
//! decided here, at the last moment, by [`replay`].

use std::collections::{HashMap, HashSet};

use rusqlite::{OptionalExtension, Transaction, params};
use serde::{Deserialize, Serialize};

use crate::imap::Imap;
use crate::store::{Role, Store, add_label, label_id, refresh_heads, thread_of};
use crate::{Error, Result};

/// How many tries an action gets. Each round of a first sync is a try, so this has to outlast a
/// rescan of a large folder.
const MAX_ATTEMPTS: i64 = 50;
/// After this many tries an action waits until the pass is over, so that one that keeps failing
/// cannot stop mail coming in.
const STUBBORN_AFTER: i64 = 3;
/// How many messages one command marks read. It keeps the command to a few kilobytes.
const SEEN_BATCH: usize = 1000;

/// What the user asked for, before it is known what that means for a given message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Triage {
    Seen(bool),
    Starred(bool),
    Archive,
    /// Back to the Inbox, from anywhere.
    Inbox,
    Trash,
    Spam,
    /// Adds or removes a label. Gmail only, where a message can have several.
    Label { label: i64, on: bool },
}

/// One change to one message, as stored in the queue.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Action {
    Seen(bool),
    Starred(bool),
    Label { label: i64, on: bool },
    /// Moves to the folder behind the label `to`, and leaves the message with exactly `labels`.
    Move { to: i64, labels: Vec<i64> },
    /// Uploads a draft, replacing any copy on the server.
    SaveDraft,
    /// Deletes the server's copy of a draft that is gone or has been sent.
    DiscardDraft { mid: String },
    /// Files sent mail in Sent, on servers that do not do that themselves.
    FileSent,
}

/// What it takes to reverse a [`Store::act`].
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Undo(Vec<(i64, Action)>);

impl Undo {
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// A queued action.
#[derive(Debug, Clone)]
pub struct Op {
    pub id: i64,
    pub message_id: i64,
    pub action: Action,
    pub undo: Option<Action>,
    pub attempts: i64,
}

fn to_json(action: &Action) -> Result<String> {
    serde_json::to_string(action).map_err(|e| Error::Protocol(e.to_string()))
}

fn from_json(text: &str) -> rusqlite::Result<Action> {
    serde_json::from_str(text).map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))
}

fn labels_of(tx: &Transaction, message_id: i64) -> Result<Vec<i64>> {
    let mut stmt = tx.prepare_cached("SELECT label_id FROM message_label WHERE message_id = ?1")?;
    let labels = stmt.query_map([message_id], |r| r.get(0))?;
    Ok(labels.collect::<rusqlite::Result<_>>()?)
}

fn label_with_role(tx: &Transaction, account_id: i64, role: Role) -> Result<Option<i64>> {
    let mut stmt = tx.prepare_cached("SELECT id FROM label WHERE account_id = ?1 AND role = ?2")?;
    Ok(stmt.query_row(params![account_id, role.as_str()], |r| r.get(0)).optional()?)
}

/// The folder a message lives in now, as a label, for moving it back there.
fn home(tx: &Transaction, message_id: i64) -> Result<Option<i64>> {
    let mut stmt = tx.prepare_cached(
        "SELECT f.label_id FROM placement p JOIN folder f ON f.id = p.folder_id WHERE p.message_id = ?1 LIMIT 1",
    )?;
    Ok(stmt.query_row([message_id], |r| r.get(0)).optional()?)
}

/// What a request comes to for one message, and how to reverse that. `None` if it changes nothing.
fn resolve(tx: &Transaction, account_id: i64, id: i64, triage: Triage) -> Result<Option<(Action, Action)>> {
    let (seen, flagged): (bool, bool) =
        tx.query_row("SELECT seen, flagged FROM message WHERE id = ?1", [id], |r| Ok((r.get(0)?, r.get(1)?)))?;
    let labels = labels_of(tx, id)?;
    let role = |role| label_with_role(tx, account_id, role);
    let inbox = role(Role::Inbox)?;
    let all = role(Role::All)?;
    let in_inbox = inbox.is_some_and(|l| labels.contains(&l));
    // The place to put it back: labels alone do not say, as a message can have several.
    let origin = || -> Result<Option<i64>> { Ok(if in_inbox { inbox } else { home(tx, id)?.or(all) }) };
    let moved = |to: i64, kept: Vec<i64>| -> Result<Option<(Action, Action)>> {
        let Some(back) = origin()? else { return Ok(None) };
        Ok(Some((Action::Move { to, labels: kept }, Action::Move { to: back, labels: labels.clone() })))
    };
    let label = |label: i64, on: bool| {
        (labels.contains(&label) != on).then_some((Action::Label { label, on }, Action::Label { label, on: !on }))
    };
    Ok(match triage {
        Triage::Seen(on) => (seen != on).then_some((Action::Seen(on), Action::Seen(!on))),
        Triage::Starred(on) => (flagged != on).then_some((Action::Starred(on), Action::Starred(!on))),
        Triage::Label { label: id, on } => label(id, on),
        Triage::Archive => match (inbox.filter(|_| in_inbox), all, role(Role::Archive)?) {
            (None, ..) => None,
            (Some(inbox), Some(_), _) => label(inbox, false),
            (Some(inbox), None, Some(archive)) => {
                let kept = labels.iter().copied().filter(|l| *l != inbox).chain([archive]).collect();
                moved(archive, kept)?
            }
            (Some(_), None, None) => None,
        },
        Triage::Trash | Triage::Spam => {
            let bin = if triage == Triage::Trash { Role::Trash } else { Role::Spam };
            match role(bin)? {
                Some(to) if !labels.contains(&to) => moved(to, vec![to])?,
                _ => None,
            }
        }
        Triage::Inbox => {
            let Some(inbox) = inbox.filter(|_| !in_inbox) else { return Ok(None) };
            let binned = [Role::Trash, Role::Spam, Role::Archive]
                .into_iter()
                .filter_map(|r| role(r).transpose())
                .collect::<Result<Vec<i64>>>()?;
            let out_of_circulation = labels.iter().any(|l| binned.contains(l));
            if all.is_some() && !out_of_circulation {
                label(inbox, true)
            } else {
                let kept = labels.iter().copied().filter(|l| !binned.contains(l)).chain([inbox]).chain(all).collect();
                moved(inbox, kept)?
            }
        }
    })
}

/// Makes an action true locally. It runs when the action is queued, and again after any sync
/// that wrote server state over a message the server has not caught up on.
fn apply(tx: &Transaction, message_id: i64, action: &Action) -> Result<()> {
    match action {
        Action::Seen(on) => {
            tx.execute("UPDATE message SET seen = ?2 WHERE id = ?1", params![message_id, on])?;
        }
        Action::Starred(on) => {
            tx.execute("UPDATE message SET flagged = ?2 WHERE id = ?1", params![message_id, on])?;
            let account: i64 = tx.query_row("SELECT account_id FROM message WHERE id = ?1", [message_id], |r| r.get(0))?;
            let starred = label_id(tx, account, &crate::store::LabelSpec::system(Role::Starred))?;
            set_label(tx, message_id, starred, *on)?;
        }
        Action::Label { label, on } => set_label(tx, message_id, *label, *on)?,
        Action::Move { labels, .. } => {
            tx.execute("DELETE FROM message_label WHERE message_id = ?1", [message_id])?;
            for label in labels {
                add_label(tx, *label, message_id)?;
            }
        }
        Action::SaveDraft | Action::DiscardDraft { .. } | Action::FileSent => {}
    }
    Ok(())
}

fn set_label(tx: &Transaction, message_id: i64, label: i64, on: bool) -> Result<()> {
    if on {
        add_label(tx, label, message_id)
    } else {
        tx.execute("DELETE FROM message_label WHERE label_id = ?1 AND message_id = ?2", params![label, message_id])?;
        Ok(())
    }
}

/// Puts back what the queue still owes a message, after sync wrote the server's view over it.
pub(crate) fn reapply_pending(tx: &Transaction, message_id: i64) -> Result<()> {
    let mut stmt = tx.prepare_cached("SELECT action FROM pending_op WHERE message_id = ?1 ORDER BY id")?;
    let actions: Vec<String> = stmt.query_map([message_id], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
    for action in actions {
        apply(tx, message_id, &from_json(&action)?)?;
    }
    Ok(())
}

/// Adds work for the server to the queue, with no local effect of its own.
pub(crate) fn enqueue(tx: &Transaction, account_id: i64, message_id: i64, action: &Action, undo: Option<&Action>) -> Result<()> {
    tx.execute(
        "INSERT INTO pending_op (account_id, message_id, action, undo) VALUES (?1, ?2, ?3, ?4)",
        params![account_id, message_id, to_json(action)?, undo.map(to_json).transpose()?],
    )?;
    Ok(())
}

impl Store {
    /// Applies a request to these messages at once and queues it for the server. Call
    /// [`crate::engine::Engine::flush`] after. Returns how to reverse it.
    pub fn act(&self, account_id: i64, messages: &[i64], triage: Triage) -> Result<Undo> {
        self.with_tx(|tx| {
            let mut undo = Vec::new();
            let mut threads = HashSet::new();
            for &id in messages {
                let Some((action, reverse)) = resolve(tx, account_id, id, triage)? else { continue };
                apply(tx, id, &action)?;
                enqueue(tx, account_id, id, &action, Some(&reverse))?;
                threads.insert(thread_of(tx, id)?);
                undo.push((id, reverse));
            }
            refresh_heads(tx, account_id, threads)?;
            Ok(Undo(undo))
        })
    }

    /// Reverses an earlier [`Store::act`]. The server hears both, which leaves it where it began.
    pub fn undo(&self, account_id: i64, undo: &Undo) -> Result<()> {
        self.with_tx(|tx| {
            let mut threads = HashSet::new();
            for (id, action) in &undo.0 {
                apply(tx, *id, action)?;
                enqueue(tx, account_id, *id, action, None)?;
                threads.insert(thread_of(tx, *id)?);
            }
            refresh_heads(tx, account_id, threads)
        })
    }

    pub(crate) fn pending_ops(&self, account_id: i64) -> Result<Vec<Op>> {
        self.with_tx(|tx| {
            let mut stmt = tx.prepare_cached(
                "SELECT id, message_id, action, undo, attempts FROM pending_op WHERE account_id = ?1 ORDER BY id",
            )?;
            let ops = stmt.query_map([account_id], |r| {
                Ok(Op {
                    id: r.get(0)?,
                    message_id: r.get(1)?,
                    action: from_json(&r.get::<_, String>(2)?)?,
                    undo: r.get::<_, Option<String>>(3)?.as_deref().map(from_json).transpose()?,
                    attempts: r.get(4)?,
                })
            })?;
            Ok(ops.collect::<rusqlite::Result<_>>()?)
        })
    }

    /// How many actions the server has yet to hear.
    pub fn pending_count(&self, account_id: i64) -> Result<usize> {
        self.with_tx(|tx| {
            let count: i64 = tx.query_row("SELECT count(*) FROM pending_op WHERE account_id = ?1", [account_id], |r| r.get(0))?;
            Ok(count as usize)
        })
    }

    fn finish_op(&self, id: i64) -> Result<()> {
        self.with_tx(|tx| {
            tx.execute("DELETE FROM pending_op WHERE id = ?1", [id])?;
            Ok(())
        })
    }

    fn finish_ops(&self, ids: impl Iterator<Item = i64>) -> Result<()> {
        self.with_tx(|tx| {
            let mut delete = tx.prepare_cached("DELETE FROM pending_op WHERE id = ?1")?;
            for id in ids {
                delete.execute([id])?;
            }
            Ok(())
        })
    }

    fn retry_op(&self, id: i64) -> Result<()> {
        self.with_tx(|tx| {
            tx.execute("UPDATE pending_op SET attempts = attempts + 1 WHERE id = ?1", [id])?;
            Ok(())
        })
    }

    /// Gives up on an action and returns the message to how it was.
    fn fail_op(&self, account_id: i64, op: &Op) -> Result<()> {
        self.with_tx(|tx| {
            tx.execute("DELETE FROM pending_op WHERE id = ?1", [op.id])?;
            let exists = tx.prepare_cached("SELECT 1 FROM message WHERE id = ?1")?.exists([op.message_id])?;
            if let (true, Some(undo)) = (exists, &op.undo) {
                apply(tx, op.message_id, undo)?;
                reapply_pending(tx, op.message_id)?;
                refresh_heads(tx, account_id, [thread_of(tx, op.message_id)?])?;
            }
            Ok(())
        })
    }

    /// Where a message is on the server, as of the last sync.
    fn placed(&self, message_id: i64) -> Result<Option<Placed>> {
        self.with_tx(|tx| {
            let mut stmt = tx.prepare_cached(
                "SELECT f.path, f.id, p.uid, f.uid_validity FROM placement p JOIN folder f ON f.id = p.folder_id
                 WHERE p.message_id = ?1 AND p.uid IS NOT NULL LIMIT 1",
            )?;
            let placed = stmt.query_row([message_id], |r| {
                Ok(Placed { path: r.get(0)?, folder: r.get(1)?, uid: r.get(2)?, validity: r.get(3)? })
            });
            Ok(placed.optional()?)
        })
    }

    /// The server path behind a label, with the folder's id if we sync it. On Gmail the Inbox is
    /// a label, but moving to the folder of that name is how it gets one.
    fn target(&self, label: i64) -> Result<Option<(String, Option<i64>)>> {
        self.with_tx(|tx| {
            let mut stmt = tx.prepare_cached("SELECT path, id FROM folder WHERE label_id = ?1")?;
            let folder = stmt.query_row([label], |r| Ok((r.get(0)?, Some(r.get(1)?)))).optional()?;
            let role: String = tx.query_row("SELECT role FROM label WHERE id = ?1", [label], |r| r.get(0))?;
            Ok(folder.or_else(|| (Role::parse(&role) == Role::Inbox).then(|| ("INBOX".to_string(), None))))
        })
    }

    /// Records where a move left a message. If the target is not a folder we sync, it has no
    /// place at all until sync finds it again, and its labels keep it from being purged.
    fn moved(&self, message_id: i64, from: i64, to: Option<(i64, u32)>) -> Result<()> {
        self.with_tx(|tx| {
            tx.execute("DELETE FROM placement WHERE folder_id = ?1 AND message_id = ?2", params![from, message_id])?;
            if let Some((folder, uid)) = to {
                tx.execute("UPDATE placement SET uid = NULL WHERE folder_id = ?1 AND uid = ?2", params![folder, uid])?;
                tx.execute(
                    "INSERT INTO placement (folder_id, message_id, uid) VALUES (?1, ?2, ?3)
                     ON CONFLICT (folder_id, message_id) DO UPDATE SET uid = ?3",
                    params![folder, message_id, uid],
                )?;
            }
            Ok(())
        })
    }

    fn label_name(&self, label: i64) -> Result<(String, Role)> {
        self.with_tx(|tx| {
            Ok(tx.query_row("SELECT name, role FROM label WHERE id = ?1", [label], |r| {
                Ok((r.get(0)?, Role::parse(&r.get::<_, String>(1)?)))
            })?)
        })
    }
}

struct Placed {
    path: String,
    folder: i64,
    uid: u32,
    validity: u32,
}

enum Outcome {
    Done,
    /// The message has no known place on the server yet, so another pass may find it.
    Later,
}

/// Plays the queue to the server, oldest first. A message's actions stay in order; one that
/// cannot run yet holds back only those of the same message. A lost connection ends the replay
/// with an error and leaves the rest for next time. Returns how many actions were given up on.
///
/// `stubborn` picks which actions run: those that have failed repeatedly go after the pass, the
/// rest before it, so that a pass always gets as far as fetching mail.
pub async fn replay(imap: &mut Imap, store: &Store, account_id: i64, stubborn: bool) -> Result<usize> {
    if !stubborn {
        replay_seen(imap, store, account_id).await?;
    }
    let mut held: HashSet<i64> = HashSet::new();
    let mut refused = 0;
    for op in store.pending_ops(account_id)? {
        if held.contains(&op.message_id) {
            continue;
        }
        if (op.attempts >= STUBBORN_AFTER) != stubborn {
            held.insert(op.message_id);
            continue;
        }
        match run(imap, store, &op).await {
            Ok(Outcome::Done) => store.finish_op(op.id)?,
            Ok(Outcome::Later) if op.attempts + 1 < MAX_ATTEMPTS => {
                store.retry_op(op.id)?;
                held.insert(op.message_id);
            }
            Ok(Outcome::Later) | Err(Error::Refused(_)) => {
                store.fail_op(account_id, &op)?;
                refused += 1;
            }
            Err(e) if op.attempts + 1 < MAX_ATTEMPTS => {
                store.retry_op(op.id)?;
                return Err(e);
            }
            Err(_) => {
                store.fail_op(account_id, &op)?;
                refused += 1;
            }
        }
    }
    Ok(refused)
}

/// Sends read and unread in bulk, as marking a whole folder one message at a time would keep new
/// mail waiting for minutes. This only takes the plain case: whatever goes wrong here stays in
/// the queue for [`replay`], which knows what to do about each message.
async fn replay_seen(imap: &mut Imap, store: &Store, account_id: i64) -> Result<()> {
    loop {
        let mut reached = HashSet::new();
        let mut batches: HashMap<(String, u32, bool), Vec<(i64, u32)>> = HashMap::new();
        for op in store.pending_ops(account_id)? {
            // A message's actions keep their order, so only its first can go ahead.
            if reached.insert(op.message_id)
                && op.attempts == 0
                && let Action::Seen(on) = op.action
                && let Some(placed) = store.placed(op.message_id)?
            {
                batches.entry((placed.path, placed.validity, on)).or_default().push((op.id, placed.uid));
            }
        }
        if batches.is_empty() {
            return Ok(());
        }
        for ((path, validity, on), ops) in batches {
            if imap.select(&path).await.ok() != Some(validity) {
                return Ok(());
            }
            for batch in ops.chunks(SEEN_BATCH) {
                let uids: Vec<u32> = batch.iter().map(|(_, uid)| *uid).collect();
                if imap.store_flag(&uids, "\\Seen", on).await.is_err() {
                    return Ok(());
                }
                store.finish_ops(batch.iter().map(|(id, _)| *id))?;
            }
        }
    }
}

async fn run(imap: &mut Imap, store: &Store, op: &Op) -> Result<Outcome> {
    match &op.action {
        Action::SaveDraft => return crate::draft::upload(imap, store, op.message_id).await.map(|()| Outcome::Done),
        Action::DiscardDraft { mid } => return crate::draft::discard_copies(imap, mid, None).await.map(|()| Outcome::Done),
        Action::FileSent => return crate::draft::file_sent(imap, store, op.message_id).await.map(|()| Outcome::Done),
        _ => {}
    }
    let Some(Placed { path, folder, uid, validity }) = store.placed(op.message_id)? else { return Ok(Outcome::Later) };
    // The folder was renumbered since we looked, so this UID may be another message entirely.
    // Sync sorts that out, and the action waits for it.
    if imap.select(&path).await? != validity {
        return Ok(Outcome::Later);
    }
    match &op.action {
        Action::Seen(on) => imap.store_flag(&[uid], "\\Seen", *on).await?,
        Action::Starred(on) => imap.store_flag(&[uid], "\\Flagged", *on).await?,
        Action::Label { label, on } => {
            let (name, role) = store.label_name(*label)?;
            imap.store_label(uid, &gmail_label(&name, role), *on).await?;
        }
        Action::Move { to, .. } => {
            let Some((target, target_folder)) = store.target(*to)? else {
                return Err(Error::Refused(format!("no folder for label {to}")));
            };
            if target != path {
                let new_uid = imap.move_to(uid, &target).await?;
                store.moved(op.message_id, folder, target_folder.zip(new_uid))?;
            }
        }
        Action::SaveDraft | Action::DiscardDraft { .. } | Action::FileSent => unreachable!("handled above"),
    }
    Ok(Outcome::Done)
}

/// A label as X-GM-LABELS spells it.
fn gmail_label(name: &str, role: Role) -> String {
    match role {
        Role::Inbox => "\\Inbox".into(),
        Role::Sent => "\\Sent".into(),
        Role::Drafts => "\\Draft".into(),
        _ => name.into(),
    }
}
