# mailfunction

A minimal, fast desktop mail client for Windows. It currently reads any number of Gmail accounts
and cannot change a mailbox: folders are opened read-only and messages are never marked as read.
[The plan](reports/Minimal%20email%20client%20tech%20plan.md) covers what comes next and why.

## Layout

- `crates/core`: everything that is not UI. The SQLite store, IMAP sync, Google sign-in and the
  HTML sanitiser.
- `crates/app`: the Slint window and the locked-down WebView2 that shows HTML mail. The typeface
  is Be Vietnam Pro, bundled in `crates/app/ui/fonts` under the SIL Open Font License.

The UI reads only from the local store, so it never waits on the network. One background thread
syncs every account at once, with two connections each: one for sync and IDLE, and one for bodies.

The webview has no network at all. Remote images stay out of a message until the user loads them,
once or always for that sender. The app then fetches them itself and hands them to the page as
`data:` URLs, so the page's lockdown never changes. Loading them can still tell the sender the
mail was opened.

Avatars show a logo where one can be found: the domain's BIMI record first, then its website's
icon, and initials otherwise. Each domain is asked directly, once, and the answer is kept for 30
days. That shows the domain this machine's IP address, but no message content or address. Shared
domains like gmail.com and anything in Spam are never looked up as senders.

## Run it

1. In Google Cloud, create a project, enable the Gmail API, set the OAuth app to External and
   "In production", and create a Desktop client.
2. Save the client JSON as `%USERPROFILE%\.config\mailfunction\google-client.json`.
3. Run the app and sign in. Google will warn that the app is unverified, which is expected for
   a personal project. Add more accounts from the sidebar, and switch with a click or `Ctrl+1`
   to `Ctrl+9`. Removing an account deletes its local mail and its saved sign-in.

```powershell
rustup run stable-x86_64-pc-windows-msvc cargo run --release -p mailfunction
```

Mail is stored in `%LOCALAPPDATA%\mailfunction`, or wherever `MAILFUNCTION_DATA_DIR` points. The
refresh tokens are kept in Windows Credential Manager.

## Test it

```powershell
rustup run stable-x86_64-pc-windows-msvc cargo test   # offline; UI screenshots go to target/ui
.\scripts\test-imap.ps1                               # sync and the live window, needs Docker
rustup run stable-x86_64-pc-windows-msvc cargo test --release -p mailfunction-core --test scale -- --ignored --nocapture
rustup run stable-x86_64-pc-windows-msvc cargo test --release -p mailfunction --test motion -- --nocapture
```

The motion test prints what each animation costs per frame, and fails if the app paints anything
once motion has ended.

No test injects real mouse or keyboard input. The UI and motion tests drive an offscreen window,
and the live test opens a real window but works through the controller. To check the cursor it
posts pointer messages to its own window, which moves nothing on the desktop. It also records the
window's pixels while moving between messages, and fails if the old message outlives the click.
