//! What the user has chosen, for the app as a whole and for each account. App settings are one
//! JSON document, so a new one needs no migration: anything missing takes its default.

use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};

use crate::Result;
use crate::store::{Reader, Store};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Appearance {
    #[default]
    System,
    Light,
    Dark,
}

impl Appearance {
    pub const ALL: [Self; 3] = [Self::System, Self::Light, Self::Dark];
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum TextSize {
    Small,
    #[default]
    Medium,
    Large,
}

impl TextSize {
    pub const ALL: [Self; 3] = [Self::Small, Self::Medium, Self::Large];
}

/// When an open thread counts as read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum MarkRead {
    /// As soon as it has been open long enough to be more than passing through.
    #[default]
    OnOpening,
    AfterAWhile,
    Never,
}

impl MarkRead {
    pub const ALL: [Self; 3] = [Self::OnOpening, Self::AfterAWhile, Self::Never];
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub appearance: Appearance,
    pub compact_list: bool,
    /// Starts hidden in the notification area when Windows does.
    pub start_with_windows: bool,
    /// Closing the window leaves the app running, rather than quitting it.
    pub keep_running: bool,
    /// Remote images load in all mail but Spam, without asking.
    pub show_all_images: bool,
    pub sender_logos: bool,
    pub text_size: TextSize,
    pub mark_read: MarkRead,
    /// After a thread leaves the list, the one that took its place opens.
    pub open_next: bool,
    /// How long mail waits to be called back. Zero sends it at once.
    pub undo_send_seconds: u32,
    /// How many of the newest messages have their bodies fetched ahead of being opened.
    pub offline_messages: u32,
    pub notifications: bool,
    pub notification_sound: bool,
    /// Notifications say who wrote and about what, rather than only that mail came.
    pub notification_preview: bool,
    /// Notifications of a login mail offer to copy its code.
    pub login_codes: bool,
    pub taskbar_count: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            appearance: Appearance::default(),
            compact_list: false,
            start_with_windows: false,
            keep_running: true,
            show_all_images: false,
            sender_logos: true,
            text_size: TextSize::default(),
            mark_read: MarkRead::default(),
            open_next: true,
            undo_send_seconds: 10,
            offline_messages: 200,
            notifications: true,
            notification_sound: true,
            notification_preview: true,
            login_codes: true,
            taskbar_count: true,
        }
    }
}

/// Where the window was left. Not a preference: nobody chooses it on the settings screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Placement {
    /// The frame's left and top edges on the desktop, in pixels.
    pub left: i32,
    pub top: i32,
    /// With its frame, and as it comes back from being maximized.
    pub width: i32,
    pub height: i32,
    pub maximized: bool,
}

/// Who an account is to the people it writes to, and whether it may interrupt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identity {
    /// The name on outgoing mail. Empty leaves the one found in the account's own sent mail.
    pub name: String,
    pub signature: String,
    pub notify: bool,
}

impl Default for Identity {
    fn default() -> Self {
        Self { name: String::new(), signature: String::new(), notify: true }
    }
}

impl Store {
    pub fn save_settings(&self, settings: &Settings) -> Result<()> {
        let json = serde_json::to_string(settings).expect("settings are plain data");
        self.with_tx(|tx| {
            tx.execute("INSERT OR REPLACE INTO settings (id, json) VALUES (1, ?1)", [json])?;
            Ok(())
        })
    }

    pub fn save_placement(&self, placement: &Placement) -> Result<()> {
        let json = serde_json::to_string(placement).expect("a placement is plain data");
        self.with_tx(|tx| {
            tx.execute("INSERT OR REPLACE INTO window_placement (id, json) VALUES (1, ?1)", [json])?;
            Ok(())
        })
    }

    pub fn save_identity(&self, account_id: i64, identity: &Identity) -> Result<()> {
        self.with_tx(|tx| {
            tx.execute(
                "UPDATE account SET name = ?2, signature = ?3, notify = ?4 WHERE id = ?1",
                params![account_id, identity.name, identity.signature, identity.notify],
            )?;
            Ok(())
        })
    }
}

impl Reader {
    /// The saved settings. Ones that cannot be read are no reason to stop the app.
    pub fn settings(&self) -> Settings {
        let json: Option<String> =
            self.conn().query_row("SELECT json FROM settings WHERE id = 1", [], |r| r.get(0)).optional().unwrap_or_default();
        json.and_then(|json| serde_json::from_str(&json).ok()).unwrap_or_default()
    }

    pub fn placement(&self) -> Option<Placement> {
        let json: Option<String> =
            self.conn().query_row("SELECT json FROM window_placement WHERE id = 1", [], |r| r.get(0)).optional().unwrap_or_default();
        json.and_then(|json| serde_json::from_str(&json).ok())
    }

    pub fn identity(&self, account_id: i64) -> Result<Identity> {
        let row = self.conn().query_row("SELECT name, signature, notify FROM account WHERE id = ?1", [account_id], |r| {
            Ok(Identity { name: r.get(0)?, signature: r.get(1)?, notify: r.get(2)? })
        });
        Ok(row?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{Account, AuthKind, Security};

    #[test]
    fn signing_in_again_keeps_who_the_account_is() {
        let dir = std::env::temp_dir().join(format!("mailfunction-settings-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let store = Store::open(&dir).unwrap();
        let reader = Reader::open(&dir).unwrap();
        let account = Account {
            id: 0,
            email: "kong@example.com".into(),
            host: "localhost".into(),
            port: 993,
            security: Security::Tls,
            auth: AuthKind::Google,
            smtp_host: "localhost".into(),
            smtp_port: 25,
        };
        let id = store.add_account(&account).unwrap();
        let identity = Identity { name: "Kong".into(), signature: "Thanks".into(), notify: false };
        store.save_identity(id, &identity).unwrap();
        assert_eq!(store.add_account(&account).unwrap(), id);
        assert_eq!(reader.identity(id).unwrap(), identity);

        let settings = Settings { show_all_images: true, undo_send_seconds: 30, ..Settings::default() };
        store.save_settings(&settings).unwrap();
        store.save_settings(&Settings { compact_list: true, ..settings }).unwrap();
        assert_eq!(reader.settings(), Settings { compact_list: true, ..settings });
        assert_eq!(reader.placement(), None);
        let placement = Placement { left: -8, top: 40, width: 1600, height: 900, maximized: true };
        store.save_placement(&placement).unwrap();
        store.save_placement(&placement).unwrap();
        assert_eq!(reader.placement(), Some(placement));
        drop((store, reader));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_setting_added_later_takes_its_default() {
        let saved: Settings = serde_json::from_str(r#"{"show_all_images":true}"#).unwrap();
        assert_eq!(saved, Settings { show_all_images: true, ..Settings::default() });
    }
}
