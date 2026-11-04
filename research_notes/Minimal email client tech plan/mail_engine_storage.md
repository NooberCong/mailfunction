# Mail Engine and Storage Layer for a Minimal-Footprint Desktop Email Client (state as of 2026-11-04)

Method note for the report writer: repository and crate facts below were read live on 2026-11-04 through the GitHub API, raw source files and the crates.io API, not taken from READMEs alone. "Recent downloads" is the crates.io rolling figure. Server capability lists marked "direct probe" were taken by me on 2026-11-04 with an unauthenticated TLS `CAPABILITY` command, so they have no URL and show only PRE-AUTH capabilities (servers commonly add more after login). Performance figures from library authors are self-reported unless stated otherwise.

## Which IMAP/SMTP client libraries are viable, and what is their real maintenance and extension status?

### Takeaway
No Rust crate is both high-level and complete: `async-imap` is production-proven (Delta Chat) but has CONDSTORE only, with no QRESYNC, while the `imap-codec` family has the protocol coverage but is a codec, and the newest high-level option (`io-imap`) is young and depends on an alpha. MailKit (.NET, MIT) is clearly the most complete client library in any language and the best reference implementation, even if the project stays in Rust.

### Cited Findings

Status table (activity read 2026-11-04):

| Library | Latest version / date | Last commit | Licence | Notes |
|---|---|---|---|---|
| async-imap | 0.12.0, 2026-09-30 | 2026-09-30 | MIT OR Apache-2.0 | Repo moved to chatmail org; ~1.03M recent downloads |
| imap (rust-imap) | 2.4.1 stable, 2025-02-08; 3.0.0-alpha.15 tag | 2026-08-08 (Dependabot only) | Apache-2.0 OR MIT | "Looking for maintainers" |
| imap-codec / imap-types | crates.io stable 1.0.0; repo at 2.0.0-alpha.10 | 2026-11-02 | MIT OR Apache-2.0 | Active |
| imap-next | 0.3.4, 2026-02-18 | 2026-02-18 | MIT OR Apache-2.0 | Quiet for ~8 months |
| imap-client (pimalaya) | 0.3.1, 2026-05-17 | 2026-05-17 | MIT | ARCHIVED, moved to io-imap |
| io-imap (pimalaya) | 0.8.1 | pushed 2026-11-03 | MIT OR Apache-2.0 | 7 GitHub stars; used by himalaya 2.2.1 |
| mail-send | 0.7.0, 2026-11-01 | 2026-11-01 | Apache-2.0 OR MIT | Tokio only |
| mail-builder | 2.0.1, 2026-11-02 | 2026-11-02 | Apache-2.0 OR MIT | Used by Delta Chat |
| lettre | 0.11.23, 2026-08-03 | 2026-10-02 | MIT | ~8.65M recent downloads |
| MailKit / MimeKit | tag 4.18.1; master is 5.0.0 (unreleased) | 2026-11-01 / 2026-11-03 | MIT | 6.9k stars |
| vmime | last release v0.9.2, 2017-01-03 | 2026-11-04 | GPL-3.0 + OpenSSL exception | Burst of author commits in Nov 2026 |
| mailcore2 | last release 0.6.4, 2020-08-01 | 2026-09-28 | BSD-style | 233 open issues |

