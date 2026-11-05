//! The composer: writing, autosaving, attachments, and handing mail to the outbox. What is on
//! screen is saved to the store on every pause, so nothing typed can be lost, and goes to the
//! server when the user leaves it.

use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use mailfunction_core::compose::{self, Attachment, Compose};
use mailfunction_core::draft::MAX_ATTACHMENTS;
use slint::{ModelRc, SharedString, TimerMode, VecModel};

use crate::actions::Reversible;
use crate::{Controller, FileItem, chrome, on_ui_thread};

const AUTOSAVE_DELAY: Duration = Duration::from_millis(800);
/// How long mail waits before it really goes, which is what makes it possible to call it back.
const UNDO_SEND_SECONDS: i64 = 10;
const NOTICE_TIME: Duration = Duration::from_secs(6);
const SUGGESTIONS: usize = 5;
/// Looking through everyone who ever wrote takes too long to do between two keys.
const SUGGEST_DELAY: Duration = Duration::from_millis(120);

/// The message being written.
pub(crate) struct Writing {
    /// Its draft, once there is anything worth keeping.
    pub draft: Option<i64>,
    /// The message it answers, whose thread it joins.
    pub reply_to: Option<i64>,
    /// What it started as: a reply left untouched is not worth keeping.
    pub initial: Compose,
    /// Its files changed, or it is new. Otherwise a draft that was only looked at stays exactly
    /// as it is on the server, which may be richer than anything this composer can write.
    pub touched: bool,
}

/// Bytes as a person would say them.
pub fn human_size(bytes: usize) -> String {
    match bytes {
        0..1024 => format!("{bytes} B"),
        1024..1_048_576 => format!("{} KB", bytes / 1024),
        _ => format!("{:.1} MB", bytes as f64 / 1_048_576.0),
    }
}

/// A file's type by its name. Anything else goes as plain bytes, which every client can save.
fn mime_type(name: &str) -> &'static str {
    let extension = name.rsplit('.').next().unwrap_or_default().to_ascii_lowercase();
    match extension.as_str() {
        "pdf" => "application/pdf",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "txt" => "text/plain",
        "csv" => "text/csv",
        "zip" => "application/zip",
        _ => "application/octet-stream",
    }
}

/// The address being typed in a recipients field: whatever follows the last separator.
fn last_address(field: &str) -> &str {
    field.rsplit([',', ';']).next().unwrap_or(field).trim_start()
}

impl Controller {
    pub(crate) fn compose_new(self: &Rc<Self>) {
        self.open_composer(Compose::default(), None, None, &[]);
    }

    pub(crate) fn reply(self: &Rc<Self>, all: bool) {
        let Some(id) = self.selected.get() else { return };
        let Some(raw) = self.reader.body(id).ok().flatten() else {
            self.show_notice("This message hasn't arrived yet", "", NOTICE_TIME);
            return;
        };
        let own = self.shown_email();
        if let Some(fields) = compose::reply(&raw, &own, all) {
            self.open_composer(fields, None, Some(id), &[]);
        }
    }

    pub(crate) fn forward(self: &Rc<Self>) {
        let Some(id) = self.selected.get() else { return };
        let Some(raw) = self.reader.body(id).ok().flatten() else {
            self.show_notice("This message hasn't arrived yet", "", NOTICE_TIME);
            return;
        };
        if let Some(fields) = compose::forward(&raw) {
            self.open_composer(fields, None, None, &compose::attachments(&raw));
        }
    }

    fn shown_email(&self) -> String {
        let accounts = self.accounts.borrow();
        let shown = accounts.iter().find(|m| Some(m.account.id) == self.shown.get());
        shown.map_or_else(String::new, |m| m.account.email.clone())
    }

    /// Opens a draft, taking it over first if it was written elsewhere. That needs its body,
    /// which may not be here yet: [`Controller::body_ready`] picks this up again if not.
    pub(crate) fn open_draft(self: &Rc<Self>, id: i64) {
        if let Some(fields) = self.reader.draft(id).ok().flatten() {
            if self.store.take_back(id).unwrap_or(false) {
                self.queued();
                self.list.refresh();
                self.open_composer(fields, Some(id), None, &[]);
            } else {
                self.show_notice("This is on its way", "", NOTICE_TIME);
            }
            return;
        }
        match self.reader.body(id).ok().flatten() {
            Some(raw) => {
                if let Some(fields) = self.store.adopt_draft(id, &raw).ok().flatten() {
                    self.open_composer(fields, Some(id), None, &[]);
                }
            }
            None => {
                self.selected.set(Some(id));
                self.opening_draft.set(Some(id));
                self.fetch_selected_body(id);
            }
        }
    }

