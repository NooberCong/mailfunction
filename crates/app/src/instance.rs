//! One app per data folder, as two would fight over its database. With the window closed the app
//! carries on in the notification area, so starting it again is how to ask for the window back.

use std::path::Path;

use windows::Win32::Foundation::{ERROR_ALREADY_EXISTS, GetLastError, HANDLE, WAIT_OBJECT_0};
use windows::Win32::System::Threading::{CreateEventW, INFINITE, SetEvent, WaitForSingleObject};
use windows::Win32::UI::WindowsAndMessaging::{ASFW_ANY, AllowSetForegroundWindow};
use windows::core::HSTRING;

use crate::on_ui_thread;

fn event_name(data: &Path) -> String {
    // A backslash in a kernel object's name would read as a namespace.
    format!(r"Local\mailfunction-{}", data.display().to_string().replace('\\', "/"))
}

/// Takes the data folder for this process, and returns true. If another process has it, asks that
/// one to show its window and returns false.
pub fn claim(data: &Path) -> bool {
    // One event does both jobs: it exists while the app runs, and setting it wakes the app.
    let Ok(event) = (unsafe { CreateEventW(None, false, false, &HSTRING::from(event_name(data))) }) else {
        return true;
    };
    if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
        unsafe {
            // This process was started by the user, so it may pass on the right to come forward.
            let _ = AllowSetForegroundWindow(ASFW_ANY);
            let _ = SetEvent(event);
        }
        return false;
    }
    let event = event.0 as isize; // a handle is only a number, but cannot cross threads as one
    std::thread::spawn(move || {
        while unsafe { WaitForSingleObject(HANDLE(event as *mut _), INFINITE) } == WAIT_OBJECT_0 {
            on_ui_thread(|this| this.reveal());
        }
    });
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_folder_names_one_object_outside_any_namespace() {
        let name = event_name(Path::new(r"C:\Users\Rog Scar\AppData\Local\mailfunction"));
        assert_eq!(name, r"Local\mailfunction-C:/Users/Rog Scar/AppData/Local/mailfunction");
    }
}
