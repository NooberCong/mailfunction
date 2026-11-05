//! What the user does to mail: triage, undo, labels and search. Every change lands in the store
//! at once and goes to the server through the queue, so nothing here waits on the network.

use std::rc::Rc;
use std::time::Duration;

use mailfunction_core::ops::{Triage, Undo};
use mailfunction_core::store::{MessageRow, Role};
use slint::{ComponentHandle, Model, TimerMode};

use crate::{Controller, LabelKind, PickItem, Verb};

const NOTICE_TIME: Duration = Duration::from_secs(6);
/// How long a thread must stay open to count as read, so that moving through the list does not
/// read everything on the way past.
pub(crate) const READ_DELAY: Duration = Duration::from_millis(400);
/// A pause in typing long enough to be worth a search.
const SEARCH_DELAY: Duration = Duration::from_millis(220);
const SEARCH_RESULTS: usize = 200;

/// What `z` and a notice's button take back.
pub(crate) enum Reversible {
    /// For the account it was done in, which may no longer be the one on screen.
    Action(i64, Undo),
    /// Mail still waiting in the outbox.
    Send(i64),
}

impl Controller {
    pub(crate) fn verb(self: &Rc<Self>, verb: Verb) {
        match verb {
            Verb::Archive => self.triage(Triage::Archive, "Archived"),
            Verb::Inbox => self.triage(Triage::Inbox, "Moved to Inbox"),
            Verb::Trash => self.triage(Triage::Trash, "Deleted"),
            Verb::Spam => self.triage(Triage::Spam, "Reported as spam"),
            Verb::Star => self.toggle_star(),
            Verb::Unread => self.toggle_unread(),
            Verb::Reply => self.reply(false),
            Verb::ReplyAll => self.reply(true),
            Verb::Forward => self.forward(),
            Verb::Compose => self.compose_new(),
            Verb::Undo => self.undo(),
        }
    }

    fn thread_ids(&self) -> Vec<i64> {
        self.thread.borrow().iter().map(|m| m.id).collect()
    }

    /// Tells the shown account's tasks there is work, and catches the sidebar up.
    pub(crate) fn queued(&self) {
        if let Some(account) = self.shown.get() {
            self.queued_for(account);
        }
    }

    fn queued_for(&self, account: i64) {
        if let Some(engine) = self.engine.borrow().as_ref() {
            engine.flush(account);
        }
        self.publish_accounts();
        self.reload_labels();
    }

    /// Does something to the open thread, then moves on to the next if it left the list.
    fn triage(self: &Rc<Self>, triage: Triage, done: &str) {
        let ids = self.thread_ids();
        let Some(account) = self.shown.get().filter(|_| !ids.is_empty()) else { return };
        let Ok(undo) = self.store.act(account, &ids, triage) else { return };
        if undo.is_empty() {
            return;
        }
        self.queued();
        *self.reversible.borrow_mut() = Some(Reversible::Action(account, undo));
        self.show_notice(done, "Undo", NOTICE_TIME);
        if self.app.get_searching() && matches!(triage, Triage::Trash | Triage::Spam) {
            self.list.drop_thread(self.open_thread.get());
        }
        self.after_list_change();
    }

    /// Picks up a change to the list: the open thread stays open if it is still there, and
    /// otherwise the thread that took its place opens.
    fn after_list_change(self: &Rc<Self>) {
        let row = self.app.get_selected_row();
        self.list.refresh();
        match self.open_thread.get().and_then(|t| self.list_position(t)) {
            Some(position) => {
                self.app.set_selected_row(position as i32);
                self.reload_thread();
            }
            None => {
                let count = self.list.row_count();
                if count == 0 || row < 0 {
                    self.clear_message();
                } else {
                    let next = (row as usize).min(count - 1);
                    self.select_row(next);
                    self.app.invoke_reveal(next as i32);
                }
            }
        }
    }

    fn toggle_star(&self) {
        let Some(account) = self.shown.get() else { return };
        let thread = self.thread.borrow().clone();
        let Some(newest) = thread.last() else { return };
        let starred: Vec<i64> = thread.iter().filter(|m| m.flagged).map(|m| m.id).collect();
        let result = if starred.is_empty() {
            self.store.act(account, &[newest.id], Triage::Starred(true))
        } else {
            self.store.act(account, &starred, Triage::Starred(false))
        };
        if result.is_ok() {
            self.queued();
            self.list.refresh();
            self.reload_thread();
        }
    }

    /// Unread if any of it was read, so the same key can also finish the job. It stays open but
    /// keeps its new state until it is opened again.
    fn toggle_unread(&self) {
        let Some(account) = self.shown.get() else { return };
        let ids = self.thread_ids();
        let unread = self.thread.borrow().iter().any(|m| m.seen);
        self.read_timer.stop();
        if self.store.act(account, &ids, Triage::Seen(!unread)).is_ok() {
            self.queued();
            self.list.refresh();
            self.reload_thread();
        }
    }

    /// Marks the open thread read, once it has been open long enough to mean it.
    pub(crate) fn mark_read(&self) {
        let Some(account) = self.shown.get() else { return };
        let unseen: Vec<i64> = self.thread.borrow().iter().filter(|m| !m.seen).map(|m| m.id).collect();
        if !unseen.is_empty() && self.store.act(account, &unseen, Triage::Seen(true)).is_ok() {
            self.queued();
            self.list.refresh();
            self.reload_thread();
        }
    }