    fn open_composer(self: &Rc<Self>, fields: Compose, draft: Option<i64>, reply_to: Option<i64>, files: &[Attachment]) {
        self.finish_writing();
        self.pane.hide();
        self.app.set_draft_to(fields.to.as_str().into());
        self.app.set_draft_cc(fields.cc.as_str().into());
        self.app.set_draft_bcc(fields.bcc.as_str().into());
        self.app.set_draft_subject(fields.subject.as_str().into());
        self.app.set_draft_body(fields.body.as_str().into());
        self.app.set_draft_copies(!fields.cc.is_empty() || !fields.bcc.is_empty());
        self.app.set_draft_error("".into());
        self.app.set_composing(true);
        let needs_recipients = fields.to.is_empty();
        let mut draft = draft;
        let mut touched = false;
        // Files have to live somewhere from the first moment.
        if draft.is_none() && !files.is_empty() {
            draft = self.shown.get().and_then(|account| self.store.create_draft(account, &fields, reply_to, files).ok());
            touched = true;
        }
        *self.writing.borrow_mut() = Some(Writing { draft, reply_to, initial: fields, touched });
        self.publish_files();
        if needs_recipients {
            self.app.invoke_focus_recipients();
        } else {
            self.app.invoke_focus_draft_body();
        }
    }

    fn fields(&self) -> Option<Compose> {
        let writing = self.writing.borrow();
        let writing = writing.as_ref()?;
        Some(Compose {
            to: self.app.get_draft_to().to_string(),
            cc: self.app.get_draft_cc().to_string(),
            bcc: self.app.get_draft_bcc().to_string(),
            subject: self.app.get_draft_subject().to_string(),
            body: self.app.get_draft_body().to_string(),
            in_reply_to: writing.initial.in_reply_to.clone(),
            references: writing.initial.references.clone(),
        })
    }

    pub(crate) fn draft_edited(self: &Rc<Self>) {
        self.app.set_draft_error("".into());
        let weak = Rc::downgrade(self);
        self.autosave.start(TimerMode::SingleShot, AUTOSAVE_DELAY, move || {
            if let Some(this) = weak.upgrade() {
                this.save_draft();
            }
        });
    }

    /// Keeps what is on screen, creating the draft the first time there is something to keep.
    pub fn save_draft(&self) -> Option<i64> {
        let fields = self.fields()?;
        let mut writing = self.writing.borrow_mut();
        let writing = writing.as_mut()?;
        let keep = fields != writing.initial && !fields.is_blank();
        let saved = writing.draft.is_some_and(|id| self.store.update_draft(id, &fields).is_ok());
        // With its draft gone from under it, what is on screen starts a new one rather than vanish.
        if !saved && (keep || writing.draft.is_some()) {
            let account = self.shown.get()?;
            writing.draft = self.store.create_draft(account, &fields, writing.reply_to, &[]).ok();
            writing.touched = true;
        }
        writing.draft
    }

    /// Ends writing. A draft goes to the server; one with nothing in it is thrown away.
    pub fn finish_writing(self: &Rc<Self>) {
        let Some(fields) = self.fields() else { return };
        let saved = self.save_draft();
        let changed = self.writing.borrow().as_ref().is_some_and(|w| w.touched || w.initial != fields);
        let empty = saved.is_some_and(|id| fields.is_blank() && self.reader.draft_attachments(id).is_ok_and(|f| f.is_empty()));
        match saved {
            Some(id) if empty => drop(self.store.discard_draft(id)),
            Some(id) if changed => {
                let text = if self.store.upload_draft(id).is_ok() { "Saved to Drafts" } else { "This draft stayed on this computer" };
                self.show_notice(text, "", NOTICE_TIME);
            }
            _ => {}
        }
        self.close_composer();
    }

    fn close_composer(self: &Rc<Self>) {
        self.autosave.stop();
        self.suggest_timer.stop();
        self.writing.borrow_mut().take();
        self.app.set_composing(false);
        self.app.set_suggesting(-1);
        self.queued();
        self.list.refresh();
        // The message that was open comes back, now that nothing is over it.
        match self.selected.get() {
            Some(id) if self.thread.borrow().iter().any(|m| m.id == id) => self.reopen(id),
            _ => self.clear_message(),
        }
        self.app.invoke_focus_list();
    }

