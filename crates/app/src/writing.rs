//! The composer: writing, autosaving, attachments, and handing mail to the outbox. What is on
//! screen is saved to the store on every pause, so nothing typed can be lost, and goes to the
//! server when the user leaves it.

use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use mailfunction_core::compose::{self, Attachment, Compose};
use mailfunction_core::draft::MAX_ATTACHMENTS;
use mailfunction_core::store::Store;
use rfd::FileDialog;
use slint::{ModelRc, SharedString, TimerMode, VecModel};
use unicode_normalization::UnicodeNormalization;

use crate::actions::Reversible;
use crate::editor::EditorEvent;
use crate::{Controller, FileItem, chrome, on_ui_thread};

const AUTOSAVE_DELAY: Duration = Duration::from_millis(800);
const NOTICE_TIME: Duration = Duration::from_secs(6);
const SUGGESTIONS: usize = 5;
/// Looking through everyone who ever wrote takes too long to do between two keys.
const SUGGEST_DELAY: Duration = Duration::from_millis(120);

/// The message being written.
pub(crate) struct Writing {
    /// The account it goes from.
    pub account: i64,
    /// What is in the editor now, as it last reported it.
    pub body: String,
    pub html: String,
    /// Pictures from the editor, until there is a draft to keep them with. A reply that is only
    /// looked at is not worth keeping, whatever was in what it quotes.
    pub pictures: Vec<Attachment>,
    /// Its draft, once there is anything worth keeping.
    pub draft: Option<i64>,
    /// The message it answers, whose thread it joins.
    pub reply_to: Option<i64>,
    /// What it started as: a reply left untouched is not worth keeping.
    pub initial: Compose,
    /// Its files changed, or it is new. Otherwise a draft that was only looked at stays exactly
    /// as it is on the server, which may be richer than anything this composer can write.
    pub touched: bool,
    /// It started from nothing, so if it is back to how it began, nothing has been written.
    pub new: bool,
}

impl Writing {
    fn start_draft(&mut self, store: &Store, fields: &Compose) {
        self.draft = store.create_draft(self.account, fields, self.reply_to, &self.pictures).ok();
        if self.draft.is_some() {
            self.pictures.clear();
        }
        self.touched = true;
    }
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

/// The most letters that read at a glance on a file's badge.
const LONGEST_BADGE: usize = 4;

/// The part of a file's name after its last dot, if that is what an extension looks like.
fn extension(name: &str) -> Option<&str> {
    let (stem, extension) = name.rsplit_once('.')?;
    (!stem.is_empty() && !extension.is_empty() && extension.chars().all(|c| c.is_ascii_alphanumeric())).then_some(extension)
}

/// Which of the theme's tones a kind of file wears: the colours people already know them by.
fn file_tone(extension: &str) -> i32 {
    match extension {
        "pdf" => 0,
        "ppt" | "pptx" | "key" | "odp" => 1,
        "zip" | "rar" | "7z" | "gz" | "tar" => 2,
        "xls" | "xlsx" | "xlsm" | "csv" | "ods" | "cell" => 3,
        "mp3" | "wav" | "m4a" | "flac" | "mp4" | "mov" | "avi" | "mkv" | "webm" => 4,
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "heic" | "bmp" | "svg" | "tiff" => 6,
        _ => 5,
    }
}

/// A file as the chips show it. Names from a Mac spell each Korean syllable or accented letter out
/// in parts, which measure narrower than they draw, so they are put together to be shown.
pub(crate) fn file_item(name: &str, size: usize) -> FileItem {
    let name: String = name.nfc().collect();
    let badge = extension(&name).filter(|extension| extension.len() <= LONGEST_BADGE);
    FileItem {
        size: human_size(size).into(),
        extension: badge.map_or_else(String::new, str::to_ascii_uppercase).into(),
        tone: badge.map_or(0, |badge| file_tone(&badge.to_ascii_lowercase())),
        name: name.into(),
    }
}

/// Asks where to save a file. Given a type, the dialog puts the extension back on a new name that
/// leaves it out, which it does not do by itself, and Windows may be hiding extensions anyway.
pub fn save_dialog(name: &str) -> FileDialog {
    let dialog = FileDialog::new().set_file_name(name);
    match extension(name) {
        Some(extension) => {
            dialog.add_filter(format!("{} file", extension.to_ascii_uppercase()), &[extension]).add_filter("All files", &["*"])
        }
        None => dialog,
    }
}

/// Tells Windows this file came from the internet, as a browser would, so that SmartScreen and
/// Office treat it as they should. Some file systems have no place for this, and nothing is lost.
fn mark_as_downloaded(path: &Path) {
    let mut mark = path.as_os_str().to_os_string();
    mark.push(":Zone.Identifier");
    let _ = std::fs::write(mark, "[ZoneTransfer]\r\nZoneId=3\r\n");
}

/// The address being typed in a recipients field: whatever follows the last separator.
fn last_address(field: &str) -> &str {
    field.rsplit([',', ';']).next().unwrap_or(field).trim_start()
}

impl Controller {
    pub(crate) fn compose_new(self: &Rc<Self>) {
        self.start_writing(None, Compose::default());
    }