    fn undo(self: &Rc<Self>) {
        let Some(reversible) = self.reversible.borrow_mut().take() else { return };
        self.hide_notice();
        match reversible {
            Reversible::Action(account, undo) => {
                if self.store.undo(account, &undo).is_ok() {
                    self.queued_for(account);
                    if self.app.get_searching() {
                        self.run_search();
                    } else {
                        self.after_list_change();
                    }
                }
            }
            Reversible::Send(message) => {
                if self.store.cancel_send(message).unwrap_or(false) {
                    self.queued();
                    self.list.refresh();
                    self.open_draft(message);
                } else {
                    self.show_notice("It has already gone", "", NOTICE_TIME);
                }
            }
        }
    }

    pub(crate) fn show_notice(&self, text: &str, action: &str, time: Duration) {
        self.app.set_notice(text.into());
        self.app.set_notice_action(action.into());
        let weak = self.app.as_weak();
        self.notice_timer.start(TimerMode::SingleShot, time, move || {
            if let Some(app) = weak.upgrade() {
                app.set_notice("".into());
                app.set_notice_action("".into());
            }
        });
    }

    fn hide_notice(&self) {
        self.notice_timer.stop();
        self.app.set_notice("".into());
        self.app.set_notice_action("".into());
    }

    pub(crate) fn notice_clicked(self: &Rc<Self>) {
        self.undo();
    }

    /// Whether any message of the open thread is in the Inbox.
    pub(crate) fn thread_in_inbox(&self) -> bool {
        let inbox = self.labels.borrow().iter().find(|l| l.role == Role::Inbox).map(|l| l.id);
        let Some(inbox) = inbox else { return false };
        self.thread.borrow().iter().any(|m| self.reader.message_labels(m.id).is_ok_and(|l| l.contains(&inbox)))
    }

    pub(crate) fn pick_labels(&self) {
        if !self.app.get_can_label() || self.thread.borrow().is_empty() {
            return;
        }
        self.publish_picks();
        self.app.set_pick_cursor(0);
        self.app.set_picking(true);
    }

    fn publish_picks(&self) {
        let thread = self.thread.borrow();
        let held: Vec<i64> = thread.iter().flat_map(|m| self.reader.message_labels(m.id).unwrap_or_default()).collect();
        let labels: Vec<_> = self.labels.borrow().iter().filter(|l| l.role == Role::None).cloned().collect();
        let items = labels.iter().map(|l| PickItem { name: l.name.as_str().into(), on: held.contains(&l.id) }).collect();
        *self.pick_ids.borrow_mut() = labels.iter().map(|l| l.id).collect();
        crate::update_rows(&self.pick_items, items);
    }

    pub(crate) fn toggle_label(&self, index: usize) {
        let label = self.pick_ids.borrow().get(index).copied();
        let on = self.pick_items.row_data(index).is_some_and(|item| !item.on);
        let (Some(account), Some(label)) = (self.shown.get(), label) else { return };
        if self.store.act(account, &self.thread_ids(), Triage::Label { label, on }).is_ok() {
            self.queued();
            self.publish_picks();
            self.list.refresh();
        }
    }

    pub(crate) fn go_to(self: &Rc<Self>, kind: LabelKind) {
        let index = self.labels.borrow().iter().position(|l| crate::label_kind(l.role) == kind);
        if let Some(index) = index {
            self.close_search();
            self.select_label(index);
        }
    }

    pub(crate) fn search_edited(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        self.search_timer.start(TimerMode::SingleShot, SEARCH_DELAY, move || {
            if let Some(this) = weak.upgrade() {
                this.run_search();
            }
        });
    }

    /// The latest search, for telling its answer from older ones.
    pub fn search_key(&self) -> u64 {
        self.search_key.get()
    }

    /// Shows what is stored here at once, and asks the server for what is not.
    pub fn run_search(&self) {
        let Some(account) = self.shown.get() else { return };
        let query = self.app.get_query().to_string();
        let key = self.search_key.get() + 1;
        self.search_key.set(key);
        self.list.show_results(self.reader.search(account, &query, SEARCH_RESULTS).unwrap_or_default());
        self.clear_message();
        let asked = !query.trim().is_empty()
            && self.engine.borrow().as_ref().is_some_and(|engine| engine.search(account, key, query));
        self.app.set_search_busy(asked);
    }

    /// Adds what only the server knew about. Late answers to old searches are ignored.
    pub(crate) fn search_done(&self, key: u64, ids: &[i64]) {
        if key != self.search_key.get() {
            return;
        }
        self.app.set_search_busy(false);
        let rows: Vec<MessageRow> = self.reader.rows(ids).unwrap_or_default();
        self.list.add_results(rows);
    }

    pub(crate) fn close_search(self: &Rc<Self>) {
        if !self.app.get_searching() {
            return;
        }
        self.search_timer.stop();
        self.search_key.set(self.search_key.get() + 1);
        self.app.set_searching(false);
        self.app.set_search_busy(false);
        self.app.set_query("".into());
        self.select_label(self.app.get_active_label() as usize);
    }
}
