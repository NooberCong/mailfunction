//! One app per data folder, as two would fight over its database. With the window closed the app
//! carries on in the notification area, so starting it again is how to ask for the window back,
//! or for a message to the address in a `mailto:` link that Windows started it with.
//!
//! The first process holds a named pipe, and later ones pass on what they were started for
//! through it. Only the user who started the app can open it, and nothing from another machine
//! gets in.

use std::path::Path;

use windows::Win32::Foundation::{
    CloseHandle, ERROR_ACCESS_DENIED, ERROR_NO_DATA, ERROR_PIPE_BUSY, ERROR_PIPE_CONNECTED, GENERIC_WRITE, HANDLE, HLOCAL,
    LocalFree,
};
use windows::Win32::Security::Authorization::{ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1};
use windows::Win32::Security::{PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAG_FIRST_PIPE_INSTANCE, FILE_FLAGS_AND_ATTRIBUTES, FILE_READ_ATTRIBUTES, FILE_SHARE_NONE,
    FlushFileBuffers, OPEN_EXISTING, PIPE_ACCESS_INBOUND, ReadFile, WriteFile,
};
use windows::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, GetNamedPipeServerProcessId, PIPE_READMODE_BYTE,
    PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_WAIT, WaitNamedPipeW,
};
use windows::Win32::UI::WindowsAndMessaging::AllowSetForegroundWindow;
use windows::core::{HRESULT, HSTRING, w};

/// The longest request heard. A link is at most a command line, which is shorter.
const LONGEST: usize = 64 * 1024;
/// How long a start waits while the app hears another one.
const PATIENCE_MS: u32 = 5_000;
/// A first process can be on its way out as a second one starts, which then takes over.
const ATTEMPTS: usize = 3;

fn pipe_name(data: &Path) -> String {
    // A backslash past the prefix would end the pipe's name.
    format!(r"\\.\pipe\mailfunction-{}", data.display().to_string().replace('\\', "/"))
}

fn is(error: &windows::core::Error, code: windows::Win32::Foundation::WIN32_ERROR) -> bool {
    error.code() == HRESULT::from_win32(code.0)
}

/// Takes the data folder for this process and returns true. What later starts ask for then comes
/// to `heard`, on a thread of its own: a link, or nothing to just bring up the window. If another
/// process has the folder, passes `request` on to it and returns false.
pub fn claim(data: &Path, request: &str, heard: impl Fn(String) + Send + 'static) -> bool {
    let name = HSTRING::from(pipe_name(data));
    for _ in 0..ATTEMPTS {
        match listen(&name) {
            Ok(pipe) => {
                serve(pipe, heard);
                return true;
            }
            Err(error) if is(&error, ERROR_ACCESS_DENIED) || is(&error, ERROR_PIPE_BUSY) => {
                if ask(&name, request).is_ok() {
                    return false;
                }
            }
            // With no pipe to be had, two apps at once is the lesser harm than none.
            Err(_) => return true,
        }
    }
    false
}

fn listen(name: &HSTRING) -> windows::core::Result<HANDLE> {
    // Its owner alone. By default anyone on the machine may open it to read, which would be enough
    // to keep it busy for as long as they liked.
    let mut owner_only = PSECURITY_DESCRIPTOR::default();
    unsafe { ConvertStringSecurityDescriptorToSecurityDescriptorW(w!("D:P(A;;GA;;;OW)"), SDDL_REVISION_1, &raw mut owner_only, None) }?;
    let attributes = SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: owner_only.0,
        bInheritHandle: false.into(),
    };
    let pipe = unsafe {
        CreateNamedPipeW(
            name,
            PIPE_ACCESS_INBOUND | FILE_FLAG_FIRST_PIPE_INSTANCE,
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
            1,
            0,
            LONGEST as u32,
            0,
            Some(&raw const attributes),
        )
    };
    let created = if pipe.is_invalid() { Err(windows::core::Error::from_thread()) } else { Ok(pipe) };
    unsafe { LocalFree(Some(HLOCAL(owner_only.0))) };
    created
}