    pub(crate) fn discard_draft(self: &Rc<Self>) {
        let draft = self.writing.borrow().as_ref().and_then(|w| w.draft);
        if let Some(id) = draft {
            let _ = self.store.discard_draft(id);
            self.show_notice("Draft discarded", "", NOTICE_TIME);
        }
        self.close_composer();
    }

    /// Hands the message to the outbox. It waits a few seconds there, to be called back.
    pub fn send_draft(self: &Rc<Self>) {
        let Some(fields) = self.fields() else { return };
        if let Some(problem) = compose::problem(&fields) {
            self.app.set_draft_error(problem.into());
            return;
        }
        // Even one that has no changes, like a forward sent as it stands.
        let draft = self.save_draft().or_else(|| {
            let reply_to = self.writing.borrow().as_ref().and_then(|w| w.reply_to);
            self.store.create_draft(self.shown.get()?, &fields, reply_to, &[]).ok()
        });
        let Some(id) = draft else {
            self.app.set_draft_error("This couldn't be saved, so it hasn't been sent".into());
            return;
        };
        let send_at = chrono::Utc::now().timestamp() + UNDO_SEND_SECONDS;
        if self.store.queue_send(id, send_at).is_err() {
            self.app.set_draft_error("This couldn't be saved, so it hasn't been sent".into());
            return;
        }
        *self.reversible.borrow_mut() = Some(Reversible::Send(id));
        self.close_composer();
        self.show_notice("Sending", "Undo", Duration::from_secs(UNDO_SEND_SECONDS as u64));
    }

    pub(crate) fn sent(&self, id: i64) {
        let waiting = matches!(*self.reversible.borrow(), Some(Reversible::Send(message)) if message == id);
        if waiting {
            self.reversible.borrow_mut().take();
        }
        self.show_notice("Sent", "", NOTICE_TIME);
    }

    pub(crate) fn send_failed(&self, maybe: bool, reason: &str) {
        let text = if maybe {
            "This may not have gone, so it's back in Drafts. Check Sent before trying again.".to_string()
        } else {
            format!("Couldn't send, so it's back in Drafts. {reason}")
        };
        self.show_notice(&text, "", Duration::from_secs(12));
    }

    fn publish_files(&self) {
        let draft = self.writing.borrow().as_ref().and_then(|w| w.draft);
        let files = draft.and_then(|id| self.reader.draft_attachments(id).ok()).unwrap_or_default();
        let items: Vec<_> = files
            .into_iter()
            .map(|file| FileItem { name: file.name.into(), size: human_size(file.size).into() })
            .collect();
        self.app.set_draft_files(ModelRc::new(VecModel::from(items)));
    }

    /// Asks which files to attach, without holding up the window.
    pub(crate) fn pick_files(&self) {
        let parent = chrome::dialog_parent(&self.app);
        std::thread::spawn(move || {
            let picked = rfd::FileDialog::new().set_parent(&parent).pick_files().unwrap_or_default();
            on_ui_thread(move |this| this.attach_paths(&picked));
        });
    }

    /// Attaches files, from the picker or dropped on the window. With nothing being written,
    /// that starts a new message.
    pub fn attach_paths(self: &Rc<Self>, paths: &[PathBuf]) {
        if paths.is_empty() {
            return;
        }
        if self.writing.borrow().is_none() {
            self.compose_new();
        }
        for path in paths {
            if let Err(problem) = self.attach_path(path) {
                self.app.set_draft_error(problem.into());
                break;
            }
        }
        self.publish_files();
    }

    fn attach_path(&self, path: &Path) -> Result<(), String> {
        let name = path.file_name().map_or_else(|| "Attachment".to_string(), |n| n.to_string_lossy().into_owned());
        let too_large = || format!("{name} is too large: attachments are limited to {} MB", MAX_ATTACHMENTS / 1024 / 1024);
        let unreadable = |_| format!("{name} couldn't be read");
        // Before reading it, because a dropped file can be any size at all.
        if path.metadata().map_err(unreadable)?.len() > MAX_ATTACHMENTS as u64 {
            return Err(too_large());
        }
        let data = std::fs::read(path).map_err(unreadable)?;
        let id = self.ensure_draft().ok_or_else(|| format!("{name} couldn't be attached"))?;
        let file = Attachment { mime: mime_type(&name).into(), name: name.clone(), data };
        self.store.attach(id, &file).map_err(|_| too_large())?;
        self.touch_files();
        Ok(())
    }

