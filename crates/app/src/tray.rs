//! The app in the notification area, which is where it lives once its window is closed. A dot on
//! the icon says there is unread mail, and the tooltip says how much.

use std::cell::Cell;
use std::mem::transmute;

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
use windows::Win32::UI::Shell::{
    DefSubclassProc, NIF_ICON, NIF_MESSAGE, NIF_SHOWTIP, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY, NIM_SETVERSION,
    NOTIFYICON_VERSION_4, NOTIFYICONDATAW, SetWindowSubclass, Shell_NotifyIconW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, DestroyIcon, DestroyMenu, GetSystemMetrics, MENUINFO, MF_STRING, MIM_STYLE,
    MNS_NOCHECK, PostMessageW, RegisterWindowMessageW, SM_CXSMICON, SetForegroundWindow, SetMenuDefaultItem,
    SetMenuInfo, TPM_BOTTOMALIGN, TPM_RETURNCMD, TPM_RIGHTBUTTON, TrackPopupMenu, WM_APP, WM_CONTEXTMENU, WM_NULL,
    WM_USER,
};
use windows::core::{PCSTR, w};

use crate::notify::APP_ID;
use crate::{chrome, on_ui_thread};

/// The message Windows sends the window about the icon.
pub const CALLBACK: u32 = WM_APP + 1;
/// Which of the window's icons this is. It only ever has the one.
pub const ICON_ID: u32 = 1;
/// A click on the icon, and Enter or Space on it. The Windows headers define these as macros.
pub const NIN_SELECT: u32 = WM_USER;
const NIN_KEYSELECT: u32 = WM_USER + 1;
const SUBCLASS_ID: usize = 2;
const OPEN: usize = 1;
const QUIT: usize = 2;
/// `SetPreferredAppMode` in uxtheme.dll, which exports it by number only.
const SET_PREFERRED_APP_MODE: usize = 135;
const FOLLOW_SYSTEM_THEME: i32 = 1;

fn tip(unread: i64) -> String {
    match unread {
        ..=0 => APP_ID.to_string(),
        _ => format!("{APP_ID} · {unread} unread"),
    }
}

fn identity(hwnd: HWND) -> NOTIFYICONDATAW {
    NOTIFYICONDATAW { cbSize: size_of::<NOTIFYICONDATAW>() as u32, hWnd: hwnd, uID: ICON_ID, ..Default::default() }
}

/// Win32 menus stay light on a dark desktop unless asked, and there is no public way to ask. If
/// Windows ever drops this, the menu is light and nothing else changes.
fn follow_system_theme() {
    unsafe {
        let Ok(uxtheme) = LoadLibraryW(w!("uxtheme.dll")) else { return };
        if let Some(set) = GetProcAddress(uxtheme, PCSTR(SET_PREFERRED_APP_MODE as *const u8)) {
            let set: unsafe extern "system" fn(i32) -> i32 = transmute(set);
            set(FOLLOW_SYSTEM_THEME);
        }
    }
}

/// Runs the menu to its end. This has to happen here rather than on the event loop, which would
/// stop until the menu closes; from here Windows keeps the loop's messages flowing.
fn show_menu(hwnd: HWND, x: i32, y: i32) {
    let choice = unsafe {
        let Ok(menu) = CreatePopupMenu() else { return };
        let _ = AppendMenuW(menu, MF_STRING, OPEN, w!("Open"));
        let _ = AppendMenuW(menu, MF_STRING, QUIT, w!("Quit"));
        let _ = SetMenuDefaultItem(menu, OPEN as u32, 0);
        // Nothing here is ever ticked, so the margin kept for ticks would only be a gap.
        let style = MENUINFO { cbSize: size_of::<MENUINFO>() as u32, fMask: MIM_STYLE, dwStyle: MNS_NOCHECK, ..Default::default() };
        let _ = SetMenuInfo(menu, &raw const style);
        // Without these two, the menu stays up after a click elsewhere.
        let _ = SetForegroundWindow(hwnd);
        let choice = TrackPopupMenu(menu, TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_BOTTOMALIGN, x, y, None, hwnd, None);
        let _ = PostMessageW(Some(hwnd), WM_NULL, WPARAM(0), LPARAM(0));
        let _ = DestroyMenu(menu);
        choice.0 as usize
    };
    match choice {
        OPEN => on_ui_thread(|this| this.reveal()),
        QUIT => {
            let _ = slint::quit_event_loop();
        }
        _ => {}
    }
}