    /// Starts what a link in a message asks for, from the account that message came to.
    pub fn compose_to(self: &Rc<Self>, fields: Compose) {
        self.start_writing(self.selected_with_account().map(|(_, account)| account), fields);
    }

    fn start_writing(self: &Rc<Self>, account: Option<i64>, fields: Compose) {
        let first = self.accounts.borrow().first().map(|m| m.account.id);
        let Some(account) = account.or(self.shown.get()).or(first) else { return };
        let fields = self.signed(account, fields);
        self.open_composer(account, fields, None, None, &[]);
        if let Some(writing) = self.writing.borrow_mut().as_mut() {
            writing.new = true;
        }
    }

    fn signature(&self, account: i64) -> String {
        let accounts = self.accounts.borrow();
        accounts.iter().find(|m| m.account.id == account).map_or_else(String::new, |m| m.identity.signature.clone())
    }

    /// A new message as an account signs it.
    fn signed(&self, account: i64, fields: Compose) -> Compose {
        let signature = self.signature(account);
        Compose {
            body: compose::with_signature(&fields.body, &signature),
            html: compose::html_with_signature(&fields.html, &signature),
            ..fields
        }
    }

    pub(crate) fn reply(self: &Rc<Self>, all: bool) {
        let Some((id, account)) = self.selected_with_account() else { return };
        let Some(raw) = self.reader.body(id).ok().flatten() else {
            self.show_notice("This message hasn't arrived yet", "", NOTICE_TIME);
            return;
        };
        let own = self.email_of(account);
        if let Some(fields) = compose::reply(&raw, &own, all) {
            self.open_composer(account, self.signed(account, fields), None, Some(id), &[]);
        }
    }

    pub(crate) fn forward(self: &Rc<Self>) {
        let Some((id, account)) = self.selected_with_account() else { return };
        let Some(raw) = self.reader.body(id).ok().flatten() else {
            self.show_notice("This message hasn't arrived yet", "", NOTICE_TIME);
            return;
        };
        if let Some(fields) = compose::forward(&raw) {
            self.open_composer(account, self.signed(account, fields), None, None, &compose::attachments(&raw));
        }
    }

    fn selected_with_account(&self) -> Option<(i64, i64)> {
        let id = self.selected.get()?;
        Some((id, self.reader.account_of(id).ok()??))
    }

    fn email_of(&self, account: i64) -> String {
        let accounts = self.accounts.borrow();
        accounts.iter().find(|m| m.account.id == account).map_or_else(String::new, |m| m.account.email.clone())
    }

    /// Opens a draft, taking it over first if it was written elsewhere. That needs its body,
    /// which may not be here yet: [`Controller::body_ready`] picks this up again if not.
    pub(crate) fn open_draft(self: &Rc<Self>, id: i64) {
        let Some(account) = self.reader.account_of(id).ok().flatten() else { return };
        if let Some(fields) = self.reader.draft(id).ok().flatten() {
            if self.store.take_back(id).unwrap_or(false) {
                self.queued(account);
                self.list.refresh();
                self.open_composer(account, fields, Some(id), None, &[]);
            } else {
                self.show_notice("This is on its way", "", NOTICE_TIME);
            }
            return;
        }
        match self.reader.body(id).ok().flatten() {
            Some(raw) => {
                if let Some(fields) = self.store.adopt_draft(id, &raw).ok().flatten() {
                    self.open_composer(account, fields, Some(id), None, &[]);
                }
            }
            None => {
                // The pane shows a header while it waits, and it must not be the last message's.
                if let Some(row) = self.reader.rows(&[id]).ok().and_then(|rows| rows.into_iter().next()) {
                    self.show_header(&row);
                }
                self.selected.set(Some(id));
                self.opening_draft.set(Some(id));
                self.fetch_selected_body(id);
            }
        }
    }