    fn touch_files(&self) {
        if let Some(writing) = self.writing.borrow_mut().as_mut() {
            writing.touched = true;
        }
    }

    /// The draft behind the composer, created now if there has been no reason to yet.
    fn ensure_draft(&self) -> Option<i64> {
        if let Some(id) = self.save_draft() {
            return Some(id);
        }
        let fields = self.fields()?;
        let account = self.shown.get()?;
        let mut writing = self.writing.borrow_mut();
        let writing = writing.as_mut()?;
        writing.draft = self.store.create_draft(account, &fields, writing.reply_to, &[]).ok();
        writing.touched = true;
        writing.draft
    }

    pub(crate) fn remove_draft_file(&self, index: usize) {
        let draft = self.writing.borrow().as_ref().and_then(|w| w.draft);
        if let Some(id) = draft {
            let _ = self.store.remove_attachment(id, index);
            self.touch_files();
            self.publish_files();
        }
    }

    pub(crate) fn address_edited(self: &Rc<Self>, field: i32, text: &str) {
        self.draft_edited();
        // What was offered no longer fits what is typed, whatever comes next.
        self.app.set_suggesting(-1);
        let (weak, text) = (Rc::downgrade(self), text.to_string());
        self.suggest_timer.start(TimerMode::SingleShot, SUGGEST_DELAY, move || {
            if let Some(this) = weak.upgrade() {
                this.suggest(field, &text);
            }
        });
    }

    fn suggest(&self, field: i32, text: &str) {
        let Some(account) = self.shown.get() else { return };
        let contacts = self.reader.contacts(account, last_address(text), SUGGESTIONS).unwrap_or_default();
        let items: Vec<SharedString> = contacts
            .into_iter()
            .map(|c| if c.name.is_empty() { c.address.into() } else { format!("{} <{}>", c.name, c.address).into() })
            .collect();
        self.app.set_suggesting(if items.is_empty() { -1 } else { field });
        self.app.set_suggestions(ModelRc::new(VecModel::from(items)));
    }

    /// Finishes the address being typed with the chosen one, ready for the next.
    pub(crate) fn accept_suggestion(&self, index: usize) {
        use slint::Model;
        let field = self.app.get_suggesting();
        let Some(chosen) = self.app.get_suggestions().row_data(index) else { return };
        let current = match field {
            0 => self.app.get_draft_to(),
            1 => self.app.get_draft_cc(),
            _ => self.app.get_draft_bcc(),
        };
        let kept = current.len() - last_address(&current).len();
        let text: SharedString = format!("{}{chosen}, ", &current[..kept]).into();
        match field {
            0 => self.app.set_draft_to(text),
            1 => self.app.set_draft_cc(text),
            _ => self.app.set_draft_bcc(text),
        }
        self.app.set_suggesting(-1);
        self.suggest_timer.stop();
        self.app.invoke_end_of_address(field);
    }

    /// Saves an attachment of the open message where the user says. Nothing is ever opened.
    pub(crate) fn save_attachment(&self, index: usize) {
        let Some(id) = self.selected.get() else { return };
        let Some(raw) = self.reader.body(id).ok().flatten() else { return };
        let Some(file) = compose::attachments(&raw).into_iter().nth(index) else { return };
        let parent = chrome::dialog_parent(&self.app);
        std::thread::spawn(move || {
            let chosen = rfd::FileDialog::new().set_parent(&parent).set_file_name(&file.name).save_file();
            let Some(path) = chosen else { return };
            let saved = std::fs::write(&path, &file.data).is_ok();
            on_ui_thread(move |this| {
                let text = if saved { format!("Saved {}", file.name) } else { format!("{} couldn't be saved", file.name) };
                this.show_notice(&text, "", NOTICE_TIME);
            });
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_read_like_sizes() {
        assert_eq!(human_size(900), "900 B");
        assert_eq!(human_size(2048), "2 KB");
        assert_eq!(human_size(3 * 1024 * 1024 / 2), "1.5 MB");
    }

    #[test]
    fn suggestions_finish_the_last_address_only() {
        assert_eq!(last_address("an"), "an");
        assert_eq!(last_address("a@x.example, bì"), "bì");
        assert_eq!(last_address("a@x.example; "), "");
    }

    #[test]
    fn unknown_files_travel_as_bytes() {
        assert_eq!(mime_type("Hóa đơn.PDF"), "application/pdf");
        assert_eq!(mime_type("setup.exe"), "application/octet-stream");
    }
}