unsafe extern "system" fn watch_tray(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _: usize,
    taskbar_created: usize,
) -> LRESULT {
    if message == CALLBACK {
        match lparam.0 as u32 & 0xffff {
            NIN_SELECT | NIN_KEYSELECT => on_ui_thread(|this| this.reveal()),
            WM_CONTEXTMENU => show_menu(hwnd, i32::from(wparam.0 as i16), i32::from((wparam.0 >> 16) as i16)),
            _ => {}
        }
        return LRESULT(0);
    }
    if message as usize == taskbar_created {
        on_ui_thread(|this| this.restore_tray());
    }
    unsafe { DefSubclassProc(hwnd, message, wparam, lparam) }
}

/// Starts listening for the icon, and for a restarted taskbar, which starts with no icons at all.
/// The window must exist.
pub fn watch(hwnd: HWND) {
    follow_system_theme();
    let created = unsafe { RegisterWindowMessageW(w!("TaskbarCreated")) } as usize;
    let _ = unsafe { SetWindowSubclass(hwnd, Some(watch_tray), SUBCLASS_ID, created) };
}

#[derive(Default)]
pub struct Tray {
    /// The window that owns the icon, once Windows has it.
    owner: Cell<Option<HWND>>,
    /// What the icon shows now. Sync reports often, and most reports change nothing.
    shown: Cell<Option<i64>>,
}

impl Tray {
    /// Puts the icon there, or brings it up to date.
    pub fn show(&self, hwnd: HWND, unread: i64) {
        if self.shown.get() == Some(unread) {
            return;
        }
        let resource = if unread > 0 { chrome::UNREAD_ICON } else { chrome::EXE_ICON };
        let Some(icon) = chrome::exe_icon(resource, unsafe { GetSystemMetrics(SM_CXSMICON) }) else { return };
        let mut data = identity(hwnd);
        data.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP | NIF_SHOWTIP;
        data.uCallbackMessage = CALLBACK;
        data.hIcon = icon;
        for (to, from) in data.szTip.iter_mut().zip(tip(unread).encode_utf16()) {
            *to = from;
        }
        data.Anonymous.uVersion = NOTIFYICON_VERSION_4;
        let added = self.owner.get().is_some();
        let applied = unsafe {
            let applied = Shell_NotifyIconW(if added { NIM_MODIFY } else { NIM_ADD }, &raw const data).as_bool();
            let _ = DestroyIcon(icon); // Windows keeps its own copy
            applied && (added || Shell_NotifyIconW(NIM_SETVERSION, &raw const data).as_bool())
        };
        if applied {
            self.owner.set(Some(hwnd));
        }
        self.shown.set(applied.then_some(unread));
    }

    /// Forgets the icon, which a restarted taskbar has lost, so that [`Self::show`] adds it again.
    pub fn forget(&self) {
        self.owner.set(None);
        self.shown.set(None);
    }
}

impl Drop for Tray {
    fn drop(&mut self) {
        if let Some(hwnd) = self.owner.get() {
            let data = identity(hwnd);
            let _ = unsafe { Shell_NotifyIconW(NIM_DELETE, &raw const data) };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tooltip_mentions_mail_only_when_there_is_some() {
        assert_eq!(tip(0), "mailfunction");
        assert_eq!(tip(45_000), "mailfunction · 45000 unread");
    }
}