    fn open_composer(self: &Rc<Self>, account: i64, fields: Compose, draft: Option<i64>, reply_to: Option<i64>, files: &[Attachment]) {
        self.finish_writing();
        self.hide_page();
        self.app.set_draft_to(fields.to.as_str().into());
        self.app.set_draft_cc(fields.cc.as_str().into());
        self.app.set_draft_bcc(fields.bcc.as_str().into());
        self.app.set_draft_subject(fields.subject.as_str().into());
        self.app.set_draft_copies(!fields.cc.is_empty() || !fields.bcc.is_empty());
        self.app.set_draft_error("".into());
        self.app.set_composing(true);
        let needs_recipients = fields.to.is_empty();
        let mut draft = draft;
        let mut touched = false;
        // Files have to live somewhere from the first moment.
        if draft.is_none() && !files.is_empty() {
            draft = self.store.create_draft(account, &fields, reply_to, files).ok();
            touched = true;
        }
        let html = self.editor_html(&fields, draft);
        *self.writing.borrow_mut() = Some(Writing {
            account,
            body: fields.body.clone(),
            html: fields.html.clone(),
            pictures: Vec::new(),
            draft,
            reply_to,
            initial: fields,
            touched,
            new: false,
        });
        self.publish_sender();
        self.publish_files();
        if !self.editor.open(&self.app, &html, !needs_recipients) {
            self.app.set_draft_error("The editor couldn't start".into());
        }
        if needs_recipients {
            self.app.invoke_focus_recipients();
        }
    }

    /// A message as the editor shows it. A draft from before formatting is just its text.
    fn editor_html(&self, fields: &Compose, draft: Option<i64>) -> String {
        let html = if fields.html.is_empty() { compose::text_to_html(&fields.body) } else { fields.html.clone() };
        let files = draft.and_then(|id| self.reader.draft_files(id).ok()).unwrap_or_default();
        compose::show_pictures(&html, &files)
    }

    /// Puts what is being written back in the editor, which may have closed or changed colours.
    pub(crate) fn reload_editor(&self) {
        let Some(fields) = self.fields() else { return };
        let draft = self.writing.borrow().as_ref().and_then(|w| w.draft);
        self.editor.open(&self.app, &self.editor_html(&fields, draft), false);
    }

    /// What the editor said.
    pub fn editor_event(self: &Rc<Self>, event: EditorEvent) {
        match event {
            EditorEvent::Ready => {}
            EditorEvent::Loaded { html, text } => self.editor_loaded(html, text),
            EditorEvent::Changed { html, text } => {
                let mut writing = self.writing.borrow_mut();
                let Some(writing) = writing.as_mut() else { return };
                writing.html = html;
                writing.body = text;
            }
            EditorEvent::Send => return self.send_draft(),
            EditorEvent::Close => return self.finish_writing(),
            EditorEvent::Leave => {
                self.editor.release_focus();
                return self.app.invoke_focus_subject();
            }
            EditorEvent::Sender { index } => return self.choose_sender(index),
            EditorEvent::Discard => return self.discard_draft(),
            EditorEvent::AttachFiles => return self.pick_files(),
            EditorEvent::Picture { cid, name, mime, data } => return self.add_picture(cid, name, mime, &data),
            EditorEvent::File { name, mime, data } => {
                let attached = STANDARD.decode(data).map_err(|_| format!("{name} couldn't be read"));
                if let Err(problem) = attached.and_then(|data| self.attach_bytes(name, Some(mime), data)) {
                    self.app.set_draft_error(problem.into());
                }
                return self.publish_files();
            }
            EditorEvent::Problem { text } => return self.app.set_draft_error(text.into()),
        }
        self.draft_edited();
    }

    /// The browser writes HTML its own way, so what it made of a message is what counts as
    /// untouched, unless the user has already changed something, as a signature can come later.
    fn editor_loaded(&self, html: String, text: String) {
        let fields = self.fields();
        if let Some(writing) = self.writing.borrow_mut().as_mut() {
            if fields.as_ref() == Some(&writing.initial) {
                writing.initial.html.clone_from(&html);
                writing.initial.body.clone_from(&text);
            }
            writing.html = html;
            writing.body = text;
        }
    }

