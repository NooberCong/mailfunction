//! Opening `mailto:` links from other apps. Windows offers an app for a kind of link once the app
//! declares it in the current user's hive, and leaves the choice of default to the user: in the
//! list a click on such a link brings up, or in Settings. Windows starts the app with the link,
//! and [`crate::instance`] takes it to the app if it was running already.

use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance, CoTaskMemFree};
use windows::Win32::UI::Shell::{
    AL_EFFECTIVE, AT_URLPROTOCOL, ApplicationAssociationRegistration, IApplicationAssociationRegistration, ShellExecuteW,
};
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
use windows::core::w;

use crate::notify::{APP_ID, set_registry};

/// What Windows calls the app's kind of link. Keep it: the user's choice of default names it.
const PROG_ID: &str = "mailfunction.mailto";
const CAPABILITIES: &str = r"Software\mailfunction\Capabilities";

fn command(exe: &str) -> String {
    format!(r#""{exe}" "%1""#)
}

/// Offers the app for `mailto:` links. The exe can move, so this runs at each start.
pub fn register() -> Option<()> {
    let exe = std::env::current_exe().ok()?;
    let exe = exe.to_string_lossy();
    let icon = format!("{exe},0");
    let class = format!(r"Software\Classes\{PROG_ID}");
    set_registry(&class, None, "Email link")?;
    set_registry(&format!(r"{class}\DefaultIcon"), None, &icon)?;
    set_registry(&format!(r"{class}\shell\open\command"), None, &command(&exe))?;
    set_registry(CAPABILITIES, Some("ApplicationName"), APP_ID)?;
    set_registry(CAPABILITIES, Some("ApplicationDescription"), "Email that keeps up with you")?;
    set_registry(CAPABILITIES, Some("ApplicationIcon"), &icon)?;
    set_registry(&format!(r"{CAPABILITIES}\URLAssociations"), Some("mailto"), PROG_ID)?;
    set_registry(r"Software\RegisteredApplications", Some(APP_ID), CAPABILITIES)
}

/// Whether the user chose the app for `mailto:` links.
pub fn is_default() -> bool {
    let current = unsafe {
        CoCreateInstance::<_, IApplicationAssociationRegistration>(&ApplicationAssociationRegistration, None, CLSCTX_INPROC_SERVER)
            .and_then(|registration| registration.QueryCurrentDefault(w!("mailto"), AT_URLPROTOCOL, AL_EFFECTIVE))
    };
    let Ok(current) = current else { return false };
    let ours = unsafe { current.to_string() }.is_ok_and(|id| id.eq_ignore_ascii_case(PROG_ID));
    unsafe { CoTaskMemFree(Some(current.0.cast())) };
    ours
}

/// Opens the app's page in Windows' default apps, where the user can choose it.
pub fn choose() {
    let page = w!("ms-settings:defaultapps?registeredAppUser=mailfunction");
    unsafe { ShellExecuteW(None, w!("open"), page, None, None, SW_SHOWNORMAL) };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_link_stays_one_argument_whatever_it_holds() {
        assert_eq!(command(r"C:\Program Files\mailfunction.exe"), r#""C:\Program Files\mailfunction.exe" "%1""#);
    }
}
