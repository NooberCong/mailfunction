# mailfunction

A minimal, fast desktop mail client for Windows, for any number of Gmail accounts: threads, triage,
plain-text compose with attachments, undo send, local search and full keyboard control.
[The plan](reports/Minimal%20email%20client%20tech%20plan.md) covers what comes next and why.

## Layout

- `crates/core`: everything that is not UI. The SQLite store, IMAP sync, the operation queue,
  drafts and sending, Google sign-in and the HTML sanitiser.
- `crates/app`: the Slint window and the locked-down WebView2 that shows HTML mail. The typeface
  is Be Vietnam Pro, bundled in `crates/app/ui/fonts` under the SIL Open Font License.

The UI reads only from the local store and writes only to it, so it never waits on the network. A
change lands in the store and in a durable queue in one transaction, and the server hears about it
later, in order, even across a restart. Until then sync re-applies what is queued over whatever the
server reports, and if the server refuses, the message goes back where it was and the app says so.

One background thread runs every account, with three tasks each: sync and IDLE, which also replays
the queue, on one connection; bodies and server search on a second; and the outbox, over SMTP.

Mail goes out once or not at all. It waits ten seconds to be called back, and anything that cannot
leave waits in the Outbox and tries again. If the connection dies while mail is being handed over,
or the app stops, nobody can say whether it went, so it returns to Drafts for its author to decide.

The webview has no network at all. Remote images stay out of a message until the user loads them,
once or always for that sender. The app then fetches them itself and hands them to the page as
`data:` URLs, so the page's lockdown never changes. Loading them can still tell the sender the
mail was opened.

Avatars show a logo where one can be found: the domain's BIMI record first, then its website's
icon, and initials otherwise. Each domain is asked directly, once, and the answer is kept for 30
days. That shows the domain this machine's IP address, but no message content or address. Shared
domains like gmail.com and anything in Spam are never looked up as senders.

Closing the window leaves the app running in the notification area, so new mail still shows a
notification. Quit from the icon's menu. Starting the app again brings its window back.

Settings are in the sidebar, or `Ctrl+,`, and apply as they change. The app's own are one JSON
document in the store, so a new one needs no migration. Each account's name, signature and
notifications live on the account. Show all images loads every remote image without asking,
except in Spam.

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

`?` lists every key. The ones that matter most: `j` `k` move between threads, `e` archives, `#`
deletes, `r` replies, `c` composes, `/` searches, `z` undoes, and `Ctrl+Enter` sends. Drop a file on
the window to attach it.

Mail is stored in `%LOCALAPPDATA%\mailfunction`, or wherever `MAILFUNCTION_DATA_DIR` points. The
refresh tokens are kept in Windows Credential Manager.

## Test it

```powershell
rustup run stable-x86_64-pc-windows-msvc cargo test   # offline; UI screenshots go to target/ui
.\scripts\test-imap.ps1                               # sync, sending and the live window, needs Docker
rustup run stable-x86_64-pc-windows-msvc cargo test --release -p mailfunction-core --test scale -- --ignored --nocapture
rustup run stable-x86_64-pc-windows-msvc cargo test --release -p mailfunction --test motion -- --nocapture
```

The motion test prints what each animation costs per frame, and fails if the app paints anything
once motion has ended. Real-server tests use Dovecot for IMAP and mailpit for SMTP, and check what
reached each one. Gmail's own commands are pinned by a scripted server in `gmail_wire.rs`, which
proves what goes on the wire but not what Gmail does with it.

No test injects real mouse or keyboard input. The UI and motion tests drive an offscreen window,
and the live test opens a real window but works through the controller. To check the cursor it
posts pointer messages to its own window, which moves nothing on the desktop. It also records the
window's pixels while moving between messages, and fails if the old message outlives the click.
The tray test starts the real executable and sends it the messages Windows would send. The
settings test works each setting from its control to what it changes, and the live and tray tests
cover the ones that need a real page, real SMTP or Windows itself.
