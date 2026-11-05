//! Starting with Windows, through the current user's Run key.

use windows::Win32::System::Registry::{HKEY_CURRENT_USER, RegDeleteKeyValueW};
use windows::core::HSTRING;

use crate::notify::{APP_ID, set_registry};

/// Starts the app with its window closed, as if it had been running all along.
pub const BACKGROUND: &str = "--background";
const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";

fn command(exe: &str) -> String {
    format!(r#""{exe}" {BACKGROUND}"#)
}

/// Turns it on or off. The exe can move, so the app also renews this at each start while it is on.
pub fn set(on: bool) {
    if on {
        if let Ok(exe) = std::env::current_exe() {
            set_registry(RUN_KEY, Some(APP_ID), &command(&exe.to_string_lossy()));
        }
    } else {
        let _ = unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, &HSTRING::from(RUN_KEY), &HSTRING::from(APP_ID)) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_path_with_spaces_stays_one_argument() {
        assert_eq!(command(r"C:\Program Files\mailfunction.exe"), r#""C:\Program Files\mailfunction.exe" --background"#);
    }
}
