# Minimal Desktop Email Client: Feature Triage, Scope, Phasing and Lessons (state as of November 2026)

Evidence labels used in these notes:
- **[DOC]**: vendor documentation, advisory, or a primary statement.
- **[PRESS]**: trade press or a measured third-party test.
- **[VENDOR-CLAIM]**: a marketing or self-reported claim, not independently verified.
- **[ANECDOTE]**: individual user opinion (forum, review, comment).
- **[COMPETITOR]**: written by a rival email vendor, so biased.

Anything under "Inferences" is my judgement, not a sourced fact. That includes the triage lists, performance budgets and phase plan.

## What do users of minimal or fast clients actually rely on?

### Takeaway
The clients people praise for speed and focus share a small core: local data, fast search, keyboard control, and reliable sync. They also tend to win by narrowing scope, such as Gmail-only, IMAP-only or Markdown-only compose. Bloat complaints in 2026 centre mostly on latency and memory (new Outlook), then on ads and unwanted AI, so "core only" is a credible position, though a crowded one.

### Cited Findings

**What is in and out, per client**
- **Mimestream** [DOC]: at 1.0 (May 2023) it had labels, inbox categories, server-side filters, calendar invite responses, snooze, Gmail search, tracking prevention, vacation responses, Unified Inbox, profiles, templates, aliases, undo send, send-and-archive, synced signatures, push notifications, keyboard shortcuts and gestures. It uses the Gmail API rather than IMAP — [Mimestream launch post](https://mimestream.com/blog/mimestream-launches).
- **Mimestream scope** [DOC]: its FAQ says it currently supports only Gmail. Microsoft 365/Outlook, IMAP and JMAP are listed as planned, with no dates (FAQ undated) — [Mimestream FAQ](https://mimestream.com/faqs).
- **MailMate** [DOC]: describes itself as "an IMAP email client for macOS" with "Markdown integrated email composition", "extensive keyboard control", and "advanced search conditions and drill-down search links". It also says it "is not for everyone" — [Freron](https://freron.com/). The page does not advertise HTML composition, and I did not confirm from a primary source that it is absent.
- **MailMate release status**: Michael Tsai noted no official release since early 2023, only experimental builds (written October 2024) — [Michael Tsai](https://mjtsai.com/blog/2024/10/24/postbox-discontinued). The site still offers a beta as the current download — [Freron](https://freron.com/).
- **Apple Mail** [PRESS]: since macOS Ventura, Undo Send is on by default at 10 seconds, configurable to Off, 10, 20 or 30 — [MacRumors](https://www.macrumors.com/how-to/change-undo-send-delay-apple-mail).
- **Thunderbird** [PRESS]: 2026 priorities are finishing Exchange support, a new SQLite-based database ("Panorama"), and a calendar UI refresh, so it is a full PIM rather than a minimal client — [Phoronix](https://www.phoronix.com/news/Thunderbird-Roadmap-2026); [Thunderbird roadmap](https://roadmaps.thunderbird.net/en-CA/desktop).
- **Geary** [ANECDOTE, undated]: a forum user noted no address book and no sorting of mail into folders, and a reply called it "very basic" — [BunsenLabs forum](https://forums.bunsenlabs.org/viewtopic.php?id=8259). Distros still package 46.0 in 2026 — [Arch](https://archlinux.org/packages/extra/x86_64/geary/).
- **aerc** [ANECDOTE/descriptive]: Vim-style keys and an ex-command system. It fetches only what the interface needs, and its asynchronous IMAP stops the UI freezing on a poor connection — [Pyra forum post](https://pyra-handheld.com/boards/goto/post?id=1456076).
- **Claws Mail** [DOC]: HTML rendering is delegated to Dillo or a WebKit "Fancy" option rather than built in, and external commands can filter message text — [Claws README](https://git.disroot.org/gfgit/claws-mail/src/commit/fd2ef9c3249b8555585d08e8f7ee4bbed8c47534/README).
- **Fastmail** [DOC]: its first official desktop app launched in October 2025 with offline read, reply and calendar, and changes sync on reconnect — [Fastmail blog](https://www.fastmail.com/blog/desktop-app/). Offline must be enabled per device, and search may behave differently offline — [Fastmail help](https://www.fastmail.help/hc/en-us/articles/11517883953039). One guide describes it as Electron wrapping the PWA [COMPETITOR] — [Mailbird](https://www.getmailbird.com/best-fastmail-app-macos/).
- **HEY** [PRESS, mostly 2020-21]: the Screener holds first-time senders for a one-time allow or block decision — [Impact](https://www.impactplus.com/blog/basecamp-new-email-service-hey). It proxies all images and flags trackers — [Signal v. Noise](https://signalvnoise.com/svn3/spy-pixels-are-evolving-like-malware-so-heys-adapting/).
- **Superhuman** [VENDOR-CLAIM]: every action has a shortcut, Cmd+K command palette, local database, plus Snippets, Split Inbox and Remind Me — [Superhuman blog](https://blog.superhuman.com/superhuman-is-built-for-speed/).
- **Mailspring** [mixed]: free core with optional Pro at about $8/month that adds tracking and analytics — [AlternativeTo](https://alternativeto.net/software/mailspring/about). A mandatory Mailspring ID was criticised and later dropped [ANECDOTE] — [HN mirror](https://brios-git-crit-brianlovin.vercel.app/hn/42915586).
- **eM Client** [DOC, quoted]: positions itself as combining email, calendars, tasks, contacts, notes and chat — [Michael Tsai](https://mjtsai.com/blog/2024/10/24/postbox-discontinued).
- **Spark** [PRESS/DOC]: AI features since Spark 3 and a "My Writing Style" assistant — [MacRumors](https://www.macrumors.com/2024/02/22/spark-mail-ai-writing-assistant). Its privacy policy was updated on May 19, 2026 to add AI details — [Spark privacy](https://sparkmailapp.com/ja/legal/privacy-app). Data-collection concerns were reported in 2023 — [AppleInsider](https://appleinsider.com/articles/23/01/13/spark-mail-211-review-e-mail-organizer-with-gatekeeper-smart-inbox).
- **Canary** [COMPETITOR]: described as PGP plus on-device AI — [Dove](https://dove.email/blog/best-spark-mail-alternatives-2026).

**Bloat complaints**
- **New Outlook, memory** [PRESS]: 490-636 MB at idle against 117-148 MB for classic in Windows Latest's 2026 testing, typically around 10 processes, with one test showing the WebView2 element above 1.5 GB — [Windows Latest](https://www.windowslatest.com/2026/06/15/microsofts-new-outlook-takes-10-seconds). Office Watch reports looser figures of a gigabyte or more — [Office Watch](https://office-watch.com/2026/new-outlook-slow-memory-hog/).
- **New Outlook, latency** [PRESS]: clicking a notification can take about 10 seconds to show the message, where classic is near-instant (June 2026) — [Windows Latest](https://www.windowslatest.com/2026/06/15/microsofts-new-outlook-takes-10-seconds).
- **New Outlook, sentiment** [ANECDOTE]: users on Microsoft's own forum call it slow, particularly opening windows, and some revert to classic — [Tech Community](https://techcommunity.microsoft.com/discussions/outlookgeneral/new-outlook/4525890/replies/4526671). Ads are a repeated grievance, but the evidence is opinion, not a confirmed 2026 policy — [Q&A thread](https://learn.microsoft.com/en-us/answers/questions/4655721/new-outlook-memory-usage?page=5).
- **Anti-AI positioning** [VENDOR-CLAIM]: Marco's maker says every client launching now leads with AI and "most of it is noise" (bootstrapped at $8/month) — [Show HN: Marco](https://hn.jakechampion.name/item/47519302).
- **Minimal positioning** [VENDOR-CLAIM + ANECDOTE]: Skim says "no calendar, no rules engine, no bloat", and commenters praised opt-in AI, the small installer and fast startup — [Product Hunt](https://www.producthunt.com/p/skim-5/skim-7).

### Inferences
- The recurring must-keep set is local store, instant search, full keyboard control, reliable multi-account sync and tracking protection. Calendars, AI, tasks, chat and snooze sit outside that set, even though several clients ship them.
- Every praised minimal client gave something up on purpose. Mimestream's Gmail-only stance, years after launch, is the strongest signal of how expensive provider breadth is. Three providers from day one is a heavier scope than Mimestream took on with a team of five.
- "No AI, no bloat" is not differentiation on its own, as Marco, Skim, Talanoa and others all claim it. On Windows, the measurable gap against new Outlook (memory, notification-to-message time) is the stronger pitch.

### Gaps
- No quantitative survey of which features users actually use. Everything here is revealed preference from feature lists plus anecdote.
- Thin evidence on BlueMail, and no independent evidence for eM Client or Canary. Sources on Spark and Canary in 2026 are dominated by rival vendors.
- I could not confirm from primary docs: Geary's offline behaviour, aerc's HTML handling, or MailMate's lack of HTML compose.
- A claim that Microsoft delayed the new Outlook opt-out deadline to March 2027 appeared in a search summary, but I did not trace it to a source. Treat as unverified.

## Which features look optional but are actually table stakes?

### Takeaway
Most items on the brief's list are table stakes, because their absence causes visible failure in daily use or breaks interoperability. The genuine candidates for deferral are rich HTML compose beyond basics, printing polish, and spam-training depth. The triage below is my judgement, based on feature-list overlap and failure reports.

### Cited Findings
- Undo send is now a default in mainstream clients: Apple Mail ships it on at 10 seconds — [MacRumors](https://www.macrumors.com/how-to/change-undo-send-delay-apple-mail) — and Mimestream listed it at 1.0 — [Mimestream](https://mimestream.com/blog/mimestream-launches).
- Unified Inbox, synced signatures, aliases, push notifications and keyboard shortcuts were all part of Mimestream's 1.0 list — [Mimestream](https://mimestream.com/blog/mimestream-launches).
- Offline is treated as a headline feature even by a webmail-first provider — [Fastmail](https://www.fastmail.com/blog/desktop-app/).
- Sync gaps drive abandonment [ANECDOTE]: Airmail reviewers cite folders and sent messages not syncing across devices, hangs when switching folders, and unanswered bug reports — [Trustpilot](https://nz.trustpilot.com/review/airmailapp.com).
- The default handler matters because other apps rely on `mailto:` to launch the user's default mail app — [Microsoft Learn](https://learn.microsoft.com/en-us/previous-versions/windows/apps/hh779673(v=win.10)).
- Plain-text-first clients remain viable for a niche, with Claws Mail and aerc listed as handling plain text well — [useplaintext.email mirror](https://hn.jakechampion.name/reader/https://useplaintext.email).

### Inferences

**Must-have (v1 is not usable without these)**
- Multiple accounts across Google, Microsoft and Apple, with a unified inbox.
- Reliable two-way sync of read state, flags, moves, deletes and archive.
- Threaded conversation view, with a way to view messages individually.
- Reply, reply-all and forward with correct quoting, `In-Reply-To`/`References` headers, and correct attachment handling on forward.
- Drafts that autosave locally and sync to the server, without leaving duplicates.
- Attachments: send, receive, save, open with a warning, plus drag and drop.
- Safe HTML rendering with remote images blocked by default, and a per-sender allow.
- Compose with basic formatting (bold, italic, links, lists, quoted text) and a plain-text option. Replying to HTML mail with plain text only is viable for a niche but is not a mainstream default.
- Signatures, per account and per alias.
- Send-as/aliases, because many Google and Microsoft users depend on them.
- Local full-text search, with server search as a fallback for unsynced history.
- Full keyboard control, plus a shortcut cheat sheet.
- Undo send. It is cheap to build as a client-side delay and is expected.
- Native Windows notifications that open the exact message.
- Read and triage offline, with an outbox that queues mail.
- `mailto:` handling.
- Report spam and not-spam, at minimum moving mail to and from the provider's Junk folder.
- Printing a message. It is cheap, and it is a sharp failure when missing for receipts and tickets.
- Dark mode, high-DPI support, and basic accessibility. A Postbox commenter left over accessibility — [Michael Tsai](https://mjtsai.com/blog/2024/10/24/postbox-discontinued).

**Should-have later**
- Saved searches and server-side filter viewing.
- Snooze and send later. Neither is core, and both need a storage decision.
- Templates and snippets.
- Contacts autocomplete beyond a harvested address list. Basic autocomplete from seen addresses belongs in v1.
- One-click unsubscribe via `List-Unsubscribe`.
- Calendar invite accept and decline only, as Mimestream did, without a calendar.
- Rich compose extras (tables, inline images), vacation responder, PGP/S-MIME, import/export (mbox/eml), generic IMAP providers, localisation, a command palette.

**Deliberately excluded**
- AI summaries, drafting or triage.
- Calendar, tasks, notes and chat.
- Read receipts and sender-side open tracking. Excluding these matches the privacy position, and Mailspring's tracking was reported as unreliable [ANECDOTE] — [HN mirror](https://brios-git-crit-brianlovin.vercel.app/hn/42915586).
- A mandatory cloud account or vendor backend, plus ads and upsell prompts inside the mail UI.
- Plugin systems, POP3, Exchange on-premises and local rule engines, at least initially.
- Social or sender-profile enrichment.

### Gaps
- I found no source ranking these features by usage or churn impact.
- Spam reporting behaviour differs by provider. I did not research whether moving to Junk trains each provider's filter.
- The `References`/threading algorithm and drafts-sync mechanics were not sourced here. They need provider-level research.

## Security and privacy baseline

### Takeaway
The vulnerability record shows the HTML body, its CSS, SVG, link targets and attachment previews are the main attack surface, and any network request made during rendering is a leak. Treat every message as hostile: render in a locked-down sandbox with no scripts and no network, and never hand message content to OS parsers implicitly.

### Cited Findings
- **EFAIL (2018)**: direct exfiltration abuses an unclosed `img src` spanning MIME parts, so decrypted plaintext is sent in a URL. Apple Mail, iOS Mail and Thunderbird had flaws enabling it. Exfiltration channels existed in 25 of 35 S/MIME clients and 10 of 28 OpenPGP clients, and bypasses were published after disclosure — [efail.de](https://efail.de/).
- **EFAIL mitigation**: disabling HTML rendering closes the most prominent channel, though harder backchannels may exist. Attackers can also convert plain-text encrypted mail to HTML — [efail.de](https://efail.de/).
- **CVE-2026-0818 (Thunderbird, 2026)**: the CSS of an outer HTML email could exfiltrate decrypted inline OpenPGP text when remote content was allowed. Fixed in 147.0.1 and 140.7.1, CVSS 4.3 in NVD-derived entries — [Red Hat Bugzilla](https://bugzilla.redhat.com/show_bug.cgi?id=2433720); [OpenCVE](https://app.opencve.io/cve/CVE-2026-0818).
- **Roundcube SVG XSS**: CVE-2023-5631 and CVE-2024-37383 came from crafted SVG, and CVE-2025-68461 from the SVG animate tag. Aggregators list all three as CISA KEV, which I did not check against CISA directly — [PT Security](https://dbugs.ptsecurity.com/vulnerability/CVE-2025-68461).
- **Roundcube sanitiser bypasses**: CVE-2025-68460 bypassed CSS/HTML filtering in the style sanitiser, enabling data exfiltration — [Feedly](https://feedly.com/cve/CVE-2025-68460). CVE-2026-48848 is CSS injection through SVG animate elements — [CIRCL](https://vulnerability.circl.lu/vuln/cve-2026-48848).
- **Roundcube non-obvious paths**: CVE-2026-35539 is XSS in HTML attachment preview — [NVD](https://nvd.nist.gov/vuln/detail/CVE-2026-35539). CVE-2026-54433 is stored XSS from a crafted *plain-text* email, fixed in 1.6.17 and 1.7.2 — [cvelogic](https://www.cvelogic.com/vendor/roundcube).
- **CVE-2024-21413 "MonikerLink" (Outlook)**: a `file://` link with a `!` suffix bypassed Protected View and leaked NTLM credentials. CVSS 9.8, patched February 2024, listed as exploited — [inthewild](https://inthewild.io/vuln/CVE-2024-21413); [Strobes](https://strobes.co/vi/cve/CVE-2024-21413/).
- **CVE-2023-23397 (Outlook)**: an elevation-of-privilege flaw allowing NTLM credential theft, disclosed March 2023 — [Critical Start](https://criticalstart.com/microsoft-outlook-zeroday-exploited-in-the-wild).
- **CVE-2025-21298 (Windows OLE)**: a memory flaw in ole32.dll reachable through crafted RTF, CVSS 9.8, patched January 2025, reportedly triggered by previewing in Outlook. Censys says the flaw is in Windows rather than Outlook itself, so scope reports conflict — [Censys](https://censys.com/advisory/cve-2025-21298/); [Cyber Security News](https://cybersecuritynews.com/critical-windows-ole-zero-click-vulnerability).
- **Tracking pixels**: HEY proxies all images and uses heuristics, began with 50+ named services, and spent a day adding detection after GMass published evasion techniques. DHH calls this "a constantly moving target" — [Signal v. Noise](https://signalvnoise.com/svn3/spy-pixels-are-evolving-like-malware-so-heys-adapting/).
- **Apple Mail Privacy Protection** [secondary sources only]: described as preloading remote content through two relays, which makes open tracking unreliable but does not stop click tracking — [gblock](https://www.gblock.app/articles/apple-mail-privacy-protection-fake-email-opens-2026).
- **Third-party audit burden**: Marco's author says Google's CASA audit is an annual requirement because of the restricted email scope — [Show HN: Marco](https://hn.jakechampion.name/item/47519302). The OAuth researcher should confirm details and cost.

### Inferences

**Baseline for v1**
- Remote content is off by default, with no prefetching. Allow per sender or per message, with a clear banner. Blocking at the network layer of the renderer is safer than rewriting HTML, because CSS `url()`, `@import`, fonts, SVG and `srcset` all trigger requests.
- Render in a sandboxed, script-disabled view with a strict CSP and no same-origin access to app state, plus an allowlist sanitiser. Use two layers, because Roundcube's record shows sanitisers alone get bypassed repeatedly.
- Drop or rasterise SVG, and strip `<style>` constructs that can select on or exfiltrate content. Do not render attachments inline as HTML.
- Treat plain text as hostile too: escape it, and linkify conservatively.
- Allow only `http`, `https` and `mailto` link schemes. Block `file:`, UNC paths and custom schemes outright. This one rule addresses the MonikerLink class and NTLM leaks.
- Show the real destination before opening a link, and flag mismatches between display text and target, punycode, and mixed-script domains.
- Never auto-open attachments. Warn on executable types, write them with Mark-of-the-Web so SmartScreen applies, and avoid Windows preview handlers and OLE/RTF parsing in the app.
- Keep decrypted parts isolated if PGP/S-MIME arrives later. EFAIL and CVE-2026-0818 both come from mixing encrypted content with surrounding HTML.
- Do not build your own image proxy: running a server undermines the no-backend position, and blocking by default gives comparable protection for zero cost.
- Set parser limits for depth and part counts, as mail-parser does, to prevent resource exhaustion.
- Store tokens in Windows Credential Manager/DPAPI rather than config files, and decide on database encryption explicitly. The stack and OAuth researchers own the specifics.
- Ship a fast security-update path. Mail renderers need patching quickly, which affects the update mechanism choice.

### Gaps
- No dedicated source on phishing link display practice across clients. The rules above are derived from CVE lessons.
- KEV status for the Roundcube CVEs comes from aggregators and was not confirmed in CISA's catalogue.
- No independent, academic tracking-pixel prevalence figure. The only number found, attributed to Stripo, could not be traced to its original.
- Apple's own Mail Privacy Protection docs were not retrieved.
- No confirmation of current exploitation for CVE-2025-21298, or of any 2026 Outlook preview-pane CVE.

## Perceived performance: what makes email feel instant, and sensible budgets

### Takeaway
"Instant" comes from architecture, not micro-optimisation: every user action reads and writes a local store, the network runs in the background, likely-next messages are pre-rendered, and the keyboard removes pointer time. Target under 100 ms for interactions, as a measured budget enforced in CI.

### Cited Findings
- 0.1 s is about the limit for feeling instantaneous, 1 s for uninterrupted flow of thought, and 10 s for holding attention (1993, updated 2014) — [Nielsen Norman Group](https://www.nngroup.com/articles/response-times-3-important-limits/).
- Superhuman [VENDOR-CLAIM]: every interaction should be under 100 ms, aiming for under 50 ms where possible. A quoted tweet claims a renderer showing email in 1-2 frames (under 32 ms) — [Superhuman blog](https://blog.superhuman.com/superhuman-is-built-for-speed/).
- Superhuman techniques [VENDOR-CLAIM]: continuous measurement, a local email database, preloading and prerendering likely-next threads, minimal animation, shortcuts for everything, in-UI shortcut hints, and a command palette — [Superhuman blog](https://blog.superhuman.com/superhuman-is-built-for-speed/).
- Superhuman's 2017 Fluent talk covered "managing optimistic updates, syncing, and performance on a single thread" in an offline-first client — [O'Reilly Fluent](https://conferences.oreilly.com/fluent/fl-ca-2017/public/schedule/detail/57884). A summary says it modelled updates as a series of changes and tracked those not yet applied to the server; I only have the abstract, not the talk.
- Superhuman's comparison page lists figures of about 100 ms for itself against 32 ms for Gmail, which are inconsistent with its speed claims, so I would not cite either — [Superhuman](https://blog.superhuman.com/gmail-vs-superhuman/).
- Mimestream credits native Swift/AppKit and the Gmail API for speed [VENDOR-CLAIM] — [Mimestream](https://mimestream.com/blog/mimestream-launches); [MacRumors forum](https://forums.macrumors.com/threads/mimestream-a-native-gmail-client-for-macos-now-in-free-public-beta.2240443/).
- aerc keeps the UI responsive by fetching only what is needed and doing IMAP asynchronously — [Pyra forum post](https://pyra-handheld.com/boards/goto/post?id=1456076).
- Mailspring moved sync into a separate native process with SQLite, one process per account, and claims about half the RAM and CPU of Nylas Mail [VENDOR-CLAIM] — [Mailspring-Sync](https://github.com/Foundry376/Mailspring-Sync).
- Comparison baseline [PRESS]: new Outlook idles at 490-636 MB, classic at 117-148 MB, and new Outlook's notification-to-message time is about 10 s — [Windows Latest](https://www.windowslatest.com/2026/06/15/microsofts-new-outlook-takes-10-seconds).

### Inferences

**Design rules**
- Reads come only from the local store. Writes go to the local store plus a durable operation queue, and the UI never waits on the network.
- Optimistic actions need visible, recoverable failure: a failed archive must return the message with a notice, not silently vanish.
- Keep sync off the UI thread or in a separate process.
- Prefetch bodies for the top of the inbox and the next and previous messages, and pre-render the next message during triage.
- After an archive or delete, auto-advance to the next message.
- Virtualise the message list.
- Sync newest-first so the client is usable within seconds on a huge account, then backfill in the background.

**Proposed budgets (my proposals, not sourced standards)**

Measure on a named mid-range Windows 11 laptop with a seeded 100k-message mailbox, reporting p95.

| Metric | Target | Hard limit |
|---|---|---|
| Keypress or action to visual feedback | under 50 ms | 100 ms |
| Open cached message (plain or typical HTML) | under 50 ms | 100 ms |
| Switch folder or account | under 50 ms | 100 ms |
| Scroll message list | 60 fps | no frame over 50 ms |
| Local search, first results | under 100 ms | 250 ms |
| Open compose window | under 100 ms | 200 ms |
| Warm start to usable inbox | under 300 ms | 500 ms |
| Cold start to usable inbox | under 1 s | 2 s |
| Notification click to message visible | under 300 ms | 1 s |
| Idle RAM, 3 accounts | under 150 MB | 250 MB |
| New mail arrival to notification (push path) | under 5 s | 30 s |
| First-run: account added to recent inbox visible | under 10 s | 30 s |

- The RAM target is set to match classic Outlook's reported idle use, about a quarter of new Outlook's. Whether it is achievable depends on the stack, particularly if a web renderer is embedded, so the stack researcher should validate it.
- Test at 100k and 1M messages. Add automated timing to CI from the first milestone, and treat a regression as a build failure. Superhuman's "measure first" point is the part of its playbook that transfers directly.

### Gaps
- No published engineering deep-dive from Mimestream on performance, and no measured third-party benchmark of Superhuman or Mimestream.
- No measured cold-start numbers for any client. An ITHome figure of about 5 s for new Outlook could not be verified.
- No independent benchmark of Mailspring's RAM claim; user reports of battery and resource drain conflict with it [ANECDOTE] — [AlternativeTo](https://alternativeto.net/software/mailspring/about).

## Project failure and success histories: what took longest, what is underestimated

### Takeaway
Few of the well-known deaths came from pure engineering failure: they came from acquisition, strategy shifts, thin economics, or a single owner running out of capacity. The engineering lesson is that sync reliability takes years rather than months. Mimestream spent over two years in public beta with one provider before charging, growing from one person to five along the way.

### Cited Findings
- **Mimestream**: public beta from about September 2020 — [MacRumors](https://macrumors.com/2020/09/10/mimestream-native-gmail-client-mac) — to 1.0 on May 22, 2023. The launch post cites a beta of over two years, 220+ updates, 2,500+ improvements, 100+ new features, and growth from one founder to five. Pricing was $49.99/year or $4.99/month — [Mimestream](https://mimestream.com/blog/mimestream-launches). Subscription pricing drew objections [ANECDOTE] — [MacRumors forum](https://forums.macrumors.com/threads/former-apple-engineers-mimestream-gmail-client-for-mac-gets-official-launch-free-beta-access-to-end-tomorrow.2390478).
- **Nylas Mail to Mailspring**: Nylas sunset the client and re-licensed it under MIT, with 2.0.31 the last release — [GitHub](https://www.github.com/nylas/nylas-mail/releases/latest). The stated driver was focus on the growing API business (2017, third-party summary) — [Plushcap](https://www.plushcap.com/content/nylas/blog/nylas-sunsetting-nylas-mail-development).
- **Mailspring rewrite**: one original developer continued the project and replaced the JavaScript sync with a C++ engine on MailCore2 — [GitHub](https://www.github.com/nylas/nylas-mail/releases/latest); [LinuxLinks](https://www.linuxlinks.com/mailspring-new-version-nylas-mail/). Older sources call the engine closed source, while its repo is now public under GPLv3 per later sources, so check before relying on either — [Mailspring-Sync](https://github.com/Foundry376/Mailspring-Sync).
- **Sparrow**: acquired by Google in 2012, the Mac app never updated after that, and both apps were pulled in February 2015 — [TechCrunch](https://techcrunch.com/2015/02/17/google-sparrow/).
- **Mailbox**: bought by Dropbox in 2013 for a reported $100M and shut down February 26, 2016. Dropbox refocused on its core product, and App Annie data showed no sustained popularity — [TechCrunch](https://techcrunch.com/2015/12/07/failing-to-find-users-dropbox-will-shut-down-mailbox-in-february-2016-and-carousel-in-march). The team said its advances had spread to other apps — [TechRadar](https://www.techradar.com/news/software/dropbox-is-dropping-mailbox-because-everyone-copied-it-1310708).
- **Postbox**: reported acquired by eM Client and shut down, October 2024. It was built on Thunderbird. Commenters cited no Apple Silicon-native Postbox 7 after about four years of waiting [ANECDOTE] — [Michael Tsai](https://mjtsai.com/blog/2024/10/24/postbox-discontinued). Vendor backup support has since ended — [Druva](https://help.druva.com/en/articles/14450848-faq-end-of-life-eol-for-legacy-email-clients).
- **Newton**: subscription model from 2016, shutdown announced for 2018, revived by Essential, ended again when Essential closed in 2020, then revived by a third owner in May 2020 — [Android Central](https://www.androidcentral.com/newton-mail-dies-second-time-following-essential-closure); [MacRumors](https://www.macrumors.com/2020/05/11/cross-platform-newton-mail-service-to-continue). Sources disagree on acquisition timing and owners.
- **Airmail**: moved iOS users to subscription in July 2019, and users said paid features went behind it — [MacRumors](https://www.macrumors.com/2019/07/15/airmail-subscription-based-pricing). Later reviews cite sync gaps and unanswered bugs [ANECDOTE] — [Trustpilot](https://nz.trustpilot.com/review/airmailapp.com).
- **Geary**: a 2016 Fedora report called it dead upstream, was closed NOTABUG, and Michael Gratton applied to take over maintenance that March — [Red Hat Bugzilla](https://bugzilla.redhat.com/show_bug.cgi?id=1321337). Single-maintainer dependence is the visible risk, and I found no 2026 maintainer statement.
- **Thunderbird**: years of work to replace the Mork storage layer, whose limitations include issues with threaded conversations. Rust was introduced through Exchange support — [Thunderbird blog](https://blog.thunderbird.net/tag/exchange/); [FOSDEM 2024](https://fosdem.org/schedule/event/fosdem-2024-2469-thunderbird-how-to-exchange-rot-for-rust). The 2026 plan builds the new SQLite store in parallel with the existing database, and no Panorama release version was found — [Thunderbird roadmap](https://roadmaps.thunderbird.net/en-CA/desktop); [Heise](https://www.heise.de/en/news/What-Thunderbird-users-can-expect-in-2026-database-overhaul-and-iOS-app-11120751.html?view=print).
- **Marco (2026)**: tried WatermelonDB, Triplit, InstantDB, PowerSync and Replicache before falling back to plain HTTP endpoints with TanStack DB/Query, at hundreds of thousands of entities per user. The author's verdict: "IMAP can truly be a pain in the ass" — [Show HN: Marco](https://hn.jakechampion.name/item/47519302).

**Underestimated details**
- **Sent duplicates**: Gmail copies anything sent through its SMTP into Sent, which is non-standard, so a client that also APPENDs gets duplicates — [MailMate list](https://lists.freron.com/mailmate/2014-August/002969.html). Microsoft documents the same pattern for some providers in IMAP Outlook — [Microsoft Support](https://support.microsoft.com/en-us/topic/imap-sent-messages-are-duplicated-and-unread-in-sent-items-folder-f9cb7d98-b6be-4740-89f8-3b7c2277a615).
- **Missing Message-ID**: duplicates also appear when the client relies on the SMTP server to add a Message-ID, and a Gmail team response notes APPENDed messages can end up with different IDs — [GNOME Bugzilla](https://bugzilla.gnome.org/show_bug.cgi?id=599230). These sources are old (about 2010-2018), so verify current behaviour.
- **Labels versus folders**: Gmail exposes labels as IMAP folders, so one message appears in several, including All Mail — [MailMate list](https://lists.freron.com/mailmate/2014-August/002975.html); [Thunderbird bug 739110](https://bugzilla.mozilla.org/show_bug.cgi?id=739110).
- **Special folders**: other clients can create fake "Sent" folders, and localised names vary — [Mozilla bug 1134987](https://bugzilla.mozilla.org/show_bug.cgi?id=1134987).
- **Bandwidth limits**: Workspace IMAP limits are 2,500 MB/day download and 500 MB/day upload, and multiple IMAP clients multiply use. Google also recommends limiting labels (500 as best practice) — [Google Workspace](https://knowledge.workspace.google.com/admin/gmail/gmail-bandwidth-limits). Initial sync of a large mailbox must therefore throttle and resume. Consumer limits may differ.
- **MIME and encoding**: mail-parser enforces depth and part limits, reports malformed transfer encodings, and handles dozens of charsets, which shows the scale of the edge-case surface — [mail-parser](https://github.com/stalwartlabs/mail-parser). The eml-codec project has an open effort to improve parser recovery based on real-world emails — [eml-codec PR 53](https://git.deuxfleurs.fr/Deuxfleurs/eml-codec/pulls/53).

### Inferences

**What kills projects**
- Provider breadth before one provider is reliable.
- Building on a codebase you cannot move. Postbox's Thunderbird base and Thunderbird's own Mork both show how expensive storage and platform foundations become.
- Choosing a slow sync architecture early. Nylas to Mailspring needed a full engine rewrite.
- Over-engineered sync frameworks when a queue plus simple state would do, per Marco.
- Weak economics: a price tag is hard to add later (Airmail backlash), while free clients depend on someone's spare time.
- Single-maintainer burnout, where unanswered bugs erode trust fast.
- Data loss or duplicate-sends. For a mail client, one such bug destroys trust in a way a missing feature does not.

**Phasing proposal (a judgement call)**

Durations are rough estimates for one developer. Mimestream's history suggests they may be optimistic, so treat exit criteria as the real gate.

| Phase | Scope | Exit criterion |
|---|---|---|
| 0: Spike (4-8 weeks) | Read-only sync of one real Gmail account into a local store, plus MIME parse, sanitised render, and a performance harness | Your own mailbox loads and message open meets budget |
| 1: Daily driver, one provider (4-6 months) | Gmail: triage actions, operation queue, compose, reply, drafts, send, attachments, search, keyboard | You use only this client for 30 days with no data loss |
| 2: Second and third providers (4-6 months) | Microsoft, then iCloud; multi-account, unified inbox, notifications | All three pass the same conformance suite; each used daily for 30 days |
| 3: Private beta (3-6 months) | `mailto:`, printing, undo send polish, updates, signing, crash reporting (opt-in), migrations | 20-50 external users, no open data-loss bugs |
| 4: Public 1.0 | Store listing, docs, pricing decision | Clean upgrade path, security update process tested |
| 5: Later | Should-have list, driven by actual requests | Each addition must fit performance budgets |

- Build the provider abstraction after the second provider exists, not before, but keep the operation queue and local store provider-neutral from the start.
- Design schema migrations, sync-state versioning and "resync from scratch without data loss" in Phase 0-1.
- Keep a written "will not build" list, published if the project goes commercial. It protects scope and is itself the marketing message.
- Commercial option: keep a local-only architecture with no backend, charge for a licence or subscription from the first paid release, and do not move paid features later. Budget for any recurring Google security assessment before committing to a commercial Gmail path.

### Gaps
- No formal postmortem from Nylas, and no first-hand account of how long Mailspring's engine took. Mimestream's development start date and which parts took longest are not public in what I found. A [Mac Power Users episode](https://chartable.com/podcasts/mac-power-users/episodes/167518399-all-about-mimestream-with-neil-jhaveri) (October 2024) may help, but I only saw its summary.
- eM Client's and Postbox's own statements were not read directly. The Postbox account comes via Tsai and TidBITS.
- Newton's status after 2020 is unknown.
- Sent-folder evidence is old and needs retesting against current Gmail, Outlook.com and iCloud behaviour. I found nothing current on iCloud or Outlook.com IMAP quirks.

## Testing: servers, fixtures, and real-provider strategy

### Takeaway
Use layers: a parser corpus with golden outputs, deterministic local servers in CI, a recorded provider-quirk suite, and a small nightly run against real accounts. The tools exist individually, but nobody publishes a combined recipe, so expect to assemble it yourself.

### Cited Findings
- **GreenMail**: SMTP, POP3 and IMAP test servers with SSL, usable embedded, standalone, or in Docker — [GreenMail](https://greenmail-mail-test.github.io/greenmail).
- **Dovecot in Docker**: `docker-test-dovecot` generates random mail in test accounts and wipes Maildirs on startup — [docker-test-dovecot](https://awesome.ecosyste.ms/projects/github.com%2Fspezifisch%2Fdocker-test-dovecot).
- **imaptest**: a generic IMAP compliance tester with stress testing with state tracking, scripted testing and benchmarking, available as `dovecot/imaptest` — [GitHub](https://github.com/dovecot/imaptest); [Dovecot docs](https://doc.dovecot.org/admin_manual/testing/running_imaptest). A community image notes no TLS support, so stunnel is needed — [Docker Hub](https://hub.docker.com/r/jheiss/imaptest). imaptest tests servers rather than clients, but its scripted tests and benchmarks are useful for building a fixture server.
- **Stalwart**: a Rust server with SMTP, IMAP, JMAP, Sieve, CalDAV and CardDAV [ANECDOTE description] — [PieFed](https://piefed.keyboardvagabond.com/post/229377).
- **mail-parser corpus**: messages compared against expected JSON in LF and CRLF forms, every function fuzzed, Miri runs on decoders, and a claim of "millions of real-world e-mail messages" [VENDOR-CLAIM] — [mail-parser](https://github.com/stalwartlabs/mail-parser). Test directories are named rfc, legacy, thirdparty and malformed — [docs.rs](https://docs.rs/crate/mail-parser/0.6.1/source/benches/libraries.rs).
- **Elixir `email_parser`**: vendors 95 real-world messages, including malformed ones, each with golden attachment expectations, derived from mail-parser's corpus — [hex.pm](https://hex.pm/packages/email_parser/0.1.0/files).
- **Other corpora**: Enron is used in mail-parser benchmarks — [mail-parser](https://github.com/stalwartlabs/mail-parser). `mirage/hamlet` provides generated mail whose headers are looser than the RFC allows — [eml-codec](https://git.deuxfleurs.fr/Deuxfleurs/eml-codec/src/tag/0.4/resources/hamlet). MIMEminer is another MIME-related project, not examined — [GitHub](https://github.com/MIME-miner/MIMEminer).

### Inferences

**Test pyramid**
1. **Parser and render**: corpora above, plus your own anonymised failures. Golden output for the parse tree, and a sanitiser test that asserts zero network requests from a hostile HTML set built from the CVEs in this document. Fuzz parser and sanitiser continuously.
2. **Sync state machine**: a fake server in-process, with fault injection for dropped connections, mid-batch failures, reused or changed UIDVALIDITY, throttling, and duplicate delivery. Property-based testing fits well: random operation sequences must converge to the server state.
3. **Protocol integration in CI**: GreenMail for fast runs, Dovecot as the reference IMAP, and Stalwart as a second implementation and a JMAP target.
4. **Provider emulation**: scripted fakes reproducing observed quirks (auto-saved Sent, labels as folders, throttling). Local servers will not reproduce them, so this layer has to be built and maintained by hand.
5. **Real providers, nightly**: dedicated test accounts on Gmail, Workspace, Outlook.com, Microsoft 365 and iCloud. Keep a small, scripted round-trip: send, receive, reply, move, delete, draft edit, attachment, non-ASCII subject and name. Respect rate limits, and check for duplicates in Sent and Drafts after every run.
6. **Dogfood**: your own real mailbox from Phase 1. Add a large synthetic account of 100k+ messages for performance. Keep a debug-only, redacted protocol log so user reports can be turned into fixtures.

- Licensing should be checked before vendoring any corpus, and real personal mail must never enter fixtures unanonymised.

### Gaps
- No published combination of GreenMail, Dovecot and Stalwart for malformed-MIME testing. No source on Stalwart's Docker image or its suitability as a test target was retrieved.
- No published strategy from a client vendor on testing against real providers. Layer 5 is my recommendation, and test-account terms of service should be checked.
- Licences for the corpora were not checked.

## Distribution on Windows: signing, packaging, updates, default handler

### Takeaway
The Microsoft Store is the cheapest route to no SmartScreen warnings: individual registration is free in nearly 200 markets, and Store-delivered apps never show the warning. Azure Artifact Signing starts at $9.99/month, but **individual developers must be in the US or Canada**, so the developer's location decides the direct-download path. No app can set itself as the default mail handler: Windows requires the user to do it in system UI.

### Cited Findings

**SmartScreen (Microsoft doc, updated August 2026)**
- Store apps are "never subject to SmartScreen download warnings" — [Microsoft Learn](https://learn.microsoft.com/en-us/windows/apps/package-and-deploy/smartscreen-reputation).
- "EV certificates no longer bypass SmartScreen", and paying for EV solely for this "is no longer justified" — [Microsoft Learn](https://learn.microsoft.com/en-us/windows/apps/package-and-deploy/smartscreen-reputation).
- A new signed app is still flagged until reputation accumulates. There is no exact threshold, but it "can take several weeks and hundreds of clean installs" — [Microsoft Learn](https://learn.microsoft.com/en-us/windows/apps/package-and-deploy/smartscreen-reputation).
- Certificate reputation can carry to new versions signed by the same identity, while unsigned files start from zero every update. A consistent signing identity is advised — [Microsoft Learn](https://learn.microsoft.com/en-us/windows/apps/package-and-deploy/smartscreen-reputation).
- There is no consumer submission mechanism to speed this up — [Microsoft Learn](https://learn.microsoft.com/en-us/windows/apps/package-and-deploy/smartscreen-reputation). The Artifact Signing FAQ does suggest submitting persistently flagged signed files to Microsoft Security Intelligence, which is a slight inconsistency between the two docs — [Artifact Signing FAQ](https://learn.microsoft.com/en-us/azure/artifact-signing/faq).
- On Windows 11, Smart App Control may supersede SmartScreen and "will block execution of unsigned files unless the file has a positive reputation" — [Microsoft Learn](https://learn.microsoft.com/en-us/windows/apps/package-and-deploy/smartscreen-reputation).

**Azure Artifact Signing (formerly Trusted Signing)**
- Starts at $9.99/month per Microsoft Learn — [Microsoft Learn](https://learn.microsoft.com/en-us/windows/apps/package-and-deploy/smartscreen-reputation). Tiers: Basic 5,000 signatures/month and Premium 100,000; the pricing page rendered without dollar figures for me. Billing starts on account creation and is not pro-rated — [Azure pricing](https://azure.microsoft.com/en-us/pricing/details/artifact-signing/).
- A blog's "effectively unlimited signings" claim conflicts with these quotas — [codenote](https://codenote.net/en/posts/windows-code-signing-ci-cost-per-signing-volume-2026/).
- **Eligibility (doc updated October 2026)**: Public Trust is available to organisations in the US, Canada, EU, UK, Australia, New Zealand, Japan, South Korea, Singapore, Switzerland, Norway and Israel. "Individual developers must be located in the United States or Canada" — [Quickstart](https://learn.microsoft.com/en-us/azure/artifact-signing/quickstart). A July 2026 blog said Japan was excluded, which conflicts with this; the later Microsoft doc should win — [codenote](https://codenote.net/en/posts/windows-code-signing-ci-cost-per-signing-volume-2026/).
- Individuals need an Azure billing account of type Individual, with legal name and address matching government ID. Validation runs through Verified ID and AU10TIX — [Quickstart](https://learn.microsoft.com/en-us/azure/artifact-signing/quickstart).
- Organisation validation takes 1-20 business days, possibly longer, and allows three document attempts — [Quickstart](https://learn.microsoft.com/en-us/azure/artifact-signing/quickstart).
- Free, trial and sponsored subscriptions are not supported. Custom CN/O is not possible, EV is not offered, and resources cannot move across subscriptions or tenants — [Artifact Signing FAQ](https://learn.microsoft.com/en-us/azure/artifact-signing/faq).

**Alternatives (July 2026, blog-reported prices, excluding tax)**
- SSL.com OV from $129/year and EV from $349/year, with eSigner from $20/month for 20 signings ($15 on annual commitment) — [codenote](https://codenote.net/en/posts/windows-code-signing-ci-cost-per-signing-volume-2026/).
- GlobalSign OV about $434/year, plus an Azure Key Vault Premium HSM key at about $5/month — [codenote](https://codenote.net/en/posts/windows-code-signing-ci-cost-per-signing-volume-2026/).
- Private keys must sit in FIPS 140-2 Level 2+ hardware or a cloud HSM since June 2023, so a plain file-based certificate is not an option — [codenote](https://codenote.net/en/posts/windows-code-signing-ci-cost-per-signing-volume-2026/).

**Microsoft Store**
- Individual registration is free with no credit card, using a government ID and selfie check, in "nearly 200 markets" (September 2025) — [Windows Developer Blog](https://blogs.windows.com/windowsdeveloper/2025/09/10/free-developer-registration-for-individual-developers-on-microsoft-store/).
- The Store does not re-sign MSI/EXE installers, so those must be signed with your own certificate. Commission is 15% for apps through Microsoft's commerce platform — [Microsoft Learn FAQ](https://learn.microsoft.com/en-my/windows/apps/publish/faq/get-started-with-the-microsoft-store).
- Direct Win32 update support was announced at Build 2025, rollout unconfirmed — [Neowin](https://www.neowin.net/amp/microsoft-store-finally-gets-last-updated-for-apps-direct-win32-updates-and-more/).
- Company fees: Microsoft's FAQ lists about $99 once, while later press says it was dropped, so check Partner Center — [Windows Central](https://www.windowscentral.com/software-apps/microsoft-store-drops-registration-fees-for-companies-as-part-of-onboarding-overhaul).

**Packaging and updates**
- Velopack generates an installer, delta updates and a self-updating portable package from one command, supports C#, C++, JS and Rust, and migrates Squirrel apps. Its speed and no-UAC claims are testimonials — [Velopack](https://github.com/velopack/velopack).
- MSIX offers native auto-update but can bring sandbox restrictions, and enterprises often control updates by policy [secondary] — [BestHub](https://besthub.dev/articles/choosing-the-right-windows-installer-msi-vs-exe-vs-msixbundle-4f55b75c896e).

**Default mail handler (doc updated September 2026)**
- "Windows does not allow programmatic changes to default apps without user interaction in system UI." Registry changes are unsupported, and UCPD.sys blocks apps from writing the data — [Default apps platform](https://learn.microsoft.com/en-us/windows/apps/develop/windows-integration/default-apps-platform).
- Supported route: use `ms-settings:defaultapps` to open Default Apps or your app's page there, prompt contextually, and avoid repeated prompts. Query state with `IApplicationAssociationRegistration::QueryCurrentDefault`. Windows prompts users automatically when a newly registered app could handle a type — [Default apps platform](https://learn.microsoft.com/en-us/windows/apps/develop/windows-integration/default-apps-platform).
- Store policy section 10.2.8 requires only supported methods for changing defaults — [Default apps platform](https://learn.microsoft.com/en-us/windows/apps/develop/windows-integration/default-apps-platform).
- Win32 registration uses Capabilities/`UrlAssociations` and `RegisteredApplications`. `Clients\Mail` still governs the default MAPI client. This page is dated 2018 and says it does not apply to Windows 10 as written, so use it only for key names — [Default Programs](https://learn.microsoft.com/en-us/windows/win32/shell/default-programs).

### Inferences
- **Decision hinge: where the developer lives.** If in the US or Canada, Artifact Signing as an individual is the cheapest direct-download option. Elsewhere, an individual cannot use it for Public Trust and needs either an eligible registered organisation or an OV certificate from a CA with a cloud HSM (about $180-$430+/year from the prices above). The certificate subject would show the developer's legal name, a consideration for a future commercial brand.
- **Recommended path**: an MSIX through the Store for most users, since it needs no paid certificate and avoids SmartScreen altogether. Add a signed direct download only if demand justifies it. Note that Store-delivered MSIX gives Microsoft-handled updates, which helps fast security patching.
- Check sandbox and capability limits early by building a Phase 0 MSIX, including `mailto:` activation, notifications, and the embedded renderer, before committing.
- A signed Velopack build is the fallback, and also the route to a portable edition. A portable build is low priority: it cannot cleanly register `mailto:` or use packaged notification identity, and SmartScreen still applies.
- Keep one signing identity from the first public release, because changing it costs accumulated trust. During private beta, warn testers about SmartScreen, or use Store private distribution if available.
- `mailto:` should be a first-run prompt shown once, opening Settings. Do not try to set it silently. Handle parameters in `mailto:` links defensively, for example ignoring `attach=`-type arguments, as a security measure (my recommendation, no source).
- Do not build a MAPI handler in v1. "Send to mail recipient" from Office and similar will not work, which is an accepted limitation until users ask.
- Budget (excluding Store route): about $120/year for Artifact Signing, or about $180-$430+/year otherwise.

### Gaps
- Live Artifact Signing prices did not render on Azure's page. The $9.99 figure comes from Microsoft Learn, and the Premium price and overage rates are unknown.
- The MSIX `mailto:` manifest declaration was not confirmed. The default apps doc shows only `uap:FileTypeAssociation`, though it says apps can register for link types; `uap:Protocol` should be checked in the manifest schema.
- Whether Store private or flighted distribution suits a beta, and the rollout status of direct Win32 updates, were not verified.
- No primary source on MSIX restrictions affecting a mail client, such as background sync at login or an embedded browser engine. The MSIX evidence here is weak and secondary.
- CA prices come from a single blog and may vary by region. The current CA/Browser Forum maximum certificate lifetime was not established.
- The user's country is unknown, and it affects signing eligibility.
