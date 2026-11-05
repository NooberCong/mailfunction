//! One pass of the sync loop for one folder (RFC 4549 order, CONDSTORE deltas). Local actions go
//! first, so that what comes back already includes them.
//!
//! - New mail: everything from `next_uid` upward.
//! - Flag and label changes: CHANGEDSINCE the stored modseq.
//! - Expunges: CONDSTORE cannot see them, so the local UID count is compared with EXISTS and
//!   a UID diff runs only when they disagree.
//! - History: backfill walks down from the top in batches, so the newest mail shows first and
//!   an interrupted sync resumes where it stopped.
//! - UIDVALIDITY change: messages stay visible while the folder is rescanned; only what the
//!   finished rescan did not find is deleted.

use std::collections::HashSet;

use mail_parser::MessageParser;
use sha2::{Digest, Sha256};

use crate::engine::Event;
use crate::imap::{Fetched, Imap, RemoteFolder};
use crate::ops::replay;
use crate::store::{Change, Folder, LabelSpec, NewMessage, Role, Store};
use crate::{Error, Result};

/// UIDs covered by one backfill request. Sparse folders simply return fewer messages.
const BACKFILL_SPAN: u32 = 1000;

/// The folders to mirror. On Gmail every message is in All Mail, or in Spam or Trash, and
/// labels say the rest, so syncing the label folders too would only fetch duplicates.
pub fn folders_to_sync(folders: Vec<RemoteFolder>, gmail: bool) -> Vec<RemoteFolder> {
    let mut folders: Vec<_> = if gmail {
        folders.into_iter().filter(|f| matches!(f.role, Role::All | Role::Spam | Role::Trash)).collect()
    } else {
        folders
    };
    // The folder that matters most goes first, because it is also the one IDLE watches.
    folders.sort_by_key(|f| !matches!(f.role, Role::All | Role::Inbox));
    if gmail && !folders.iter().any(|f| f.role == Role::All) {
        // "Show in IMAP" can be turned off per label in Gmail settings.
        folders.clear();
    }
    folders
}

pub fn label_of(folder: &RemoteFolder) -> LabelSpec {
    match folder.role {
        Role::None => LabelSpec::user(&folder.name),
        role => LabelSpec::system(role),
    }
}

/// Maps X-GM-LABELS to labels. Starred comes from the \Flagged flag on every provider, and
/// \Important is a ranking signal rather than a place people go.
fn gmail_labels(labels: &[String]) -> Vec<LabelSpec> {
    labels
        .iter()
        .filter_map(|label| match label.as_str() {
            "\\Inbox" => Some(LabelSpec::system(Role::Inbox)),
            "\\Sent" => Some(LabelSpec::system(Role::Sent)),
            "\\Draft" => Some(LabelSpec::system(Role::Drafts)),
            "\\Starred" | "\\Important" | "\\Muted" => None,
            other if other.starts_with('\\') => None,
            other => Some(LabelSpec::user(other)),
        })
        .collect()
}

fn new_message(fetched: Fetched) -> NewMessage {
    let header = fetched.data.unwrap_or_default();
    let parsed = MessageParser::default().parse_headers(&header);
    let parsed = parsed.as_ref();
    let subject = parsed.and_then(|m| m.subject()).unwrap_or_default().to_string();
    let sender = parsed.and_then(|m| m.from()).and_then(|list| list.first());
    let from_name = sender.as_ref().and_then(|s| s.name()).unwrap_or_default().to_string();
    let from_addr = sender.as_ref().and_then(|s| s.address()).unwrap_or_default().to_string();
    let mid = parsed.and_then(|m| m.message_id()).map(str::to_string);
    let references = parsed.map_or_else(Vec::new, |m| {
        let replied_to = m.in_reply_to().into_iter().flat_map(|l| l.iter().collect::<Vec<_>>());
        m.references().into_iter().flat_map(|l| l.iter().collect::<Vec<_>>()).chain(replied_to).map(String::from).collect()
    });
    let remote_id = match (fetched.gm_message_id, mid.as_deref()) {
        (Some(id), _) => format!("gm:{id}"),
        // Two copies with one Message-ID in a folder collapse into one placement here. Gmail
        // cannot hit this; generic IMAP (iCloud, Phase 2) must key copies by UID as well.
        (None, Some(id)) => format!("mid:{id}"),
        // No Message-ID at all: identical content is the best identity left.
        (None, None) => {
            let digest = Sha256::digest(&header);
            let hex: String = digest[..16].iter().map(|b| format!("{b:02x}")).collect();
            format!("hash:{hex}:{}:{}", fetched.date, fetched.size)
        }
    };
    NewMessage {
        uid: fetched.uid,
        remote_id,
        gm_thread_id: fetched.gm_thread_id,
        mid,
        references,
        subject,
        from_name,
        from_addr,
        date: fetched.date,
        size: fetched.size,
        seen: fetched.seen,
        flagged: fetched.flagged,
        labels: fetched.labels.as_deref().map(gmail_labels).unwrap_or_default(),
    }
}

