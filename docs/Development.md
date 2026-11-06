# Development

How the app is put together, and how to run, test and release it. [The plan](../reports/Minimal%20email%20client%20tech%20plan.md) covers what comes next and why.

## How it fits together

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
once or always for that sender. A message then goes up at once, and its images come in behind it
through the same hook that serves the page: from a disk cache, or fetched by the app. So the page's
lockdown never changes, and a message opened again touches no server. Loading images the first time
can still tell the sender the mail was opened.

Anyone can write a From line, so the app believes one only if Gmail does: sync keeps Gmail's own
DMARC verdict with each message, for the one address on show. Only such mail gets a logo, or can be
trusted with always loading images, and nothing in Spam ever is. A logo comes from the sender's own
domain, so the Sender logos setting does tell that domain your IP address, the first time its mail
is on screen. Mail arriving tells nobody. Free hosting domains get no logo, as they would let a
sender give each recipient a domain of their own.

Saved attachments are marked as coming from the internet, as a browser would, and are never opened.

A login code can be copied from its notification, or from a bar over the open message. Mail arrives
as headers, so the notification of a mail that only looks like a login waits up to three seconds for
its body. Finding a code (`crates/core/src/codes.rs`) fails closed, as a wrong code is worse than none:
only mail Gmail verified counts, only one code per mail, and only in the languages listed there. The
code stays out of the notification's XML, which Windows keeps on disk, and out of clipboard history.

Settings are in the sidebar, or `Ctrl+,`, and apply as they change. The app's own are one JSON
document in the store, so a new one needs no migration. Each account's name, signature and
notifications live on the account. Show all images loads every remote image without asking,
except in Spam.

## Run it

Set up a Google client as the [README](../README.md#sign-in) describes, then:

```powershell
rustup run stable-x86_64-pc-windows-msvc cargo run --release -p mailfunction
```

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
The closed test works in the real window, and fails if the UI thread stays busy behind a closed
window or if the window comes back with anything missing. The restored test minimizes the real
window, lets mail and syncs happen, and fails if it comes back with any of itself see-through. The
tray test starts the real executable and sends it the messages Windows would send. The settings
test works each setting from its control to what it changes, and the live and tray tests cover
the ones that need a real page, real SMTP or Windows itself.

## Release it

```powershell
.\scripts\package.ps1
```

This needs [Inno Setup 6](https://jrsoftware.org/isinfo.php), and leaves the executable, the
installer and their checksums in `target\dist`. The C runtime is built in, so neither needs anything
installed beyond WebView2, which Windows 11 includes.

The README's pictures are made up mail in the real window, and taking them touches no real mailbox:

```powershell
rustup run stable-x86_64-pc-windows-msvc cargo test -p mailfunction --test showcase
rustup run stable-x86_64-pc-windows-msvc cargo test -p mailfunction --test showcase -- dark
```