    fn add_picture(&self, cid: String, name: String, mime: String, data: &str) {
        let Ok(data) = STANDARD.decode(data) else { return };
        let file = Attachment { name, mime, data, content_id: Some(cid) };
        let draft = self.writing.borrow().as_ref().and_then(|w| w.draft);
        match draft {
            Some(id) if self.store.attach(id, &file).is_err() => {
                self.app.set_draft_error(format!("Attachments are limited to {} MB", MAX_ATTACHMENTS / 1024 / 1024).into());
            }
            Some(_) => self.touch_files(),
            None => {
                if let Some(writing) = self.writing.borrow_mut().as_mut() {
                    writing.pictures.push(file);
                }
            }
        }
    }

    /// What is in the editor, as text.
    pub fn draft_text(&self) -> String {
        self.writing.borrow().as_ref().map_or_else(String::new, |w| w.body.clone())
    }

    fn publish_sender(&self) {
        let account = self.writing.borrow().as_ref().map(|w| w.account);
        let index = self.accounts.borrow().iter().position(|m| Some(m.account.id) == account);
        self.app.set_draft_from(index.map_or(0, |i| i as i32));
    }

    /// Sends from another account instead. Its signature comes along, and a draft moves there.
    pub fn choose_sender(self: &Rc<Self>, index: usize) {
        self.app.set_choosing_sender(false);
        let to = self.accounts.borrow().get(index).map(|m| m.account.id);
        let from = self.writing.borrow().as_ref().map(|w| w.account);
        let (Some(to), Some(from)) = (to, from.filter(|from| Some(*from) != to)) else { return };
        let moved = match self.save_draft().map(|draft| self.store.move_draft(draft, to)).transpose() {
            Ok(moved) => moved,
            Err(_) => {
                self.app.set_draft_error("This draft couldn't move to that account".into());
                return;
            }
        };
        if let Some(writing) = self.writing.borrow_mut().as_mut() {
            writing.account = to;
            writing.draft = moved;
            // A thread is its account's own. The headers still place a reply where it belongs.
            writing.reply_to = None;
            writing.touched |= moved.is_some();
        }
        self.editor.sign(&compose::signature_html(&self.signature(to)));
        self.publish_sender();
        self.save_draft();
        self.queued(from);
        self.list.refresh();
    }

    fn fields(&self) -> Option<Compose> {
        let writing = self.writing.borrow();
        let writing = writing.as_ref()?;
        Some(Compose {
            to: self.app.get_draft_to().to_string(),
            cc: self.app.get_draft_cc().to_string(),
            bcc: self.app.get_draft_bcc().to_string(),
            subject: self.app.get_draft_subject().to_string(),
            body: writing.body.clone(),
            html: writing.html.clone(),
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
            writing.start_draft(&self.store, &fields);
        }
        writing.draft
    }

    /// Ends writing. A draft goes to the server; one with nothing in it is thrown away.
    pub fn finish_writing(self: &Rc<Self>) {
        let Some(fields) = self.fields() else { return };
        let saved = self.save_draft();
        let changed = self.writing.borrow().as_ref().is_some_and(|w| w.touched || w.initial != fields);
        // A signature alone is not something anyone wrote.
        let blank = fields.is_blank() || self.writing.borrow().as_ref().is_some_and(|w| w.new && w.initial == fields);
        let empty = saved.is_some_and(|id| blank && self.reader.draft_attachments(id).is_ok_and(|f| f.is_empty()));
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
        let writing = self.writing.borrow_mut().take();
        self.editor.hide();
        self.app.set_composing(false);
        self.app.set_suggesting(-1);
        self.app.set_choosing_sender(false);
        if let Some(writing) = writing {
            self.queued(writing.account);
        }
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
            let mut writing = self.writing.borrow_mut();
            let writing = writing.as_mut()?;
            writing.start_draft(&self.store, &fields);
            writing.draft
        });
        let Some(id) = draft else {
            self.app.set_draft_error("This couldn't be saved, so it hasn't been sent".into());
            return;
        };
        let waits = i64::from(self.settings.get().undo_send_seconds);
        if self.store.queue_send(id, chrono::Utc::now().timestamp() + waits).is_err() {
            self.app.set_draft_error("This couldn't be saved, so it hasn't been sent".into());
            return;
        }
        self.close_composer();
        if waits > 0 {
            *self.reversible.borrow_mut() = Some(Reversible::Send(id));
            self.show_notice("Sending", "Undo", Duration::from_secs(waits as u64));
        } else {
            self.show_notice("Sending", "", NOTICE_TIME);
        }
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
            .map(|file| file_item(&file.name, file.size))
            .collect();
        self.app.set_draft_files(ModelRc::new(VecModel::from(items)));
    }