fn change(fetched: Fetched) -> Change {
    Change {
        uid: fetched.uid,
        seen: fetched.seen,
        flagged: fetched.flagged,
        labels: fetched.labels.as_deref().map(gmail_labels),
    }
}

pub struct Outcome {
    pub changed: bool,
    pub more_history: bool,
    /// Messages that came in since the last sync. Backfilled history is not among them.
    pub arrived: Vec<i64>,
}

/// Brings one folder up to date, apart from history older than one backfill step.
/// `more_history` tells the caller to come back for the next step.
pub async fn sync_folder(imap: &mut Imap, store: &Store, folder: &mut Folder) -> Result<Outcome> {
    let status = imap.examine(&folder.path).await?;
    if status.uid_validity == 0 || status.uid_next == 0 {
        return Err(Error::Protocol(format!("{} reported no UIDVALIDITY or UIDNEXT", folder.path)));
    }
    let mut changed = false;
    let mut arrived = Vec::new();

    if folder.uid_validity != status.uid_validity {
        if folder.uid_validity != 0 {
            store.forget_uids(folder)?;
            changed = true;
        }
        folder.uid_validity = status.uid_validity;
        folder.next_uid = status.uid_next;
        folder.backfill_uid = status.uid_next;
        folder.modseq = status.highest_modseq;
        // A folder recreated empty has nothing to rescan, so its old messages are gone now.
        if folder.backfilled() {
            store.remove_unmatched(folder)?;
        }
        store.save_progress(folder)?;
    }

    if status.uid_next > folder.next_uid {
        let fresh = imap.fetch_headers(folder.next_uid, None).await?;
        changed |= !fresh.is_empty();
        let messages: Vec<_> = fresh.into_iter().map(new_message).collect();
        arrived = store.save_messages(folder, &messages)?;
        folder.next_uid = status.uid_next;
        store.save_progress(folder)?;
    }

    if status.highest_modseq > folder.modseq {
        let changes: Vec<_> = imap.fetch_changes(folder.modseq).await?.into_iter().map(change).collect();
        changed |= !changes.is_empty();
        store.apply_changes(folder, &changes)?;
        // Only now: a crash above must repeat the delta, not lose it (RFC 7162 §6).
        folder.modseq = status.highest_modseq;
        store.save_progress(folder)?;
    }

    if folder.backfilled() {
        if store.uid_count(folder)? != status.exists {
            changed |= remove_expunged(imap, store, folder).await?;
        }
        return Ok(Outcome { changed, more_history: false, arrived });
    }

    let low = folder.backfill_uid.saturating_sub(BACKFILL_SPAN).max(1);
    let batch = imap.fetch_headers(low, Some(folder.backfill_uid - 1)).await?;
    changed |= !batch.is_empty();
    let messages: Vec<_> = batch.into_iter().map(new_message).collect();
    store.save_messages(folder, &messages)?;
    folder.backfill_uid = low;
    // Before the progress is saved, so that a crash repeats this step instead of skipping it.
    if folder.backfilled() {
        store.remove_unmatched(folder)?;
        changed = true;
    }
    store.save_progress(folder)?;
    Ok(Outcome { changed, more_history: !folder.backfilled(), arrived })
}

