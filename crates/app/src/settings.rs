//! The settings screen. Everything applies as it changes: the UI reads most of it straight from
//! [`Preferences`], and what the rest of the app needs is applied here. Nothing covers the mail
//! but this screen, so the message in the reader goes while it is open and comes back after.

use std::rc::Rc;
use std::time::Duration;

use mailfunction_core::settings::{Appearance, MarkRead, Settings, TextSize};
use slint::{ComponentHandle, TimerMode};

use crate::{Controller, Preferences, Theme, chrome};

/// A pause in typing long enough to be worth saving.
const SAVE_DELAY: Duration = Duration::from_millis(600);

impl From<Settings> for Preferences {
    fn from(s: Settings) -> Self {
        Self {
            appearance: s.appearance as i32,
            compact_list: s.compact_list,
            start_with_windows: s.start_with_windows,
            keep_running: s.keep_running,
            show_all_images: s.show_all_images,
            sender_logos: s.sender_logos,
            text_size: s.text_size as i32,
            mark_read: s.mark_read as i32,
            open_next: s.open_next,
            undo_send: s.undo_send_seconds as i32,
            offline_messages: s.offline_messages as i32,
            notifications: s.notifications,
            notification_sound: s.notification_sound,
            notification_preview: s.notification_preview,
            login_codes: s.login_codes,
            taskbar_count: s.taskbar_count,
        }
    }
}

impl From<Preferences> for Settings {
    fn from(p: Preferences) -> Self {
        let pick = |index: i32, count: usize| (index as usize).min(count - 1);
        Self {
            appearance: Appearance::ALL[pick(p.appearance, Appearance::ALL.len())],
            compact_list: p.compact_list,
            start_with_windows: p.start_with_windows,
            keep_running: p.keep_running,
            show_all_images: p.show_all_images,
            sender_logos: p.sender_logos,
            text_size: TextSize::ALL[pick(p.text_size, TextSize::ALL.len())],
            mark_read: MarkRead::ALL[pick(p.mark_read, MarkRead::ALL.len())],
            open_next: p.open_next,
            undo_send_seconds: p.undo_send as u32,
            offline_messages: p.offline_messages as u32,
            notifications: p.notifications,
            notification_sound: p.notification_sound,
            notification_preview: p.notification_preview,
            login_codes: p.login_codes,
            taskbar_count: p.taskbar_count,
        }
    }
}

impl Controller {
    pub(crate) fn connect_settings_callbacks(self: &Rc<Self>) {
        self.app.on_open_settings(self.bound(|this| this.open_settings()));
        self.app.on_close_settings(self.bound(|this| this.close_settings()));
        self.app.on_preferences_changed(self.bound(|this| this.preferences_changed()));
        let weak = Rc::downgrade(self);
        self.app.on_identity_edited(move |index, name, signature, notify| {
            if let Some(this) = weak.upgrade() {
                this.identity_edited(index as usize, &name, &signature, notify);
            }
        });
    }

    pub fn settings(&self) -> Settings {
        self.settings.get()
    }

    /// What was being written goes to Drafts, as it does on leaving for anywhere else.
    pub fn open_settings(self: &Rc<Self>) {
        self.finish_writing();
        self.returning_to.set(self.open_thread.get());
        self.clear_message();
        self.app.set_showing_keys(false);
        self.app.set_settings_open(true);
    }

    pub fn close_settings(self: &Rc<Self>) {
        self.leave_settings();
        let row = self.returning_to.take().and_then(|thread| self.list_position(thread));
        if let Some(row) = row {
            self.select_row(row);
        }
        self.app.invoke_focus_list();
    }

    /// Takes the screen away, for when something else is about to fill the window.
    pub(crate) fn leave_settings(&self) {
        self.save_identities();
        // Not while typing, as the fields would be written over under the cursor.
        self.publish_accounts();
        self.app.set_settings_open(false);
    }

    fn preferences_changed(&self) {
        let new = Settings::from(self.app.global::<Theme>().get_preferences());
        let old = self.settings.replace(new);
        if old == new {
            return;
        }
        let _ = self.store.save_settings(&new);
        if old.sender_logos != new.sender_logos {
            self.logos.set_enabled(new.sender_logos);
            self.list.refresh();
            self.publish_accounts();
        }
        if old.taskbar_count != new.taskbar_count {
            self.show_badge();
        }
        if old.start_with_windows != new.start_with_windows {
            self.app.invoke_start_with_windows_changed(new.start_with_windows);
        }
    }

    /// Every key lands here, so it is held in memory and written after a pause.
    fn identity_edited(self: &Rc<Self>, index: usize, name: &str, signature: &str, notify: bool) {
        if let Some(mailbox) = self.accounts.borrow_mut().get_mut(index) {
            mailbox.identity.name = name.to_string();
            mailbox.identity.signature = signature.to_string();
            mailbox.identity.notify = notify;
            mailbox.unsaved = true;
        }
        let weak = Rc::downgrade(self);
        self.identity_save.start(TimerMode::SingleShot, SAVE_DELAY, move || {
            if let Some(this) = weak.upgrade() {
                this.save_identities();
            }
        });
    }

    /// Puts the window back where it was left. Call this before showing it.
    pub fn place_window(&self) {
        self.placement.set(chrome::place(&self.app, self.reader.placement()));
    }

    /// Dragging moves the window many times a second, so this waits for it to come to rest. The
    /// window can't be asked on the way out, as Windows may end the app without closing it.
    pub(crate) fn window_changed(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        self.placement_save.start(TimerMode::SingleShot, SAVE_DELAY, move || {
            if let Some(this) = weak.upgrade() {
                this.save_placement();
            }
        });
    }

    pub(crate) fn save_placement(&self) {
        self.placement_save.stop();
        let Some(last) = self.placement.get() else { return };
        let Some(now) = chrome::placement(&self.app, last) else { return };
        if now != last {
            self.placement.set(Some(now));
            let _ = self.store.save_placement(&now);
        }
    }

    pub(crate) fn save_identities(&self) {
        self.identity_save.stop();
        for mailbox in self.accounts.borrow_mut().iter_mut().filter(|m| m.unsaved) {
            mailbox.unsaved = self.store.save_identity(mailbox.account.id, &mailbox.identity).is_err();
        }
    }
}