    /// Asks which files to attach, without holding up the window.
    pub(crate) fn pick_files(&self) {
        let parent = chrome::dialog_parent(&self.app);
        std::thread::spawn(move || {
            let picked = FileDialog::new().set_parent(&parent).pick_files().unwrap_or_default();
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
        let unreadable = format!("{name} couldn't be read");
        // Before reading it, because a dropped file can be any size at all.
        if path.metadata().map_err(|_| unreadable.clone())?.len() > MAX_ATTACHMENTS as u64 {
            return Err(format!("{name} is too large: attachments are limited to {} MB", MAX_ATTACHMENTS / 1024 / 1024));
        }
        self.attach_bytes(name, None, std::fs::read(path).map_err(|_| unreadable)?)
    }

    fn attach_bytes(&self, name: String, mime: Option<String>, data: Vec<u8>) -> Result<(), String> {
        let id = self.ensure_draft().ok_or_else(|| format!("{name} couldn't be attached"))?;
        let mime = mime.filter(|m| !m.is_empty()).unwrap_or_else(|| mime_type(&name).into());
        let problem = format!("{name} is too large: attachments are limited to {} MB", MAX_ATTACHMENTS / 1024 / 1024);
        self.store.attach(id, &Attachment { mime, name, data, content_id: None }).map_err(|_| problem)?;
        self.touch_files();
        Ok(())
    }

    /// The files changed, so what was wrong with them last time no longer stands.
    fn touch_files(&self) {
        self.app.set_draft_error("".into());
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
        let mut writing = self.writing.borrow_mut();
        let writing = writing.as_mut()?;
        writing.start_draft(&self.store, &fields);
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
        let Some(account) = self.writing.borrow().as_ref().map(|w| w.account) else { return };
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
            let chosen = save_dialog(&file.name).set_parent(&parent).save_file();
            let Some(path) = chosen else { return };
            let saved = std::fs::write(&path, &file.data).is_ok();
            if saved {
                mark_as_downloaded(&path);
            }
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
    fn only_what_looks_like_an_extension_counts_as_one() {
        assert_eq!(extension("보고서 v2.XLSX"), Some("XLSX"));
        assert_eq!(extension("archive.tar.gz"), Some("gz"));
        assert_eq!(extension("Attachment"), None);
        assert_eq!(extension(".profile"), None);
        assert_eq!(extension("trailing."), None);
        assert_eq!(extension("Mr. Smith's notes"), None);
    }

    #[test]
    fn a_file_wears_its_extension_while_it_fits() {
        let sheet = file_item("보고서.XLSX", 2048);
        assert_eq!((sheet.extension.as_str(), sheet.tone, sheet.size.as_str()), ("XLSX", 3, "2 KB"));
        assert_eq!(file_item("Attachment", 1).extension, "");
        assert_eq!(file_item("seeds.torrent", 1).extension, "");
    }

    #[test]
    fn a_name_spelled_out_in_parts_is_shown_whole() {
        assert_eq!(file_item("\u{1109}\u{116e}\u{110c}\u{1165}\u{11bc}\u{110b}\u{116d}\u{110e}\u{1165}\u{11bc}.pptx", 1).name, "수정요청.pptx");
    }

    #[test]
    fn suggestions_finish_the_last_address_only() {
        assert_eq!(last_address("an"), "an");
        assert_eq!(last_address("a@x.example, bì"), "bì");
        assert_eq!(last_address("a@x.example; "), "");
    }

    #[test]
    fn a_saved_file_is_marked_as_from_the_internet() {
        let path = std::env::temp_dir().join(format!("mf-mark-{}.exe", std::process::id()));
        std::fs::write(&path, b"MZ").unwrap();
        mark_as_downloaded(&path);
        let mark = std::fs::read_to_string(format!("{}:Zone.Identifier", path.display())).unwrap();
        assert!(mark.contains("ZoneId=3"), "{mark}");
        assert_eq!(std::fs::read(&path).unwrap(), b"MZ", "the file itself is untouched");
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn unknown_files_travel_as_bytes() {
        assert_eq!(mime_type("Hóa đơn.PDF"), "application/pdf");
        assert_eq!(mime_type("setup.exe"), "application/octet-stream");
    }
}