/// One complete pass over an account: lists folders, syncs them all including history, then
/// deletes what no folder claimed. Returns the folders, most important first. `on_change` is told
/// what arrived with each change.
pub async fn sync_account(
    imap: &mut Imap,
    store: &Store,
    account_id: i64,
    mut on_event: impl FnMut(Event),
) -> Result<Vec<Folder>> {
    let remote = folders_to_sync(imap.list().await?, imap.gmail);
    if remote.is_empty() {
        return Err(Error::Protocol("no folders to sync; is All Mail shown in IMAP?".into()));
    }
    // On Gmail, Inbox is only a label, but it must lead the sidebar from the first moment.
    store.ensure_label(account_id, &LabelSpec::system(Role::Inbox))?;
    let mut folders =
        remote.iter().map(|f| store.folder(account_id, &f.path, &label_of(f))).collect::<Result<Vec<_>>>()?;

    // History is interleaved one step per folder, so no folder waits behind another's.
    // Actions are replayed before every round, so a long first sync does not hold them up. The
    // round that follows goes over every folder, so a moved message is found before the purge.
    let mut more = true;
    while more {
        more = false;
        if replay(imap, store, account_id, false).await? > 0 {
            on_event(Event::ActionRefused);
        }
        for folder in &mut folders {
            let mut outcome = sync_folder(imap, store, folder).await?;
            if outcome.changed {
                on_event(Event::Changed);
                if !outcome.arrived.is_empty() {
                    on_event(Event::Arrived(std::mem::take(&mut outcome.arrived)));
                }
            }
            more |= outcome.more_history;
        }
    }
    // Only now: a message that left one folder may have turned up in a later one.
    store.purge_orphans(account_id)?;
    // Last of all, because these have been failing, and new mail must not wait on them.
    if replay(imap, store, account_id, true).await? > 0 {
        on_event(Event::ActionRefused);
    }
    Ok(folders)
}

async fn remove_expunged(imap: &mut Imap, store: &Store, folder: &Folder) -> Result<bool> {
    let remote: HashSet<u32> = imap.all_uids().await?.into_iter().collect();
    let gone: Vec<u32> = store.uids(folder)?.difference(&remote).copied().collect();
    store.remove_uids(folder, &gone)?;
    Ok(!gone.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn remote(path: &str, role: Role) -> RemoteFolder {
        RemoteFolder { path: path.into(), name: path.into(), role }
    }

    #[test]
    fn gmail_syncs_only_all_mail_spam_and_trash() {
        let folders = vec![
            remote("INBOX", Role::Inbox),
            remote("Receipts", Role::None),
            remote("[Gmail]/Trash", Role::Trash),
            remote("[Gmail]/All Mail", Role::All),
            remote("[Gmail]/Spam", Role::Spam),
        ];
        let paths: Vec<_> = folders_to_sync(folders, true).into_iter().map(|f| f.path).collect();
        assert_eq!(paths, ["[Gmail]/All Mail", "[Gmail]/Trash", "[Gmail]/Spam"]);
    }

    #[test]
    fn other_servers_sync_everything_inbox_first() {
        let folders = vec![remote("Work", Role::None), remote("INBOX", Role::Inbox)];
        let paths: Vec<_> = folders_to_sync(folders, false).into_iter().map(|f| f.path).collect();
        assert_eq!(paths, ["INBOX", "Work"]);
    }

    #[test]
    fn gmail_labels_map_to_roles_and_drop_signals() {
        let labels: Vec<String> =
            ["\\Inbox", "\\Important", "\\Starred", "\\Sent", "Hóa đơn"].map(String::from).to_vec();
        assert_eq!(
            gmail_labels(&labels),
            vec![LabelSpec::system(Role::Inbox), LabelSpec::system(Role::Sent), LabelSpec::user("Hóa đơn")]
        );
    }

    #[test]
    fn identity_prefers_gmail_id_then_message_id_then_content() {
        let header = b"From: =?UTF-8?Q?Nguy=E1=BB=85n_An?= <an@example.com>\r\nSubject: =?UTF-8?B?Q2jDoG8gYuG6oW4=?=\r\nMessage-ID: <1@x>\r\n\r\n";
        let fetched = |gm| Fetched { uid: 1, gm_message_id: gm, data: Some(header.to_vec()), ..Default::default() };
        let m = new_message(fetched(Some(42)));
        assert_eq!(m.remote_id, "gm:42");
        assert_eq!(m.subject, "Chào bạn");
        assert_eq!(m.from_name, "Nguyễn An");
        assert_eq!(m.from_addr, "an@example.com");
        assert_eq!(new_message(fetched(None)).remote_id, "mid:1@x");

        let bare = Fetched { uid: 1, data: Some(b"Subject: x\r\n\r\n".to_vec()), ..Default::default() };
        assert!(new_message(bare).remote_id.starts_with("hash:"));
    }

    #[test]
    fn a_message_with_unreadable_headers_is_still_kept() {
        let m = new_message(Fetched { uid: 3, ..Default::default() });
        assert_eq!(m.uid, 3);
        assert_eq!(m.subject, "");
    }
}