/// Hears one start at a time, for as long as the process lasts. The one pipe is never let go,
/// so no later start can take it in between.
fn serve(pipe: HANDLE, heard: impl Fn(String) + Send + 'static) {
    let pipe = pipe.0 as isize; // a handle is only a number, but cannot cross threads as one
    std::thread::spawn(move || {
        let pipe = HANDLE(pipe as *mut _);
        loop {
            // A start with nothing to say can come and go before this, and still counts.
            if let Err(error) = unsafe { ConnectNamedPipe(pipe, None) }
                && !is(&error, ERROR_PIPE_CONNECTED)
                && !is(&error, ERROR_NO_DATA)
            {
                return;
            }
            let request = read(pipe);
            let _ = unsafe { DisconnectNamedPipe(pipe) };
            // Whatever it was, the user asked for the app.
            heard(request.unwrap_or_default());
        }
    });
}

/// All that a start wrote, up to its closing the pipe.
fn read(pipe: HANDLE) -> Option<String> {
    let mut request = Vec::new();
    let mut buffer = [0u8; 4096];
    loop {
        let mut count = 0;
        match unsafe { ReadFile(pipe, Some(&mut buffer), Some(&raw mut count), None) } {
            Ok(()) if count > 0 => request.extend_from_slice(&buffer[..count as usize]),
            _ => break,
        }
        if request.len() > LONGEST {
            return None;
        }
    }
    String::from_utf8(request).ok()
}

fn ask(name: &HSTRING, request: &str) -> windows::core::Result<()> {
    let access = GENERIC_WRITE.0 | FILE_READ_ATTRIBUTES.0;
    let pipe = loop {
        match unsafe { CreateFileW(name, access, FILE_SHARE_NONE, None, OPEN_EXISTING, FILE_FLAGS_AND_ATTRIBUTES(0), None) } {
            Ok(pipe) => break pipe,
            // The app is hearing another start.
            Err(error) if is(&error, ERROR_PIPE_BUSY) => unsafe { WaitNamedPipeW(name, PATIENCE_MS) }.ok()?,
            Err(error) => return Err(error),
        }
    };
    // Once the app has the pipe open it hears this start whatever comes next, so from here on
    // asking again would only ask twice.
    unsafe {
        // This process was started by the user, so it may pass on the right to come forward.
        let mut app = 0;
        if GetNamedPipeServerProcessId(pipe, &raw mut app).is_ok() {
            let _ = AllowSetForegroundWindow(app);
        }
        if !request.is_empty() {
            let _ = WriteFile(pipe, Some(request.as_bytes()), None, None);
        }
        // Returns once the app has read it all, so closing loses none of it.
        let _ = FlushFileBuffers(pipe);
        let _ = CloseHandle(pipe);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;
    use std::time::Duration;

    use super::*;

    #[test]
    fn a_folder_names_one_pipe() {
        let name = pipe_name(Path::new(r"C:\Users\Rog Scar\AppData\Local\mailfunction"));
        assert_eq!(name, r"\\.\pipe\mailfunction-C:/Users/Rog Scar/AppData/Local/mailfunction");
    }

    #[test]
    fn later_starts_reach_the_first_with_what_they_were_started_for() {
        let data = std::env::temp_dir().join(format!("mailfunction-instance-{}", std::process::id()));
        let (sender, heard) = mpsc::channel();
        assert!(claim(&data, "", move |request| sender.send(request).unwrap()));
        let next = || heard.recv_timeout(Duration::from_secs(5)).expect("the first process heard nothing");

        let link = "mailto:a@x.example?subject=Hi%20there&body=Line%20one";
        assert!(!claim(&data, link, |_| panic!("a second process took the folder")));
        assert_eq!(next(), link);
        assert!(!claim(&data, "", |_| unreachable!()));
        assert_eq!(next(), "");

        // Too long to be a link, and the window still comes up.
        assert!(!claim(&data, &"x".repeat(LONGEST + 1), |_| unreachable!()));
        assert_eq!(next(), "");

        // Many at once, as when several links are opened together, and none is lost.
        let starts: Vec<_> = (0..8)
            .map(|n| {
                let data = data.clone();
                std::thread::spawn(move || assert!(!claim(&data, &format!("mailto:{n}@x.example"), |_| unreachable!())))
            })
            .collect();
        starts.into_iter().for_each(|start| start.join().unwrap());
        let mut all: Vec<String> = (0..8).map(|_| next()).collect();
        all.sort();
        assert_eq!(all, (0..8).map(|n| format!("mailto:{n}@x.example")).collect::<Vec<_>>());

        // Another folder is another app.
        assert!(claim(&data.join("other"), "", |_| {}));
    }
}