Sources for the table: [async-imap](https://github.com/chatmail/async-imap), [rust-imap](https://github.com/jonhoo/rust-imap), [imap-codec](https://github.com/duesee/imap-codec), [imap-next](https://github.com/duesee/imap-next), [imap-client](https://github.com/pimalaya/imap-client), [io-imap](https://github.com/pimalaya/io-imap), [mail-send](https://github.com/stalwartlabs/mail-send), [mail-builder](https://github.com/stalwartlabs/mail-builder), [lettre](https://github.com/lettre/lettre), [MailKit](https://github.com/jstedfast/MailKit), [vmime](https://github.com/kisli/vmime), [mailcore2](https://github.com/MailCore/mailcore2), with versions from [crates.io](https://crates.io/crates/async-imap).

async-imap:
- Exposes `select_condstore`, `authenticate<A: Authenticator>` (generic SASL, so XOAUTH2 is an app-supplied authenticator), `uid_mv`, `uid_expunge`, `idle`, plus `compress`, `id` and `quota` extensions — [client.rs](https://github.com/chatmail/async-imap/blob/main/src/client.rs), [extensions/](https://github.com/chatmail/async-imap/tree/main/src/extensions)
- Zero matches for `QRESYNC`, `VANISHED` or `CHANGEDSINCE` in `client.rs`, `mailbox.rs`, `parse.rs` and `fetch.rs`. `Mailbox` does carry `highest_modseq`. Raw escape hatches exist: `run_command`, `read_response` — [client.rs](https://github.com/chatmail/async-imap/blob/main/src/client.rs)
- Default feature is `runtime-async-std`; `runtime-tokio` and `compress` are opt-in, and Delta Chat uses `default-features = false, features = ["runtime-tokio", "compress"]` — [async-imap Cargo.toml](https://github.com/chatmail/async-imap/blob/main/Cargo.toml), [chatmail Cargo.toml](https://github.com/chatmail/core/blob/main/Cargo.toml)
- Gmail labels and msg id accessors were added in 0.11.2 (Feb 2026), when `idle::Handle::wait()` also moved to a 29-minute timeout. `fetch.rs` has `gmail_labels()` and `gmail_msg_id()` but no public X-GM-THRID accessor function — [CHANGELOG](https://github.com/chatmail/async-imap/blob/main/CHANGELOG.md), [fetch.rs](https://github.com/chatmail/async-imap/blob/main/src/types/fetch.rs)

rust-imap: README says "This crate is looking for maintainers"; commits since March 2025 are all dependency bumps — [rust-imap](https://github.com/jonhoo/rust-imap)

imap-codec family:
- Parsing and serialisation only. Streaming-mode, zero-copy, fuzzed, with a `Fragmentizer` for "two-stage parsing" of invalid real-world messages — [imap-codec README](https://github.com/duesee/imap-codec)
- `imap-types/src/extensions` has binary, compress, condstore_qresync, enable, idle, list_extended, metadata, move, namespace, quota, sort, thread, uidplus, unselect, utf8 — [extensions](https://github.com/duesee/imap-codec/tree/main/imap-types/src/extensions)
- Ships about 14 named `quirk_*` feature flags for real-server deviations (for example `quirk_crlf_relaxed`, `quirk_spaces_between_addresses`, `quirk_trailing_space_capability`) — [Cargo.toml](https://github.com/duesee/imap-codec/blob/main/imap-codec/Cargo.toml)
- Consumers already pin the alpha: meli uses `=2.0.0-alpha.6`, io-imap uses `2.0.0-alpha.10` — [melib Cargo.toml](https://github.com/meli/meli/blob/master/melib/Cargo.toml), [io-imap Cargo.toml](https://github.com/pimalaya/io-imap/blob/master/Cargo.toml)
- The changelog's Unreleased section lists ID, UNSELECT, SORT/THREAD, BINARY and METADATA as newly implemented, so these need the 2.0 line rather than 1.0.0 — [CHANGELOG](https://github.com/duesee/imap-codec/blob/main/CHANGELOG.md)

imap-next: a "thin sans I/O abstraction" over literals, AUTHENTICATE and IDLE. io-imap's Cargo.toml depends on imap-codec but not imap-next — [imap-next](https://github.com/duesee/imap-next), [io-imap Cargo.toml](https://github.com/pimalaya/io-imap/blob/master/Cargo.toml)

io-imap / Pimalaya:
- Three layers: `no_std` I/O-free coroutines, a blocking light client, and a full client. Claims IDLE, CONDSTORE and QRESYNC resync, "message bodies flow between socket and storage, never held in memory whole", and PLAIN, LOGIN, XOAUTH2, OAUTHBEARER, SCRAM-SHA-256. The RFC table lists 2177, 2971, 3501, 3691, 4315, 4959, 5161, 5256, 6851, 7162, 7628, with no RFC 4978 (COMPRESS) row — [io-imap README](https://github.com/pimalaya/io-imap)
- Himalaya 2.2.1 (2026-10-02) has been rebuilt on `io-imap`, `io-smtp`, `io-jmap`, `io-gmail`, `io-msgraph`, `io-maildir`, with `mime-rs` still a git-pinned 0.0.1. The older `pimalaya/core` last changed 2026-03-01 and `email-lib` is at 0.27.0 (Feb 2026) — [himalaya Cargo.toml](https://github.com/pimalaya/himalaya/blob/master/Cargo.toml), [pimalaya/core](https://github.com/pimalaya/core), [email-lib](https://crates.io/crates/email-lib)

Stalwart crates: the repos show no GitHub licence metadata, but crates.io and the READMEs say Apache-2.0 OR MIT. mail-send requires Tokio and supports XOAUTH2 with automatic mechanism negotiation; 0.7.0 bumped to mail-parser 1.0 and mail-builder 2.0 — [mail-send](https://github.com/stalwartlabs/mail-send), [CHANGELOG](https://github.com/stalwartlabs/mail-send/blob/main/CHANGELOG.md)

lettre: has `Mechanism::Xoauth2`, but `DEFAULT_MECHANISMS` is only PLAIN and LOGIN, so XOAUTH2 must be requested explicitly — [authentication.rs](https://github.com/lettre/lettre/blob/master/src/transport/smtp/authentication.rs)

Delta Chat does not use lettre or mail-send; it uses `async-smtp` 0.10 — [chatmail Cargo.toml](https://github.com/chatmail/core/blob/main/Cargo.toml)

MailKit:
- IMAP4rev1 and IMAP4rev2, plus IDLE, CONDSTORE, QRESYNC, COMPRESS, MOVE, UIDPLUS, NOTIFY, LIST-STATUS, SPECIAL-USE, OBJECTID, METADATA, PARTIAL, PREVIEW, ESEARCH, MULTIAPPEND, SORT, THREAD, UTF8=ACCEPT, X-GM-EXT1 (MSGID, THRID, RAW, LABELS) and more. SASL includes XOAUTH2, OAUTHBEARER and SCRAM variants — [MailKit README](https://github.com/jstedfast/MailKit)
- 5.0 (unreleased) drops the BouncyCastle dependency in favour of `MimeKit.Core`; the project sets `IsAotCompatible` for net7.0+ and targets net8.0 and net10.0 — [ReleaseNotes](https://github.com/jstedfast/MailKit/blob/master/ReleaseNotes.md), [MailKit.csproj](https://github.com/jstedfast/MailKit/blob/master/MailKit/MailKit.csproj)

vmime: GPL-3.0 with an OpenSSL exception; no release since 2017 but the author landed several commits on 2026-11-02 to 2026-11-04, including a new security policy; GitHub code search returned no `QRESYNC` hits — [vmime](https://github.com/kisli/vmime), [commits](https://github.com/kisli/vmime/commits/master)

mailcore2: BSD-style licence; recent commits by Hoa Dinh (Sept 2026) add Gmail API, ActiveSync and JMAP layers, not IMAP work; Mailspring vendors it under `Vendor/mailcore2` — [mailcore2 commits](https://github.com/MailCore/mailcore2/commits/master), [Mailspring-Sync](https://github.com/Foundry376/Mailspring-Sync)

Server reality (direct probe, pre-auth only, 2026-11-04):
- Fastmail: IMAP4rev2, CONDSTORE, QRESYNC, MOVE, UIDPLUS, SPECIAL-USE, IDLE, AUTH=XOAUTH2, AUTH=OAUTHBEARER
- Gmail: IDLE, X-GM-EXT-1, AUTH=XOAUTH2, AUTH=OAUTHBEARER; CONDSTORE and QRESYNC not visible pre-auth
- Outlook.com / Exchange Online: UIDPLUS, MOVE, IDLE, LITERAL+, AUTH=XOAUTH2, LOGINDISABLED; CONDSTORE and QRESYNC not visible pre-auth
- iCloud: AUTH=XOAUTH2 and AUTH=PLAIN; almost nothing else is shown pre-auth

### Inferences
- Primary recommendation: build a small Tokio client directly on `imap-types` and `imap-codec` 2.x. A QRESYNC-first design is not achievable through `async-imap`'s typed API, and the codec brings quirk handling that took other projects years. Cost: you write the connection state machine, literal handling and DEFLATE layer yourself.
- Fallback: use `async-imap` with `runtime-tokio` plus `compress`, accepting CONDSTORE-only sync, or adding QRESYNC via a fork. Its parser dependency (`imap-proto`) has not been confirmed to parse VANISHED (see Gaps).
- `io-imap` is architecturally the closest fit (sans-I/O, streaming) and worth a two-day spike, but 7 stars, a blocking-only ready-made client, an alpha dependency and one main author make it a risk as a sole foundation.
- `imap` (rust-imap) should be rejected. Delete `imap-client` and `email-lib` from consideration too, as both are superseded by their own author.
- SMTP: `lettre` is the safe default for popularity and MIT licence; `mail-send` is a reasonable choice if you want one vendor with mail-parser and mail-builder. SMTP is a small part of the problem either way.
- vmime's GPL rules it out for a closed commercial build unless a commercial licence exists. mailcore2 is licence-friendly, but its recent direction is non-IMAP and it has no release in six years.
- MailKit is worth keeping open in a second window as the reference for correct behaviour, whichever language is chosen.

### Gaps
- Whether `imap-proto` 0.17 parses VANISHED and MODSEQ fully. My fetch of its source returned nothing, so this is unverified.
- Authenticated capability lists for Gmail, Outlook and iCloud. I found no primary source, and an authenticated test was out of scope. From prior knowledge, Gmail is generally reported as CONDSTORE without QRESYNC and Exchange Online as neither, but I could not confirm this, and it should be tested with real accounts before finalising sync design.
- Whether crates.io `imap-codec` 1.0.0 includes `ext_condstore_qresync` at the same maturity as main.
- rust-imap's blocking-only API is from prior knowledge, not verified this session. Likewise the status of async-std, which is still async-imap's default runtime.
- vmime commercial licensing availability was not verified.

## Which MIME parser should be used, and how do they compare on performance, conformance and malformed mail?

### Takeaway
`mail-parser` is the leading Rust choice on features and claimed speed, but 1.0 is a full rewrite released on 2026-10-24 and already needed a data-losing bug fixed nine days later. Use it, pin it, and keep `mailparse` as a differential-test oracle. All Rust options parse an in-memory buffer, whereas MimeKit can truly reference a source stream.

### Cited Findings
- mail-parser 1.0.0 (2026-10-24) is "a rewrite of the parser for performance, with a new read-only API"; bodies are decoded on access through `decoded()`, `decode_into()`, `decoded_len()` (measures base64 and QP without decoding), `text()` and `text_prefix()` (decodes only what the first bytes need) — [CHANGELOG](https://github.com/stalwartlabs/mail-parser/blob/main/CHANGELOG.md)
- 1.0.1 (2026-11-02) fixed a folded-line parameter bug "losing `name`, `filename` and `boundary`" — [CHANGELOG](https://github.com/stalwartlabs/mail-parser/blob/main/CHANGELOG.md)
- Malformed mail: `PartFlags` records `MISSING_DELIMITER`, `FALLBACK_DELIMITER`, `NO_BLANK_LINE`, `UNTERMINATED`, `NESTING_LIMIT`, `LIMIT_REACHED`; `decoded_checked()` reports `DecodeProblems`. Limits default to depth 64, 10,000 parts, 3 encoded nesting levels — [CHANGELOG](https://github.com/stalwartlabs/mail-parser/blob/main/CHANGELOG.md), [README](https://github.com/stalwartlabs/mail-parser)
- `parse_with()` with `MessageBuffers` and `into_buffers()` reuses one message's storage for the next; `parse_owned()` gives a `Message<'static>` — [CHANGELOG](https://github.com/stalwartlabs/mail-parser/blob/main/CHANGELOG.md)
- Self-reported benchmarks (Apple M4, median of three): about 3.0x (full decode), 3.2x (headers), 2.8x (structure) geometric-mean speedup over `mailparse`, and roughly 4x to 33x against GMime, VMime, libEtPan and Dovecot's parser. Claims every function is fuzzed and tested under Miri. MSRV 1.98, no default features. Multi-byte legacy charsets such as Big5 and ISO-2022-JP need the optional `encoding_rs` feature — [README](https://github.com/stalwartlabs/mail-parser)
- RFC coverage includes 5322, 2045 to 2049, 2047, 2231, 2183, 2392, 6532 and JMAP body lists (RFC 8621 §4.1.4); the `hasAttachment` rule follows RFC 8621 — [README](https://github.com/stalwartlabs/mail-parser), [CHANGELOG](https://github.com/stalwartlabs/mail-parser/blob/main/CHANGELOG.md)
- `mailparse`: 0.18.0 (2026-10-05), 0BSD, one open issue, 4.47M recent downloads, and the parser Delta Chat runs in production (while building with mail-builder 2.0) — [mailparse](https://github.com/staktrace/mailparse), [crates.io](https://crates.io/crates/mailparse), [chatmail Cargo.toml](https://github.com/chatmail/core/blob/main/Cargo.toml)
- MimeKit works directly on raw byte buffers, with pooled buffers, lazily decoded headers, SIMD scanning, and `MimeContentStorage.Source` so "MIME part content can reference the original stream instead of being copied into memory"; it claims comfortable handling of multi-gigabyte mbox files — [MimeKit README](https://github.com/jstedfast/MimeKit)

### Inferences
- `text_prefix()` and `decoded_len()` map directly onto snippet generation and attachment-size display without decoding whole bodies.
- With a whole-message input model, never pass a 25 MB raw message through the parser on the hot path. Fetch BODYSTRUCTURE plus the text parts, and treat attachments as separate streamed parts.
- Build a differential test early: run `mail-parser` and `mailparse` over a real mailbox and flag differences in attachment names, charset output and part counts. Given the 1.0.1 bug, this is cheap insurance.

### Gaps
- No independent benchmark or conformance comparison was found. All speed numbers are the author's.
- No published real-world malformed-corpus failure rates for any parser.

## What local storage design works, and what did Thunderbird, Mailspring, Geary, notmuch and meli learn?

### Takeaway
SQLite in WAL mode via `rusqlite` is the clear default; every comparable client that needs a metadata store uses it (or Xapian, for notmuch). The most useful lesson across projects is the same one: separate message identity from per-folder location, and never key on server UIDs. Mailspring migrated to this in 2026, and Thunderbird is rebuilding for it.

### Cited Findings

SQLite and bindings:
- rusqlite 0.41.1 (2026-10-28), MIT, bundles SQLite 3.54.1 with FTS5 enabled in the bundled build flags — [Cargo.toml](https://github.com/rusqlite/rusqlite/blob/master/Cargo.toml), [build.rs](https://github.com/rusqlite/rusqlite/blob/master/libsqlite3-sys/build.rs), [sqlite3.h](https://github.com/rusqlite/rusqlite/blob/master/libsqlite3-sys/sqlite3/sqlite3.h)
- sqlx 0.9.0 (2026-05-21), repo now under `transact-rs`. Its SQLite driver spawns a background worker thread per connection to present an async API — [crates.io](https://crates.io/crates/sqlx), [worker.rs](https://github.com/transact-rs/sqlx/blob/main/sqlx-sqlite/src/connection/worker.rs)
- Delta Chat uses rusqlite with its own pool, which is "organized as a stack" because "each SQLite connection has its own page cache". It allows at most one write connection, since SQLite's lock "just sleeps in a loop" and blocks a thread — [pool.rs](https://github.com/chatmail/core/blob/main/src/sql/pool.rs)
- Delta Chat's pragmas: `secure_delete=on`, `soft_heap_limit = 8388608`, `foreign_keys=on`, `temp_store=memory`, `auto_vacuum=INCREMENTAL`, `journal_mode=WAL`, `synchronous=NORMAL`, with explicit WAL checkpoint code — [sql.rs](https://github.com/chatmail/core/blob/main/src/sql.rs), [wal_checkpoint.rs](https://github.com/chatmail/core/blob/main/src/sql/pool/wal_checkpoint.rs)
- Mailspring's pragmas: WAL, `page_size=4096`, `cache_size=10000`, `synchronous=NORMAL`, `journal_size_limit=67108864`, because a checkpoint rewinds the WAL but leaves the file at peak size — [MailStore.cpp](https://github.com/Foundry376/Mailspring-Sync/blob/master/MailSync/MailStore.cpp)

Raw message storage:
- SQLite's study: BLOBs under about 100 KB read faster from the database (up to about 2.4x at 10 KB), BLOBs over about 100 KB read faster from separate files (1.4x to 4.3x at 1 MB, depending on page size). 8192 or 16384 byte pages work best for large BLOB I/O. Caveat: SQLite 3.7.8, Ubuntu, Ext4, a SATA disk, circa 2011 — [SQLite](https://www.sqlite.org/intern-v-extern-blob.html)
- Later SQLite measurements (3.19.0): about 35% faster than direct disk I/O for 10 KB BLOBs — [SQLite](https://www.sqlite.org/intern-v-extern-blob.html)
- Mailspring holds bodies in the database and attachments as files; it only fetches contents for the last three months by default and purges bodies "fetched more than 14 days ago of messages older than the body sync window" — [README](https://github.com/Foundry376/Mailspring-Sync), [SyncWorker.cpp](https://github.com/Foundry376/Mailspring-Sync/blob/master/MailSync/SyncWorker.cpp)

Non-SQL stores: redb 4.4.0 (2026-10-13, MIT OR Apache-2.0, active) and heed 0.23.1 (2026-10-26, MIT, LMDB wrapper) are both maintained — [redb](https://github.com/cberner/redb), [heed](https://crates.io/crates/heed)

Thunderbird (Panorama):
- Single global database replacing per-folder databases; compiled behind `MOZ_PANORAMA` (automatic only on nightly) and enabled by pref `mail.panorama.enabled`; no user-facing parts per the source docs — [Panorama docs](https://source-docs.thunderbird.net/en/latest/panorama/index.html)
- IMAP and News use server UIDs as primary keys, which fail globally (Bug 1806770); code relies on detached header objects SQLite does not support (Bug 1952094) — [Panorama docs](https://source-docs.thunderbird.net/en/latest/panorama/index.html)
- Open questions in the docs: full-text indexing with language-aware tokenising, how to represent a message in multiple folders, and whether flags such as Read are per-folder — [Panorama docs](https://source-docs.thunderbird.net/en/latest/panorama/index.html)
- A `LiveView` abstraction provides self-updating, filter-defined message collections for the UI — [Panorama docs](https://source-docs.thunderbird.net/en/latest/panorama/index.html)
- Roadmap (updated 2026-10-22): Panorama "Active", aiming to let users start testing with consistent migration; Conversation View "Planned" and dependent on it; no dates — [Roadmap](https://roadmaps.thunderbird.net/en-US/desktop)
- Bugzilla shows patches for threaded and grouped views and async loading, still assigned, not finished — [Bug 1941471](https://bugzilla.mozilla.org/show_bug.cgi?id=1941471)

Mailspring:
- "Fat" rows: a `data` JSON column plus copied queryable columns, which makes fields cheap to add but slows SCAN queries: "In retrospect we may have chosen a different approach" — [README](https://github.com/Foundry376/Mailspring-Sync)
- Schema V10 created a `MessageFolder` table with one row per physical copy and rebuilt `Message` without location columns. `Placement` has `remoteUID` (0 = not on server), unread, starred, draft, `labels` (X-GM-LABELS of this copy) and `pendingFolderId` ("set while an optimistic move is in flight") — [MailStore.cpp](https://github.com/Foundry376/Mailspring-Sync/blob/master/MailSync/MailStore.cpp), [Placement.hpp](https://github.com/Foundry376/Mailspring-Sync/blob/master/MailSync/Placement.hpp)
- Real-world scale data point: a 1.15 GB, 258k-message database (about 4.5 KB per message). That migration grew it by 385 MB, with a similar WAL peak, so they require 1.5x free space — [MailStore.cpp](https://github.com/Foundry376/Mailspring-Sync/blob/master/MailSync/MailStore.cpp)
- Stable IDs are hashes of headers: "In rare cases, two messages in an account can have the same ID and Mailspring will only show one" — [README](https://github.com/Foundry376/Mailspring-Sync)
- One sync process per account, with force-kill on quit being safe because every change happens within SQLite transactions — [README](https://github.com/Foundry376/Mailspring-Sync)

Geary: `MessageTable` plus `MessageLocationTable (message_id, folder_id, ordering, remove_marker)` from its very first schema, and `ImapFolderPropertiesTable` with `uid_validity` and `uid_next` — [version-001.sql](https://github.com/GNOME/geary/blob/main/sql/version-001.sql)

Delta Chat: an `imap` table maps `(transport_id, rfc724_mid, folder, uid, uidvalidity, target)`, unique on folder, uid and uidvalidity; `imap_sync` holds per-folder uidvalidity, uid_next and a modseq column. Message-ID is the identity — [imap.rs](https://github.com/chatmail/core/blob/main/src/imap.rs), [migrations.rs](https://github.com/chatmail/core/blob/main/src/sql/migrations.rs)

meli: an optional SQLite cache with `envelopes(hash, uid, mailbox_hash, envelope, modsequence)` primary-keyed on `(mailbox_hash, uid)`, and `mailbox(uidvalidity, flags, highestmodseq)`. This is a cache, not a global store — [sqlite3_cache.rs](https://github.com/meli/meli/blob/master/melib/src/imap/sync/sqlite3_cache.rs)

notmuch: Xapian is the store and files stay on disk; the database holds "ghost" message documents for referenced-but-missing messages so threads survive arrival order — [message.cc](https://github.com/notmuch/notmuch/blob/master/lib/message.cc), [add-message.cc](https://github.com/notmuch/notmuch/blob/master/lib/add-message.cc)

Reference code licences: Mailspring-Sync GPLv3; meli and melib EUPL-1.2 OR GPL-3.0-or-later; Geary LGPL; Delta Chat MPL-2.0 — [Mailspring-Sync](https://github.com/Foundry376/Mailspring-Sync), [melib Cargo.toml](https://github.com/meli/meli/blob/master/melib/Cargo.toml), [Geary COPYING](https://github.com/GNOME/geary/blob/main/COPYING), [chatmail LICENSE](https://github.com/chatmail/core/blob/main/LICENSE)

### Inferences
- Use `rusqlite` over `sqlx`. Local SQLite has no network latency to hide, and sqlx's model adds a thread per connection. Run one writer thread fed by a channel plus a small read pool, as Delta Chat does. FTS5 tokenizers and low-level pragmas are also easier with direct bindings.
- redb or LMDB would add a second storage engine without replacing SQL queries or FTS. They have no clear role here.
- Suggested raw storage: split into `meta.db` (hot, small) and `bodies.db` (zstd-compressed raw or decoded text blobs, larger page size) joined by ATTACH, with attachments over a threshold (about 100 KB, from SQLite's crossover) in content-addressed files. Keeping bulky rows out of the metadata file keeps its working set small and lets body eviction happen without fragmenting it. Avoid maildir on Windows: one file per message raises file-count overhead, and its flag-in-filename scheme solves a problem SQLite already handles. Treat this as design reasoning, to be benchmarked on NTFS.
- Packed append-only files have the best write pattern but need your own compaction and crash recovery, which is hard to justify for a solo developer.
- Licence caution: read GPL code in Mailspring and meli for algorithms, but do not copy it into a possibly commercial product. Delta Chat's MPL-2.0 is file-level copyleft, so reuse is possible with conditions.

Recommended core schema (design proposal synthesised from the above):

```sql
account(id, kind, ...);
folder(id, account_id, path, role, parent_id, uidvalidity, uidnext, highestmodseq,
       synced_min_uid, last_full_scan);
message(id INTEGER PRIMARY KEY, account_id, thread_id, rfc_message_id, gm_msgid,
        content_hash, date, from_addr, subject, snippet, size, flags_local, has_attachments);
placement(message_id, folder_id, uid, modseq, flags, pending_folder_id,
          UNIQUE(folder_id, uid));        -- uid 0 or NULL means "unknown/local"
label(id, account_id, name); message_label(message_id, label_id);
thread(id, account_id, gm_thrid, subject_base, last_date, unread_count, ...);
thread_ref(account_id, message_id_header, thread_id);
body(message_id, state, zstd_blob);        -- possibly in bodies.db
attachment(id, message_id, part_path, mime, size, sha256, local_path);
pending_op(id, account_id, kind, payload, depends_on, attempts, state);
```

Model labels as primary and treat folders as a kind of label plus placement. On Gmail a message has one `\All` placement and many label rows; on plain IMAP it has one placement per copy. Keep real columns for anything filtered or sorted on, and avoid Mailspring's JSON-duplicate pattern.

### Gaps
- Panorama's actual table definitions were not on the docs page; they live in Searchfox sources that I did not read.
- No current measurement of SQLite BLOB versus file performance on NTFS with Windows Defender was found. The official study is Linux-based and old.
- I did not retrieve Mailspring's full `constants.h` schema text beyond specific constants.
- No published retrospective from notmuch, meli or Geary on schema mistakes was found; lessons here come from code and comments.

## Which search engine fits at 100k to 1M messages: FTS5, Tantivy or Xapian?

### Takeaway
Use SQLite FTS5 in the same process, with an external or contentless index, and a capped indexed body length. Xapian is ruled out by GPL licensing for a possibly commercial product. Tantivy is the upgrade path if ranking or query features become a bottleneck. No head-to-head email benchmark exists, so sizes below come from separate sources.

### Cited Findings
- FTS5's own docs measured an email corpus of 1636 MiB: `detail=full` 743 MiB, `detail=column` 340 MiB, `detail=none` 134 MiB. `detail=column` loses phrase and NEAR queries; `detail=none` also loses column filters — [FTS5 docs](https://www.sqlite.org/fts5.html)
- Contentless-delete tables (3.43.0+) store only the index but support DELETE and `INSERT OR REPLACE`; external content tables need manual consistency and must be updated before the content table — [FTS5 docs](https://www.sqlite.org/fts5.html)
- Each committed transaction writes a new level-0 segment. `automerge` (default 4) spreads merging across later writes, whereas `crisismerge` (default 16) merges immediately and "can make one write slow". Incremental `merge` can be run in idle time; `optimize` can take a long time — [FTS5 docs](https://www.sqlite.org/fts5.html)
- Deleted content is recoverable from the file unless the `secure-delete` option plus `PRAGMA secure_delete` are used — [FTS5 docs](https://www.sqlite.org/fts5.html)
- Tokenizers: `unicode61` (default), `porter` (English only), `trigram` (substring; 3+ characters; with `detail=none` or `column`, query tokens are restricted). Prefix indexes cost space but make `term*` a direct lookup — [FTS5 docs](https://www.sqlite.org/fts5.html)
- Forum anecdote on Enron: FTS at just under 32% of database size with defaults, about 7.6% with `detail=none`, though the poster later found binary data inflating the corpus, so treat as weak evidence — [SQLite Forum](https://sqlite.org/forum/info/3baccecae55769ff)
- Mailspring caps indexed text at `SEARCH_BODY_MAX_LENGTH` 15000 (raised from 5000) and indexes per thread through `ThreadSearch (subject, to_, from_, body, categories, content_id)` — [constants.h](https://github.com/Foundry376/Mailspring-Sync/blob/master/MailSync/constants.h), [MailProcessor.cpp](https://github.com/Foundry376/Mailspring-Sync/blob/master/MailSync/MailProcessor.cpp)
- Geary uses FTS5 with a custom `geary_tokeniser` (libstemmer stemming) and `prefix="2,4,6,8,10"` — [version-030.sql](https://github.com/GNOME/geary/blob/main/sql/version-030.sql), [version-029.sql](https://github.com/GNOME/geary/blob/main/sql/version-029.sql)
- Tantivy 0.27.0 (2026-10-17), MIT, active. The writer's minimum memory budget is 15 MB per thread with up to 8 threads. Features include mmap directory, LZ4 or Zstd doc store, SIMD compression, startup under 10 ms and stemming for 17 Latin languages, with third-party CJK tokenizers — [index_writer.rs](https://github.com/quickwit-oss/tantivy/blob/main/src/indexer/index_writer.rs), [README](https://github.com/quickwit-oss/tantivy), [crates.io](https://crates.io/crates/tantivy)
- Tantivy continues to make breaking API changes per release (MSRV 1.88 then 1.91 upcoming, many breaking items in 0.27) and remains pre-1.0 — [CHANGELOG](https://github.com/quickwit-oss/tantivy/blob/main/CHANGELOG.md)
- Turso built its native search on Tantivy, splitting files into 512 KB chunks stored in the database, showing transactional integration needs real engineering — [Turso](https://turso.tech/blog/beyond-fts5)
- Xapian is GPL (COPYING is GPLv2) — [Xapian README](https://raw.githubusercontent.com/xapian/xapian/main/xapian-core/README), [COPYING](https://github.com/xapian/xapian/blob/main/xapian-core/COPYING)
- notmuch size anecdotes: about 3M messages at 13 GB, shrinking to 9 GB after compact (compaction did not speed indexing); another user with a 4.1 GiB database for 18.4 GiB of mail. HyperKitty, a different application on Xapian, reported 35 GB for roughly 300k emails — [notmuch list](https://yhetil.org/notmuch/20140724143214.GA13893@mit.edu/T), [HyperKitty issue](https://gitlab.com/mailman/hyperkitty/-/issues/533)

### Inferences
- Rough sizing from the FTS5 ratio (about 45% of source text with `detail=full`, 21% with `column`): 1M messages at 2 to 4 KB of indexed text each is 2 to 4 GB of text, implying about 1 to 1.8 GB with `detail=full`. This is an estimate, not a measurement.
- Suggested configuration: contentless-delete FTS5 (`content=''`, `contentless_delete=1`) since bodies live elsewhere, a cap around 15k characters, HTML stripped, signatures and quoted text optionally skipped, plus separate small column set for from/to/subject. Keep `detail=full` to preserve phrase search, and only drop to `column` if size testing demands it.
- Batch index commits (for example 500 to 2000 messages per transaction) in a low-priority background task, because each commit creates a segment. Run incremental `merge` when idle.
- Tantivy's segment and mmap design should be faster on ranked queries at 1M, but it adds a second consistency domain with SQLite, plus at least 15 MB of writer memory while indexing and a larger binary. Hide search behind a trait so it can be swapped later.
- For CJK, `unicode61` is weak; a custom FTS5 tokenizer or trigram table is needed.

### Gaps
- No credible head-to-head FTS5 vs Tantivy vs Xapian benchmark on email was found, covering index size, memory or incremental update latency at 100k to 1M. One personal evaluation page lists FTS5 at 200 to 500 MB and Tantivy at 1 to 2 GB but gives no corpus or method, so I excluded it from conclusions ([page](https://summer-era.pages.dev/research/indexing-tech/)). A local benchmark on a real mailbox would settle this in about a day.
- No authoritative current size figure for Xapian's glass backend on mail.

## What sync algorithm should be used: offline-first, QRESYNC with fallback, UIDVALIDITY, IDLE limits, lazy bodies, action queues, conflicts and Gmail labels?

### Takeaway
Implement a capability-tiered sync (QRESYNC, then CONDSTORE, then UID plus flag scan) behind one interface, all driven by RFC 4549's order: replay local actions first, then pull. Treat server QRESYNC as untrustworthy (Mailspring disables it on iCloud and still runs a daily full scan). Use two connections per account, not one per folder. On Gmail, sync All Mail, Spam and Trash and use X-GM-LABELS.

### Cited Findings

RFC 4549 (Informational):
- Order: client-to-server actions first, then per-mailbox server-to-client sync; use UIDs everywhere — [RFC 4549 §2, §3](https://www.rfc-editor.org/rfc/rfc4549.html)
- UIDVALIDITY change: empty that mailbox's cache, drop pending actions referencing its UIDs and treat them as failed, but do not cancel scheduled APPENDs — [RFC 4549 §3, §4.1](https://www.rfc-editor.org/rfc/rfc4549.html)
- Flags: use silent `+FLAGS` and `-FLAGS` deltas; "The FLAGS form MUST NOT be used" since it can overwrite other clients' changes — [RFC 4549 §4.2.3](https://www.rfc-editor.org/rfc/rfc4549.html)
- Expunge: plain EXPUNGE can remove messages others marked `\Deleted`, so use UID EXPUNGE; avoid CLOSE, which silently expunges — [RFC 4549 §4.2.4, §4.2.5](https://www.rfc-editor.org/rfc/rfc4549.html)
- Moves: COPY plus `\Deleted`; with UIDPLUS, COPYUID avoids a re-download; without it, search destination by Message-ID and INTERNALDATE — [RFC 4549 §4.2.2](https://www.rfc-editor.org/rfc/rfc4549.html)
- Download with BODY.PEEK to avoid setting `\Seen`, and prioritise processing received data over issuing new fetches to avoid deadlock — [RFC 4549 §4.3.3](https://www.rfc-editor.org/rfc/rfc4549.html)
- Never send fake UIDs to the server; replace with real ones from APPENDUID or COPYUID. Pending actions are ordered and dependent, so failures cascade — [RFC 4549 §5, §5.1](https://www.rfc-editor.org/rfc/rfc4549.html)
- Playback recovery: stop on connection loss, resume at the interrupted command, check non-idempotent commands first; ignore delete failures on messages already gone; silently ignore flag changes on expunged messages — [RFC 4549 §5.1](https://www.rfc-editor.org/rfc/rfc4549.html)
- Connections: about two, not one per mailbox, using STATUS for unselected mailboxes — [RFC 4549 §5.2, §5.3](https://www.rfc-editor.org/rfc/rfc4549.html)
- The RFC does not define conflict merge rules; it relies on deltas, UID EXPUNGE and ordered dependencies — [RFC 4549](https://www.rfc-editor.org/rfc/rfc4549.html)

RFC 7162 (which replaced RFC 5162's QRESYNC — [datatracker](https://datatracker.ietf.org/doc/rfc5162)):
- Client must `ENABLE QRESYNC` after authenticating, then the server sends VANISHED rather than EXPUNGE — [RFC 7162 §3.2.3](https://www.rfc-editor.org/rfc/rfc7162.html)
- `SELECT` with `QRESYNC (uidvalidity modseq known-uids seq-match-data)` delivers flag changes and VANISHED (EARLIER) in one step; if UIDVALIDITY mismatches, the server ignores the rest — [RFC 7162 §3.2.5](https://www.rfc-editor.org/rfc/rfc7162.html)
- CONDSTORE tier: `UID FETCH ... (CHANGEDSINCE n)` returns changed messages; the VANISHED modifier needs QRESYNC — [RFC 7162 §3.1.4.1, §3.2.6](https://www.rfc-editor.org/rfc/rfc7162.html)
- NOMODSEQ on SELECT means no mod-sequences for that mailbox, so CHANGEDSINCE gets BAD; discard the cached value — [RFC 7162 §3.1.2.2](https://www.rfc-editor.org/rfc/rfc7162.html), [RFC 4549 §6.1](https://www.rfc-editor.org/rfc/rfc4549.html)
- Persist HIGHESTMODSEQ only "once the synchronization is complete"; a client cannot safely derive it from a MODSEQ FETCH item alone — [RFC 7162 §6](https://www.rfc-editor.org/rfc/rfc7162.html)
- A modseq-based sync cannot detect expunges without QRESYNC, so the CONDSTORE tier still needs a UID scan — inferred from [RFC 4549 §4.3.1](https://www.rfc-editor.org/rfc/rfc4549.html) and Mailspring's split below

Real implementations:
- Mailspring runs two threads and connections per account: a background worker over all folders and a foreground worker that idles on the primary folder and handles user-driven fetches — [README](https://github.com/Foundry376/Mailspring-Sync)
- Mailspring disables QRESYNC on iCloud: it "returns malformed VANISHED responses and doesn't send the ENABLED untagged response per RFC", which caused false deletions. It cites Apple forum thread 694251 — [SyncWorker.cpp](https://github.com/Foundry376/Mailspring-Sync/blob/master/MailSync/SyncWorker.cpp)
- Mailspring still does a full verification every 24 hours on CONDSTORE and QRESYNC folders, "a safety net for messages we never ingested in the first place (which no modseq will ever cover)". Other intervals: shallow scan 2 minutes, deep scan 10 minutes, 1024 full headers per request, single-range sweep up to 5000 — [SyncWorker.cpp](https://github.com/Foundry376/Mailspring-Sync/blob/master/MailSync/SyncWorker.cpp)
- It does a NOOP on the left-selected folder before STATUS because a stale view would hide new mail, noting "VANISHED reports expunges, not appends" — [SyncWorker.cpp](https://github.com/Foundry376/Mailspring-Sync/blob/master/MailSync/SyncWorker.cpp)
- It also handles RFC 9738 MESSAGELIMIT, citing Yahoo, when a server limits how many messages a FETCH can return — [SyncWorker.cpp](https://github.com/Foundry376/Mailspring-Sync/blob/master/MailSync/SyncWorker.cpp)
- Mailspring on UIDVALIDITY change: keeps messages visible, sets placements to UID 0, rescans, re-hashes headers to the same message IDs, and only deletes rows left at UID 0 once the scan covers the whole folder — [SyncWorker.cpp](https://github.com/Foundry376/Mailspring-Sync/blob/master/MailSync/SyncWorker.cpp)
- Geary and Delta Chat take the simpler route: Geary removes all folder email, and Delta Chat deletes `imap` rows for the old uidvalidity but keeps messages (identity is Message-ID) — [minimal-folder.vala](https://github.com/GNOME/geary/blob/main/src/engine/imap-engine/imap-engine-minimal-folder.vala), [select_folder.rs](https://github.com/chatmail/core/blob/main/src/imap/select_folder.rs)
- meli has `SyncPolicy::{Basic, Condstore, CondstoreQresync}`, but `resync_condstoreqresync` logs "function unimplemented" and falls back to CONDSTORE; `resync_basic` follows RFC 4549 §4.3 — [sync/mod.rs](https://github.com/meli/meli/blob/master/melib/src/imap/sync/mod.rs)
- Delta Chat tracks only IDLE, QUOTA, METADATA and COMPRESS capabilities; it has no QRESYNC capability flag — [capabilities.rs](https://github.com/chatmail/core/blob/main/src/imap/capabilities.rs)
- Delta Chat IDLE: 5-minute no-response timeout, noting Dovecot sends keepalives every 2 minutes by default, and it skips IDLE if new mail may already be waiting — [idle.rs](https://github.com/chatmail/core/blob/main/src/imap/idle.rs)
- Initial sync: Mailspring downloads newest bodies first and walks down towards UID 1, tracked by `syncedMinUID`; a first sync of hundreds of thousands of messages "can take days" — [SyncWorker.cpp](https://github.com/Foundry376/Mailspring-Sync/blob/master/MailSync/SyncWorker.cpp)

Optimistic actions:
- Mailspring tasks live in a SQLite queue and are split into an immediate local operation and a retryable remote operation — [README](https://github.com/Foundry376/Mailspring-Sync)
- Geary's replay queue splits moves into prepare, commit and revoke operations, and `remove_marker` hides rows pending removal — [replay-ops](https://github.com/GNOME/geary/tree/main/src/engine/imap-engine/replay-ops), [version-001.sql](https://github.com/GNOME/geary/blob/main/sql/version-001.sql)
- Delta Chat uses a declarative `target` column on its `imap` table: an empty target means delete, and rows where `target != folder` are pending work. It also has `imap_markseen` and `imap_send` tables — [imap.rs](https://github.com/chatmail/core/blob/main/src/imap.rs), [migrations.rs](https://github.com/chatmail/core/blob/main/src/sql/migrations.rs)

Gmail:
- X-GM-MSGID and X-GM-THRID are 64-bit decimals; X-GM-LABELS are fetchable and searchable; `\All` marks All Mail; X-GM-RAW gives web-style search through SEARCH — [Google](https://developers.google.com/workspace/gmail/imap/imap-extensions)
- Google's page shows `STORE +X-GM-LABELS` for adding labels; it does not document removal or replacement forms — [Google](https://developers.google.com/workspace/gmail/imap/imap-extensions)
- Mailspring ignores virtual label folders and syncs only Spam, All Mail and Trash, using X-GM-LABELS for labels — [README](https://github.com/Foundry376/Mailspring-Sync)
- Connection and bandwidth limits: up to 15 email clients at a time per account, with a "Too many simultaneous connections" error beyond that; on Workspace, 2500 MB per day IMAP download and 500 MB per day upload, with limits subject to change. Google advises no more than 500 labels — [Google](https://support.google.com/mail/answer/7126229), [Workspace limits](https://knowledge.workspace.google.com/admin/gmail/gmail-bandwidth-limits)

### Inferences
Recommended per-folder algorithm:
1. Drain `pending_op` for the account in order (RFC 4549). A failed op reverts its local optimistic state and cascades to dependents.
2. Check UIDVALIDITY. On mismatch, do not wipe. Null the UIDs on placements, rescan, re-match by `X-GM-MSGID`, or Message-ID plus content hash, and delete only what remains after a complete scan.
3. Tiered delta:
   - QRESYNC (and not on a quirk list): `SELECT ... (QRESYNC ...)`, apply FETCH and VANISHED.
   - CONDSTORE: `UID FETCH 1:* (FLAGS) (CHANGEDSINCE n)`, fetch new UIDs above `uidnext`, and periodic `UID SEARCH ALL` or UID-only fetch, diffing to detect expunges.
   - Basic: new UIDs plus a windowed FLAGS scan, newest first, with older ranges swept slowly.
4. Commit state in one transaction, and write HIGHESTMODSEQ only after the full delta is applied.
5. Run a low-frequency full UID reconciliation (daily) on every tier, as Mailspring does.

- Fetch order: envelope, flags, BODYSTRUCTURE and a short text preview for newest first, then the text bodies inside a retention window, then attachments only on demand.
- Connections: one IDLE connection on Inbox per account and one worker. STATUS polling (or LIST-STATUS) covers other folders. This stays well below Gmail's limit with several devices. Optionally add a third connection for user-priority body fetches.
- Conflicts: flags merge as deltas, so last-writer-wins per flag; a move on a message deleted elsewhere is a silent no-op; server state wins after op replay. Only drafts need special handling (keep a local copy rather than overwrite).
- Prefer Delta Chat's declarative "desired state" for moves and flags over logging each user click, because it collapses repeat toggles. Keep imperative ops for APPEND and send.
- Budget for per-provider quirks from day one with a capability override table, since RFC compliance cannot be assumed.
- Gmail's bandwidth limit is a design input: the initial sync should be headers-first and body download should be throttled.

### Gaps
- Authenticated QRESYNC and CONDSTORE support for Gmail, Outlook.com, Yahoo and iCloud was not confirmed from primary sources (see first section). This directly affects how much of the user base uses each tier.
- Default per-user connection limits for Dovecot, Exchange Online and iCloud were not found.
- Whether NOTIFY (RFC 5465) or OBJECTID (RFC 8474) are deployed widely enough to matter was not researched; MailKit implements both.
- The 2500 MB figure is stated for Workspace; it was not confirmed for consumer Gmail accounts.
- Removal semantics of `-X-GM-LABELS` are not in Google's page.

## How should threading and conversation grouping work?

### Takeaway
Use X-GM-THRID when present, and otherwise an incremental, reference-table variant of JWZ, assigning thread IDs at insert time. Treat full in-memory JWZ tree building as a per-thread display step, not a whole-mailbox pass.

### Cited Findings
- RFC 5256 REFERENCES is based on Zawinski's algorithm: link each message through its References chain (In-Reply-To if none), create dummies for missing IDs, keep an existing parent because truncated References are common, skip loop-forming links, prune dummies, then merge by base subject and sort. Message-ID comparison is case-sensitive — [RFC 5256](https://www.rfc-editor.org/rfc/rfc5256.html)
- Base subject extraction strips `Re:`, `Fw:`, `(fwd)`, bracketed blobs and `[fwd: ...]` repeatedly after RFC 2047 decoding — [RFC 5256 §2.1](https://www.rfc-editor.org/rfc/rfc5256.html)
- ORDEREDSUBJECT is "poor man's threading", grouping only by subject — [RFC 5256](https://www.rfc-editor.org/rfc/rfc5256.html)
- meli implements JWZ in `melib/src/thread.rs` (GPL) — [thread.rs](https://github.com/meli/meli/blob/master/melib/src/thread.rs)
- Mailspring: if `gmailThreadID` exists, find a thread by `gThrId`; otherwise look up `ThreadReference` rows on `(threadId, accountId, headerMessageId)`. It looks at no more than 50 references per incoming message, because a rogue client could send a huge list, and it indexes at most 100 per message: the first (thread root) plus the most recent 99 — [MailProcessor.cpp](https://github.com/Foundry376/Mailspring-Sync/blob/master/MailSync/MailProcessor.cpp)
- notmuch assigns a thread ID to Message-IDs not yet seen, stores it so the later message joins, and uses ghost documents to preserve threads when messages are missing or deleted — [add-message.cc](https://github.com/notmuch/notmuch/blob/master/lib/add-message.cc), [message.cc](https://github.com/notmuch/notmuch/blob/master/lib/message.cc)
- Gmail's X-GM-THRID groups messages identically to the web interface — [Google](https://developers.google.com/workspace/gmail/imap/imap-extensions)
- Thunderbird's Conversation View depends on Panorama and is still planned — [Roadmap](https://roadmaps.thunderbird.net/en-US/desktop)

### Inferences
- Algorithm at insert time: collect `Message-ID`, `In-Reply-To`, `References`. Look these up in `thread_ref`. Zero hits: new thread. One hit: join. Several: merge threads, updating the smaller. Insert all IDs into `thread_ref`, which also covers messages arriving before their parents, in the same way as notmuch's ghosts.
- Merging threads must be a cheap single UPDATE, since out-of-order header sync will trigger it often during initial sync.
- Subject fallback should be conservative: only if the message has `Re:`/`Fw:` prefixes, same base subject, and within a time window (around 30 days), because generic subjects like "Hello" would otherwise merge unrelated mail. The window value is judgement, not sourced.
- Conversation is the UI unit across folders, so a thread spans Sent and Inbox, which a global schema gives for free.

### Gaps
- jwz.org could not be fetched (connection refused), so JWZ detail here comes from RFC 5256 and implementations, not the original page.
- Reference-count limits and accuracy data for subject fallback were not found.

## How should credentials and at-rest encryption be handled on Windows?

### Takeaway
Use the `keyring-core` plus `windows-native-keyring-store` crates, set persistence to Local, and expect the 2560-byte blob limit. Store a small master key there and keep tokens encrypted elsewhere if they may exceed it. SQLCipher is viable but should be an optional feature later, since it protects only against offline disk theft and only covers the database.

### Cited Findings
- `CredentialBlobSize` cannot exceed `CRED_MAX_CREDENTIAL_BLOB_SIZE` (5*512 = 2560 bytes); generic TargetName up to 32767 characters; up to 64 attributes — [Microsoft](https://learn.microsoft.com/en-us/windows/win32/api/wincred/ns-wincred-credentiala)
- `CRED_PERSIST_ENTERPRISE` can roam to other computers with a roaming profile; `CRED_PERSIST_LOCAL_MACHINE` stays on the machine — [Microsoft](https://learn.microsoft.com/en-us/windows/win32/api/wincred/ns-wincred-credentiala)
- The keyring ecosystem was re-architected: `keyring` 4.2.0 is now a thin wrapper and sample-code crate, and applications should use `keyring-core` (1.0.1, 2026-10-13) plus a store crate. Maintained by Dan Brotsky under `open-source-cooperative`, MIT OR Apache-2.0 — [keyring-rs](https://github.com/open-source-cooperative/keyring-rs), [keyring-core](https://crates.io/crates/keyring-core)
- `windows-native-keyring-store` 1.1.0 (2026-05-24): credentials default to Enterprise persistence, overridable with the `persistence` modifier. It warns Windows Credential Manager "loses updates when credentials, even different ones, are written and deleted concurrently", and uses a named mutex keyed by user SID. The default `search` feature pulls in `regex`, typically over 1 MB, and can be turned off — [README](https://github.com/open-source-cooperative/windows-native-keyring-store), [lib.rs](https://github.com/open-source-cooperative/windows-native-keyring-store/blob/main/src/lib.rs)
- SQLCipher 4.20.0 (October 2026, baseline SQLite 3.54.1), BSD-3-Clause — [CHANGELOG](https://github.com/sqlcipher/sqlcipher/blob/master/CHANGELOG.md), [LICENSE](https://github.com/sqlcipher/sqlcipher/blob/master/LICENSE.md)
- Design: AES-256-CBC per page with random IV, HMAC-SHA512 per page, PBKDF2-HMAC-SHA512 at 256,000 iterations by default, raw keys supported. Main database, journals and WAL are encrypted, but other transient files are not, so temp storage should be in memory — [SQLCipher design](https://www.zetetic.net/sqlcipher/design/)
- rusqlite supports `bundled-sqlcipher` and `bundled-sqlcipher-vendored-openssl`; bundled SQLCipher sets `SQLITE_TEMP_STORE=2` — [README](https://github.com/rusqlite/rusqlite), [build.rs](https://github.com/rusqlite/rusqlite/blob/master/libsqlite3-sys/build.rs)
- Delta Chat ships this configuration (`rusqlite/bundled-sqlcipher-vendored-openssl`), with `cipher_memory_security = OFF` commented as "Too slow on Android" — [Cargo.toml](https://github.com/chatmail/core/blob/main/Cargo.toml), [sql.rs](https://github.com/chatmail/core/blob/main/src/sql.rs)
- SQLite3MultipleCiphers v2.6.1 (2026-10-12), MIT, active, is an alternative; rusqlite's Cargo.toml shows no feature for it — [SQLite3MultipleCiphers](https://github.com/utelle/SQLite3MultipleCiphers), [rusqlite Cargo.toml](https://github.com/rusqlite/rusqlite/blob/master/Cargo.toml)

### Inferences
- OAuth refresh tokens can be large, so do not assume they fit in 2560 bytes. A robust pattern is one 32-byte per-install key in Credential Manager (Local persistence), with tokens encrypted by AES-GCM or ChaCha20-Poly1305 in the database. This also avoids many Credential Manager writes on refresh. Serialise refresh with one in-process lock per account, because of both Credential Manager's lost-update warning and rotating refresh tokens.
- Keep access tokens in memory only. Store refresh tokens and their expiry only.
- Credential Manager and DPAPI-style protection are bound to the logged-in user, so malware running as that user can read them. Encryption at rest therefore mainly protects a stolen powered-off disk, for which full-disk encryption is the better layer.
- Recommendation: do not enable SQLCipher in v1. Design for it (all database opens go through one function, temp store in memory) and add it as a "privacy mode" if commercial demand appears. Note that any attachment files, Tantivy index or logs would sit outside it.
- If adopted, pass a raw key from the keychain to skip the PBKDF2 startup cost.
- OpenSSL vendoring raises build complexity on Windows. That is a further reason to defer.

### Gaps
- Published SQLCipher overhead figures: the design page gives none and I found no reliable independent number.
- Whether the Windows store encodes string secrets as UTF-16 (which would halve the effective limit) was not verified; byte-oriented `set_secret` APIs should be tested.
- Typical refresh token sizes by provider were not researched (provider-specific auth is out of scope).
- DPAPI (`CryptProtectData`) as an alternative to Credential Manager was not researched in detail.
- Availability of BitLocker or device encryption on Windows 11 Home was not researched.

## How can RSS be kept low with big mailboxes?

### Takeaway
The design rule is: never hold a mailbox, only a window. Keep message lists as row IDs paged from SQLite, cap per-connection page caches, stream attachments to disk by part, and do not rely on mmap on Windows. There is no published RSS baseline for a comparable client, so set a budget and measure.

### Cited Findings
- SQLite's page cache is per connection, which is why Delta Chat reuses the most recently used connection first — [pool.rs](https://github.com/chatmail/core/blob/main/src/sql/pool.rs)
- Delta Chat sets `soft_heap_limit = 8388608` (8 MiB), "same as set in Android SQLiteDatabase" — [sql.rs](https://github.com/chatmail/core/blob/main/src/sql.rs)
- Mailspring uses `cache_size=10000` (about 40 MB per connection at 4096-byte pages) and caps the WAL file at 64 MB — [MailStore.cpp](https://github.com/Foundry376/Mailspring-Sync/blob/master/MailSync/MailStore.cpp)
- Mailspring warns that a statement left with its cursor open pins the WAL read snapshot — [MailStore.cpp](https://github.com/Foundry376/Mailspring-Sync/blob/master/MailSync/MailStore.cpp)
- mmap: disabled by default. It may reduce RAM because SQLite shares pages with the OS cache, and is "mostly a benefit for queries". An I/O error on a mapped file crashes the program. On Windows a mapped file cannot be truncated, so VACUUM or auto_vacuum shrink fails silently (space is reused later) — [SQLite mmap](https://www.sqlite.org/mmap.html)
- async-imap returns fetched bodies as `&[u8]` from a `Fetch` item, so a full-message fetch is held in memory — [fetch.rs](https://github.com/chatmail/async-imap/blob/main/src/types/fetch.rs)
- imap-codec parsing is streaming-capable and returns `Incomplete` instead of truncating; partial section fetches such as `BODY.PEEK[1.2.3.4.MIME]<42.1337>` are first-class — [imap-codec README](https://github.com/duesee/imap-codec)
- io-imap claims bodies flow between socket and storage without being held whole — [io-imap README](https://github.com/pimalaya/io-imap)
- mail-parser's `parse_with()` with `MessageBuffers` reuses allocations across messages; values borrow from the input — [CHANGELOG](https://github.com/stalwartlabs/mail-parser/blob/main/CHANGELOG.md), [README](https://github.com/stalwartlabs/mail-parser)
- Tantivy's writer requires at least 15 MB per thread; its directory is mmap-based, so OS-cache pages rather than heap — [index_writer.rs](https://github.com/quickwit-oss/tantivy/blob/main/src/indexer/index_writer.rs), [README](https://github.com/quickwit-oss/tantivy)
- Dropping `search` from the keyring store saves the `regex` crate, typically over 1 MB — [windows-native-keyring-store](https://github.com/open-source-cooperative/windows-native-keyring-store)
- Himalaya's release profile uses `lto = "fat"`, `codegen-units = 1`, `strip = "symbols"`, `panic = "abort"` — [himalaya Cargo.toml](https://github.com/pimalaya/himalaya/blob/master/Cargo.toml)
- Mailspring uses separate processes per account, which simplifies failure handling, and purges old bodies on a schedule — [README](https://github.com/Foundry376/Mailspring-Sync), [SyncWorker.cpp](https://github.com/Foundry376/Mailspring-Sync/blob/master/MailSync/SyncWorker.cpp)

### Inferences
Practical rules (design guidance, not measured):
- Lists: the UI gets a virtualised window of 100 to 200 rows through keyset pagination (`WHERE (date, id) < ?`), never OFFSET and never the whole folder. Add covering indexes for `(folder_id, date DESC)` and `(thread.last_date)`.
- SQLite budget: writer connection at about 8 to 16 MB cache, 2 readers at about 4 to 8 MB. Set `PRAGMA cache_size` negative for KiB, and `soft_heap_limit` as a safety net. Call `sqlite3_db_release_memory` or `PRAGMA shrink_memory` when the window is hidden or idle.
- Leave mmap off, or use a small value (64 to 256 MB) for read connections only on `meta.db` after benchmarking. Mapped pages may count in Windows working-set figures that users see in Task Manager, which would defeat the marketing goal of a low number, and auto_vacuum shrink fails with it.
- Keep transactions short and finalise statements, or the WAL grows. Set `journal_size_limit`, and run passive checkpoints from the writer thread.
- Network: decode-to-disk streaming for attachments, with a bounded channel between socket and writer so a slow disk applies backpressure. Partial fetches in 1 to 4 MB chunks give resume and limit buffers.
- Sync batches: FETCH in bounded chunks (Mailspring's 1024-header batches are a tested number), parse, write, and drop. No per-folder in-memory caches of envelopes.
- Tokio: use a small fixed worker pool (2 threads) or even current-thread runtime, as mail I/O is not CPU-bound; put parsing on a bounded blocking pool.
- HTML rendering, not the engine, is likely the largest memory consumer, so keep the engine's budget in perspective. (GUI work is out of scope, but it dominates the total.)
- Set a CI budget, for example under 60 MB engine RSS with a 500k-message account open and idle, and track it with a synthetic mailbox. This target is a suggestion, not an industry figure.
- Consider a process split (UI vs engine) only if needed. It gives crash isolation, and the OS can reclaim the engine's memory when restarted, at the cost of IPC.

### Gaps
- No published RSS measurements for Delta Chat, himalaya, meli, Mailspring-Sync or Thunderbird under comparable mailboxes were found.
- Actual .NET Native AOT RSS for a MailKit-based engine was not found. MailKit is marked AOT-compatible, but no footprint data exists to compare with Rust.
- Windows working-set accounting of SQLite mmap pages was reasoned from general behaviour, not measured.

## What is the recommended overall architecture and stack?

### Takeaway
A Rust core, in one process, with SQLite as the single source of truth, a single writer thread, a persistent operation queue, and a capability-tiered IMAP engine on `imap-codec`. All key components are available under MIT, Apache or BSD licences, so commercial use stays open if copyleft dependencies and copied GPL code are avoided.

### Cited Findings
- Licence check of the proposed stack: imap-codec and imap-types MIT OR Apache-2.0; mail-parser and mail-builder Apache-2.0 OR MIT; lettre MIT; rusqlite MIT; tantivy MIT; keyring-core MIT OR Apache-2.0; SQLCipher BSD-3-Clause — [crates.io](https://crates.io/crates/imap-codec), [mail-parser](https://crates.io/crates/mail-parser), [lettre](https://crates.io/crates/lettre), [rusqlite](https://crates.io/crates/rusqlite), [tantivy](https://crates.io/crates/tantivy), [keyring-core](https://crates.io/crates/keyring-core), [SQLCipher](https://github.com/sqlcipher/sqlcipher/blob/master/LICENSE.md)
- Copyleft to avoid linking or copying: vmime (GPL-3.0), Xapian (GPL), melib (EUPL-1.2 OR GPL-3.0-or-later), Mailspring-Sync (GPLv3) — [vmime](https://github.com/kisli/vmime), [Xapian README](https://raw.githubusercontent.com/xapian/xapian/main/xapian-core/README), [melib](https://crates.io/crates/melib), [Mailspring-Sync](https://github.com/Foundry376/Mailspring-Sync)
- Delta Chat proves the Rust plus Tokio plus rusqlite plus IMAP production shape, at release v2.67.0 (2026-11-02), though it is a chat product with a much narrower sync need than a full mail client — [chatmail/core](https://github.com/chatmail/core)
- Himalaya now spans IMAP, JMAP, Gmail API and Microsoft Graph through separate `io-*` crates, and Mailspring added Graph paths for Microsoft calendar and send flows in Oct to Nov 2026, showing that clients are moving beyond pure IMAP for the big providers — [himalaya Cargo.toml](https://github.com/pimalaya/himalaya/blob/master/Cargo.toml), [Mailspring-Sync commits](https://github.com/Foundry376/Mailspring-Sync/commits/master)

### Inferences
Recommended stack:

| Layer | Primary choice | Alternative |
|---|---|---|
| IMAP | `imap-codec`/`imap-types` 2.x plus own Tokio session | `async-imap` (CONDSTORE tier only), `io-imap` after a spike |
| SMTP | `lettre` | `mail-send` |
| MIME parse | `mail-parser` 1.x, pinned | `mailparse` as oracle |
| MIME build | `mail-builder` 2.x | |
| Database | `rusqlite` bundled, WAL, one writer plus read pool | |
| Raw storage | `bodies.db` zstd blobs, attachments as files | Packed files |
| Search | FTS5 contentless-delete behind a trait | Tantivy |
| Secrets | `keyring-core` + `windows-native-keyring-store`, Local persistence, no `search` | |
| At-rest encryption | Not in v1 | SQLCipher later |
| .NET equivalent | MailKit + MimeKit + Microsoft.Data.Sqlite, Native AOT | |
| C++ equivalent | mailcore2 (BSD) + SQLiteCpp, as Mailspring does | |

Component shape:
- `Account actor` per account owns two IMAP sessions: an IDLE watcher and a worker.
- `Sync engine` is a pure-ish state machine (capabilities + folder state in, commands + DB changes out), which makes it testable with recorded transcripts against a fake server. Sans-I/O design is the main lesson from imap-next and io-imap.
- `Store` has one writer thread with a typed command channel, and emits change events after commit. The UI reads via the read pool, matching Mailspring's emit-after-commit and Panorama's LiveView concepts.
- `Op queue`: local apply and enqueue in one transaction, remote apply by the worker, rollback on permanent failure.
- `Backend` trait at the sync level, not the protocol level, so JMAP, Graph or Gmail API can be added later without touching storage.
- `Indexer` is a background consumer of the change feed that batches FTS commits.

Suggested build order for a solo developer: schema and store; read-only IMAP with basic tier on one provider; bodies and MIME; threading; FTS; op queue; CONDSTORE, then QRESYNC; IDLE; Gmail labels; quirk table. Start tests against a local Dovecot and Stalwart server plus real Gmail and Outlook accounts.

Why Rust remains primary: no runtime footprint, and mature parser, SQLite and search crates. The main weakness is the missing mature high-level IMAP client. MailKit removes that gap, so if time to market matters more than the last 30 to 50 MB of RSS, a .NET Native AOT engine is the serious alternative. That RSS difference is my estimate and needs measuring.

### Gaps
- No full reference open-source Rust desktop mail engine with a permissive licence exists that I could find; expect original work on the sync layer.
- JMAP as a first-class protocol was not assessed in depth (only noted as present in himalaya, meli and mailcore2).
- Maintenance risk: several key crates (imap-codec, mail-parser, keyring ecosystem) each appear to depend on one main maintainer. I did not quantify contributor counts.
