# Language and UI Toolkit for a High-Performance, Minimal-Footprint Desktop Email Client (as of November 2026)

Evidence labels used throughout:
- **[M]** measured, with at least some stated method
- **[A]** anecdotal measurement, no controlled method
- **[C]** vendor or project claim
- **[2nd]** reached through a secondary source, primary not opened

Source caveats that apply to the whole document:
- Version numbers and several dates come from one third-party research note, the Syrinx project wiki (dated 2026-10-09). It is a personal or small-project document, not a primary source. Treat those items as probably right, and confirm them against each project's release page before relying on them.
- No source compared these stacks head to head on Windows with one method. Every cross-stack comparison below is assembled from unrelated measurements.
- No benchmarks were run for this research.

## Ranked recommendation (objective)

### Takeaway
Rank 1 is Rust with Slint for the native shell, plus one lazily created, locked-down system webview (WebView2 through wry) used only for HTML mail and probably compose. This pick is conditional on a short spike, because hosting a child webview inside Slint was not verified. Rank 2 is C# with Avalonia 12 NativeAOT. Rank 3 is Rust with Tauri v2, which is also the fallback.

### Cited Findings
- An empty WebView2 shell measured 248MB private across a six-process floor, and the app itself added only about 32MB for a medium workload [A, single developer, method not described] — [Strand PR #30](https://github.com/danielss-dev/strand/pull/30)
- Microsoft advises initialising WebView2 only when displaying actual web content, and not using it for initial UI — [Microsoft WebView2 performance docs](https://learn.microsoft.com/microsoft-edge/webview2/concepts/performance)
- WebView2's browser process starts when the first WebView2 instance is created for a user data folder — [Microsoft process model docs](https://learn.microsoft.com/microsoft-edge/webview2/concepts/process-model)
- wry can create a webview inside an existing window with `build_as_child` on Windows, macOS and Linux (X11 only on Linux), and is Apache-2.0/MIT licensed — [wry repository](https://github.com/tauri-apps/wry)
- Slint was the only toolkit in a hands-on 2025 Windows test where Narrator worked "perfectly" and IME also worked, though provisional IME states showed missing-glyph boxes — [boringcactus 2025 survey](https://www.boringcactus.com/2025/04/13/2025-survey-of-rust-gui-libraries.html)
- A separate Rust project's research note reached a similar architecture: a native Rust shell (GPUI) with the system webview through wry, and Tauri 2 as the fallback [2nd] — [Syrinx wiki](https://github.com/Fledge-Software/Syrinx-app/wiki/Background-06-engine-and-ui-performance)

### Inferences

| Rank | Stack | Why | Main risk |
|---|---|---|---|
| 1 | Rust + Slint + lazy WebView2 (wry) | Lowest footprint of the options with working accessibility and IME; virtualised list; no webview cost until HTML is opened | Child webview hosting unverified; no rich-text editor; licence is not permissive |
| 2 | C# + Avalonia 12 NativeAOT + its WebView control | MIT licence; webview control now part of the framework; cross-platform; likely fastest for a solo developer to build | No measured RAM data; AOT compatibility of the stack not verified |
| 3 | Rust + Tauri v2 (webview for everything) | Fastest route to a good UI; IME, accessibility and compose come from the browser engine; a real client (Skim) already ships this way | Permanent WebView2 floor of about 250MB, which conflicts with the minimal-footprint goal |
| 4 | C++ + Qt 6 Widgets + WebView2 or Qt WebView | Probably the most mature toolkit overall (general knowledge, not sourced here) | C++ burden for a solo developer; LGPL obligations or a paid licence; WebEngine is very large |
| 5 | Rust + gpui (+ gpui-component) | Best design reference; `uniform_list` suits message lists | No built-in text input as of the 2026 snippets; accessibility described as fragile; versioning tied to Zed |
| 6 | Rust + egui | Works, with accessibility and partial IME | Immediate-mode look and rich-text limits; about 130MB reported in real apps |
| 7 | Electron | Baseline only | Largest bundle; no footprint advantage to justify it here |
| Not viable now | iced, Floem, Xilem, Zig, WinUI 3 (for cross-platform), Blitz or Servo as the sole HTML engine | See the sections below | Accessibility gaps, immaturity, or Windows-only |
| Lower bound | TUI | Near-zero footprint reference, but delegates HTML to w3m | Not a product fit for marketing mail |

- **The native advantage is narrower than it first appears.** Once a user opens one HTML message on Windows, a native app pays roughly the same WebView2 floor as Tauri, unless it closes the webview afterwards. The real wins are instant first paint, plain-text and idle sessions, tray-resident idle, and list performance. A webview teardown policy is therefore a core requirement of rank 1, not a tuning detail.
- **Keep mail logic in a UI-free Rust crate.** Moving from rank 1 to rank 3 then costs only the UI layer, which limits the risk of the Slint choice.
- **Suggested spike, before committing:**
  1. Host a wry child webview inside a Slint window on Windows 11, and test resize, focus, keyboard handover, and menus or popups overlapping it.
  2. Measure private bytes across the whole process tree at idle, after the first HTML open, and after `Close`.
  3. Measure time to first HTML paint, cold and warm.
  4. Scroll a 100k-row list.
  5. Try compose in a `contenteditable` webview.
  6. Repeat steps 2 to 4 on Avalonia NativeAOT to settle ranks 1 and 2 with data.

### Gaps
- No source confirms that Slint exposes what `build_as_child` needs on Windows, or how well the two coexist. This is the largest unknown in the recommendation.
- Rank 2 versus rank 1 rests on inference, because no Avalonia RAM figures with a method were found.

## Stack comparison: maturity and maintenance status as of 2026

### Takeaway
Slint, egui, Qt and Avalonia are the actively released options with a credible accessibility story. gpui is powerful but tied to Zed, iced still lacks accessibility, and Xilem and Floem remain early. Zig GUI options are not production-ready.

### Cited Findings

Versions below are [2nd], from the [Syrinx wiki](https://github.com/Fledge-Software/Syrinx-app/wiki/Background-06-engine-and-ui-performance) (2026-10-09), unless another source is given.

| Stack | Latest version noted | Status notes |
|---|---|---|
| Slint | 1.18.1 (2026-09-21) | Royalty-free or GPL licence; Syrinx says a zero-copy Servo texture path was verified |
| egui | 0.36.2 (2026-09-08) | Repaints on input or animation only; claims 1–2ms per frame [C] |
| iced | 0.14.0 (2025-12-07) | Reactive rendering; no AccessKit yet |
| gpui | 0.2.2 (2025-10-22) | Used through snapshot crates; AccessKit support since 2026-05-27, described as fragile |
| Xilem / Masonry | 0.4.0 (2025-10-29) | Experimental |
| Floem | 0.2.0 (2024-11) | Repository still active, no release since 2024 |
| Dioxus | 0.7.10 (2026-07-30) | Desktop is a webview; native (Blitz) is beta |
| Makepad | 1.0.0 (2025-05) | Repository still active |

- Two 2026 survey write-ups put Slint and egui at the top, with one later adding GPUI Component; cushy, Freya, Floem, iced, Relm4 and Xilem sit just below. Only search snippets were read, because the full page returned 403, and these are one author's opinions — [2026 Rust GUI survey](https://blog.wybxc.cc/blog/rust-gui-survey-2026/)
- gpui's crates.io release does not match its GitHub examples because versioning is tied to Zed's own tags, and a known bug panics when retyping after deleting IME composer text (snippet only) — [Rust GUI frameworks comparison, gpui-starter](https://gpui-starter.freeoxide.com/blog/rust-gui-frameworks-comparison-2026/)
- Zed for Windows shipped 2025-10-15 [2nd] — [Syrinx wiki](https://github.com/Fledge-Software/Syrinx-app/wiki/Background-06-engine-and-ui-performance)
- Zed moved its Windows renderer to DirectX 11, which coverage says ran better with a lower memory footprint, at the cost of maintaining three sets of GPU shaders — [Neowin](https://www.neowin.net/news/windows-slows-zeds-development-as-platform-differences-bite/); [The Register](https://www.theregister.com/2025/08/22/everything_is_different_on_windows/)
- Avalonia 12 reportedly launched on 2026-04-07, based on a Korean blog; I did not find the date on an Avalonia page — [wikidocs blog](https://wikidocs.net/blog/@lukewire/11052/)
- Windows App SDK 1.6 added NativeAOT support for WinUI 3 — [Windows Report](https://windowsreport.com/microsoft-unveils-windows-app-sdk-1-6-with-native-aot-support-improved-controls-and-more/)
- Capy, the most-cited Zig GUI library, says it is "NOT ready for use in production" and targets Zig 0.14.1, with macOS listed as planned — [Capy repository](https://github.com/capy-ui/capy)
- A newer pure-Zig library, zigui, had about 97 stars in late October 2026 — [Trendshift](https://trendshift.io/repositories/319129)

### Inferences
- Zig should be ruled out for this project: the GUI libraries are pre-production and the language is itself pre-1.0 (the latter from general knowledge).
- WinUI 3 is Windows-only, which conflicts with the macOS and Linux goal, even though it would suit a Windows-first release.
- gpui's versioning means a solo developer would effectively track Zed's internals, which is a real maintenance cost.

### Gaps
- dvui (Zig) did not appear in any search, so its status is unknown.
- No C GUI options (for example GTK on Windows or Nuklear) were researched, so that part of the question is unanswered.
- Current Qt 6 release details and Tauri v2 release cadence were not checked.
- 2026 Windows App SDK release notes were not found.

## Measured footprint: idle RAM, startup, binary and installer size

### Takeaway
No controlled Windows comparison exists. The widely repeated "Tauri uses 30–50MB" figure conflicts with measurements that count the whole process tree, which give 109MB on what appears to be macOS and about 250MB or more on Windows. Native Rust toolkits appear to land around 30–130MB in real use, on thin evidence.

### Cited Findings

| Stack | Idle RAM | Startup | Size | Label and method |
|---|---|---|---|---|
| Tauri (screen recorder) | 109MB (main process alone about 36MB) | 311ms median of 10 | 57MB with FFmpeg; under 15MB estimated without | [M] appears to be macOS; [Better Stack](https://betterstack.com/community/guides/scaling-nodejs/tauri-vs-electron-vs-deno-vs-electrobun/), updated 2026-07-20 |
| Electron (same app) | 128MB | 273ms median of 10 | 323MB | [M] same test |
| Electrobun (same app) | 208MB | 773ms | 418MB (bundled Chromium) | [M] same test |
| Deno Desktop (same app) | 98MB | 242ms | 111MB | [M] same test, early-stage |
| WebView2 empty shell | 248MB private, six processes | Not given | Not given | [A] Windows, 2026-06-29; [Strand PR #30](https://github.com/danielss-dev/strand/pull/30) |
| Tauri on Windows | Not given | 0.441s to DOMContentLoaded | About 3MB added by WebView2 | [M][2nd] GitHub runners, 2026-10-09; [Syrinx wiki](https://github.com/Fledge-Software/Syrinx-app/wiki/Background-06-engine-and-ui-performance) |
| wry alone on Windows | Not given | 0.576s to DOMContentLoaded | Not given | [M][2nd] same source |
| Tauri-based Skim client | Not given | "Instant cold start" [C] | About 5MB installer | [C] [Skim v0.1.0](https://github.com/nikserg/skim/releases/tag/v0.1.0) |
| Slint | About 45MB, or about 54MB with the Qt backend | Not given | Not given | [A] one Reddit user; [r/rust thread](https://nyc1.lr.ggtyler.dev/r/rust/comments/1jzawxo/small_footprint_gui_library) |
| Slint, GPU off, real app | About 30MB | Not given | Not given | [A] Linux; [dev.to](https://dev.to/trystan_sarrade/how-i-took-my-rust-gui-from-135-mb-to-30-mb-by-ditching-the-gpu-2i7d) |
| egui, same real app | About 135MB | Not given | Not given | [A] Linux; same source |
| Avalonia NativeAOT template | Not given | Not given | About 18MB app, about 13MB NSIS installer | [C] self-reported; [AvaloniaAOT](https://github.com/lixinyang123/AvaloniaAOT) |
| Zed | About 600MB [C][2nd] | Under 1s to first frame [C][2nd] | Not given | No method; [Syrinx wiki](https://github.com/Fledge-Software/Syrinx-app/wiki/Background-06-engine-and-ui-performance) |

- Slint's advertised runtime of roughly 300 KiB is not a desktop figure; a reply in the thread says desktop numbers include system libraries and drivers, and that most usage goes to font and image caches and frame buffers — [r/rust thread](https://nyc1.lr.ggtyler.dev/r/rust/comments/1jzawxo/small_footprint_gui_library)
- The egui author attributes much of the cost to a GPU texture atlas of glyphs and icons — [dev.to](https://dev.to/trystan_sarrade/how-i-took-my-rust-gui-from-135-mb-to-30-mb-by-ditching-the-gpu-2i7d)
- Another 2026 post described app footprints of 180–250MB, with an egui version at about 130MB [A] — [Reddit snapshot](https://reddit.sentinel-team.org/posts/1tvqq9e/snapshots/2026-06-04T04%3A47%3A22.4424Z)
- A project moving to Avalonia NativeAOT reports about 60% less memory, roughly twice the startup speed and an installer about three times smaller than its previous build [C] — [risemote v1.2.0](https://github.com/dosoft/risemote/releases/tag/v1.2.0)
- Microsoft reported 50% lower start time, about 8x smaller package (framework package) and about 2x smaller (self-contained) for one sample WinUI app using NativeAOT [C, single app] — [Windows Report](https://windowsreport.com/microsoft-unveils-windows-app-sdk-1-6-with-native-aot-support-improved-controls-and-more/)
- Tauri/WebView2 used more memory than Electron on real pages in a 2022 Windows 10 test: postman.com 399MB versus 318MB, and vscode.dev 370MB versus 312MB [M][2nd, tauri #5889] — [Syrinx wiki](https://github.com/Fledge-Software/Syrinx-app/wiki/Background-06-engine-and-ui-performance)
- Common marketing figures of 30–80MB for Tauri and 150–400MB for Electron mostly lack methodology [C] — [pkgpulse](https://www.pkgpulse.com/guides/electron-vs-tauri-2026); contradicted by [Better Stack](https://betterstack.com/community/guides/scaling-nodejs/tauri-vs-electron-vs-deno-vs-electrobun/) and [Strand PR #30](https://github.com/danielss-dev/strand/pull/30)
- Better Stack found Electron starting slightly faster than Tauri, contradicting the common claim that Tauri always starts faster — [Better Stack](https://betterstack.com/community/guides/scaling-nodejs/tauri-vs-electron-vs-deno-vs-electrobun/)

### Inferences
- The low Tauri figures probably count only the host process. Better Stack's 36MB main process versus 109MB total shows how large that error can be.
- On Windows, Tauri's RAM advantage over Electron may be small or absent, because both run Chromium-class engines. Tauri's clear win is download size.
- Slint's software renderer with the GPU off appears to be the lowest-RAM practical configuration, but this comes from one Linux app.
- Measure with private bytes summed across the full process tree, because working set and host-only figures flatter webview apps.

### Gaps
- No measured idle RAM or startup data was found for Qt 6 Widgets or QML, Avalonia, WinUI 3, gpui, iced, Xilem or Floem on Windows.
- No TUI client memory figures were found.
- No hello-world comparison of Rust toolkits on Windows with a single method exists in what I found.

## The key problem: rendering HTML email

### Takeaway
The system webview is the only option with real fidelity for marketing mail at no engine-maintenance cost, and it can be created lazily. Blitz is the most interesting native option but is beta with partial table support. Servo is unsuitable today, litehtml requires writing your own painter, and Ultralight has licensing and maintenance doubts.

### Cited Findings

**Options compared**

| Option | RAM and startup | Fidelity | Licence and status |
|---|---|---|---|
| WebView2 | 248MB empty-shell floor [A]; about 0.44–0.58s to DOMContentLoaded on CI runners [M][2nd] | Same engine as Edge | Ships with Windows 11 [2nd]; Evergreen updates |
| WKWebView / WebKitGTK | 288–416MB peak for a 2023 Linux Tauri hello world [2nd] | WebKit; Geary uses WebKitGTK | System component |
| Blitz | No measured data found | No-JS renderer; bugs and gaps remain | Apache-2.0/MIT; beta |
| Servo | About 4,066MB versus Firefox's 730MB on a large-table test; one Wikipedia page over 1GB [2nd] | 67.8% of WPT tests passing, 2026-10-09 [2nd] | MPL-2.0; monthly breaking releases |
| Ultralight | No measured data; "light" is a claim [C] | No measured email data | Proprietary; free under $100K annual revenue |
| litehtml | No measured data | Not fully standards-compliant | BSD-3-Clause; layout only |
| Qt WebEngine | 14–20MB RAM reported in 2019, on about 145MB of disk [A] | Chromium | Single library about 193MB by Qt 6.9 [A] |

**WebView2 process cost and lazy creation**
- A process group contains a single browser process, one or more renderer processes, and helpers such as GPU and audio — [Microsoft process model docs](https://learn.microsoft.com/microsoft-edge/webview2/concepts/process-model)
- Processes are tied to a browser process per user data folder, and start when the first WebView2 for that folder is created. This confirms lazy creation works — [Microsoft process model docs](https://learn.microsoft.com/microsoft-edge/webview2/concepts/process-model)
- Microsoft's performance page says "each control creates its own set of processes" — [Microsoft performance docs](https://learn.microsoft.com/microsoft-edge/webview2/concepts/performance); contradicted by [the process model page](https://learn.microsoft.com/microsoft-edge/webview2/concepts/process-model), which says controls on one user data folder share a process collection
- Cold launch must spin up processes and disk caches, which causes a noticeable delay; launch is faster if a matching Edge is already running — [Microsoft performance docs](https://learn.microsoft.com/microsoft-edge/webview2/concepts/performance)
- Reusing a single WebView2 and navigating it may be faster than destroying and recreating controls — [Microsoft performance docs](https://learn.microsoft.com/microsoft-edge/webview2/concepts/performance)
- `MemoryUsageTargetLevel = Low` and `TrySuspendAsync` reduce use on inactive webviews, on a best-effort basis — [Microsoft performance docs](https://learn.microsoft.com/microsoft-edge/webview2/concepts/performance)
- `Close` on `CoreWebView2Controller` safely shuts down the associated browser and renderer processes — [Microsoft process model docs](https://learn.microsoft.com/microsoft-edge/webview2/concepts/process-model)
- WebView2 has no supported process-count switch, and `--single-process` risks rendering breakage — [Strand PR #30](https://github.com/danielss-dev/strand/pull/30)

**Blitz**
- Blitz describes itself as beta, rendering many no-JS sites with "many bugs and missing features". Accessibility and form controls are listed as goals, and no non-Rust bindings exist yet — [Blitz repository](https://github.com/DioxusLabs/blitz)
- Latest version noted was 0.3.0-beta.2 (2026-08-24) [2nd] — [Syrinx wiki](https://github.com/Fledge-Software/Syrinx-app/wiki/Background-06-engine-and-ui-performance); `blitz-html` 0.2.0 was the last non-prerelease, in October 2025 — [lib.rs](https://lib.rs/crates/blitz-html)
- Float, flexbox, grid, `background-image`, `border-radius` and `@font-face` are supported. `display:table` is partial, emulated with CSS Grid; `border-collapse` is partial; legacy `-webkit-box-*` will not be supported. The status page does not mention media queries — [Blitz CSS status](https://blitz.is/status/css)
- The README lists media queries as a goal — [Blitz repository](https://github.com/DioxusLabs/blitz)
- Dioxus 0.7 docs call the native renderer experimental — [Dioxus docs](https://dioxuslabs.com/learn/0.7/beyond/project_structure/llms.txt)

**Servo**
- The first crate release was in April 2026; monthly releases can break, with a half-yearly LTS line for embedders — [LWN](https://lwn.net/Articles/1067467/)
- By default Servo runs as a single process. Multiprocess is fully supported only on Linux and macOS, the sandbox is "probably broken", Windows builds are about 100MB, and the download page warns against banking use [2nd] — [Syrinx wiki](https://github.com/Fledge-Software/Syrinx-app/wiki/Background-06-engine-and-ui-performance)
- No embedders had publicly announced shipping products at the time of April 2026 coverage — [Machine Herald](https://machineherald-io.pages.dev/article/2026-04/14-servo-reaches-cratesio-with-v010-giving-rust-developers-an-embeddable-browser-engine-with-long-term-support)

**Ultralight, litehtml and Qt**
- Ultralight is free for non-commercial use and for commercial use by companies under $100K annual revenue; paid licensing is unpriced publicly — [Ultralight repository](https://github.com/ultralight-ux/Ultralight)
- One aggregator reports Ultralight's last push as 2024-04-22, unverified — [gittrend](https://gittrend.io/repo/ultralight-ux/Ultralight); its free tier has a 60fps cap and no media support [2nd] — [Syrinx wiki](https://github.com/Fledge-Software/Syrinx-app/wiki/Background-06-engine-and-ui-performance)
- litehtml computes layout only: it "does not draw any text, images, or other graphics", so the app must implement a `document_container` callback — [litehtml repository](https://github.com/litehtml/litehtml)
- Qt WebEngine figures come from a 2019 Qt developer thread — [Qt mailing list](https://lists.qt-project.org/pipermail/development/2019-June/036658.html); the 193MB `libQt6WebEngineCore` figure comes from [Flathub Discourse](https://discourse.flathub.org/t/qt-webengine-takes-200mb-of-installed-size/10292)

**Locking down a webview for untrusted mail**
- Set `IsScriptEnabled`, `AreHostObjectsAllowed`, `IsWebMessageEnabled` and `AreDefaultScriptDialogsEnabled` to `false` — [Microsoft security docs](https://learn.microsoft.com/microsoft-edge/webview2/concepts/security)
- Use `NavigationStarting` and `FrameNavigationStarting` to block navigation, and treat all web content as insecure — [Microsoft security docs](https://learn.microsoft.com/microsoft-edge/webview2/concepts/security)
- Host WebView2 at standard, non-elevated integrity — [Microsoft security docs](https://learn.microsoft.com/microsoft-edge/webview2/concepts/security)
- `WebResourceRequested` with a URI and resource-context filter is the interception hook, but it resolves on the host UI thread and is slower than file URLs — [Microsoft local content docs](https://learn.microsoft.com/microsoft-edge/webview2/concepts/working-with-local-content)
- `NavigateToString` has a 2MB HTML limit, and its documents get an about:blank location and null origin — [Microsoft local content docs](https://learn.microsoft.com/microsoft-edge/webview2/concepts/working-with-local-content)
- Skim sanitises mail in Rust, renders it in a no-script sandboxed iframe, and blocks remote images by default — [Skim v0.1.0](https://github.com/nikserg/skim/releases/tag/v0.1.0)
- Geary gained an explicit `load_remote_resources` call for remembered senders in 2021 — [GNOME commit list](https://mail.gnome.org/archives/commits-list/2021-April/msg01176.html)

### Inferences
- **Recommended tiered rendering:**
  1. Show the `text/plain` part natively if the user prefers it, or when it is a genuine alternative.
  2. Render simple HTML through sanitise-to-rich-text natively, if that proves worthwhile.
  3. Use the lazy webview for everything else.
- **Webview lifecycle policy:** create on first HTML open, reuse one instance, drop to `Low` when hidden, and call `Close` after an idle timeout or when minimised to tray. Expect about 0.4–0.6s on first open, extrapolated from CI runner figures; those include whole-app startup, so real cost may differ.
- **Defence in depth, in this order:**
  1. Sanitise in Rust before the webview sees anything.
  2. Disable scripts and all host bridges.
  3. Add a strict CSP in the document (for example `default-src 'none'; img-src cid: data:; style-src 'unsafe-inline'`).
  4. Intercept all requests with `WebResourceRequested`, and deny remote ones by default.
  5. Cancel all navigations and pass links to the system browser after a confirmation.
  6. Use a dedicated user data folder with no credentials or cookies.

  The CSP string is my proposal, and has not been tested against a reference.
- **Do not treat `NavigateToString` as a remote-content block.** The docs sentence about not referencing resources may refer to relative or local files. My recollection is that absolute https URLs can still load, so test this with a remote pixel. The 2MB limit also rules it out for large newsletters with inlined images.
- Blitz is worth re-evaluating in 6–12 months, because newsletter layouts rely heavily on nested tables, which is exactly where it is partial. It would also need an accessibility story, and it is listed only as a goal today.
- Servo is ruled out for now: running hostile content single-process with a doubtful sandbox is the wrong risk profile for email.

### Gaps
- No source measured fidelity on a real marketing or newsletter corpus for any engine, including Blitz, litehtml and Ultralight. A small test corpus is needed.
- It is unverified whether `WebResourceRequested` catches every subresource type in a `NavigateToString` document, and how it interacts with CSP.
- My recollection, not verified this session, is that `IsScriptEnabled = false` does not stop host-injected `ExecuteScript`. This matters for measuring message height to size the view.
- No isolated cold-start time exists for WebView2 alone, and the Strand per-process breakdown is in a document the PR cites but does not include.
- No sourced statement on the "airspace" problem (native popups drawn over a child webview) was found; it is expected from general knowledge and needs testing.
- Sanitise-to-rich-text fidelity was not researched in any depth.
- litehtml's activity date and CSS list were not retrieved; no email client using it was found.

## Virtualised lists, compose, IME, accessibility, HiDPI and dark mode

### Takeaway
Virtualised lists are a solved problem in the leading toolkits, though no 100k-row benchmark exists. Rich-text compose is the real gap: no mature WYSIWYG widget exists in Rust toolkits, so compose probably belongs in a webview. Accessibility and IME still separate the toolkits sharply.

### Cited Findings

**Hands-on IME and Narrator test on Windows** (one author, April 2025, so 19 months old) — [boringcactus 2025 survey](https://www.boringcactus.com/2025/04/13/2025-survey-of-rust-gui-libraries.html)

| Library | Japanese IME | Narrator | Text input |
|---|---|---|---|
| Slint | Works; missing-glyph boxes in provisional states | Works "perfectly" | Works |
| egui | Partial; Tab captured, default font lacks kana/kanji | Works, somewhat janky | Works |
| iced | Does not work | Cannot see window contents | Works |
| Xilem | Works; some missing glyphs | Sees text, wrong position | Works |
| Floem | Does not work | Cannot see text | Works |
| gpui | Works | Cannot see window contents (since changed, see below) | No basic widget; adapted a 700+ line example |
| Dioxus (webview) | Works well | Sees text, reported as "Web content region" | Works |
| Tauri | Works | Sees all text | Works in frontend |

**Lists**
- Slint's `ListView` instantiates only visible elements, which the docs say guarantees stable performance with "a practically unlimited number of items" [C]. The page does not explain variable row heights — [Slint ListView docs](https://docs.slint.dev/latest/docs/slint/reference/std-widgets/views/listview/)
- gpui's `uniform_list` builds only visible rows, so a 100k-item list reportedly costs the same as a 20-item one, provided rows have equal height [C, third-party wrapper docs] — [yororen-ui wiki](https://github.com/MeowLynxSea/yororen-ui/wiki/Widget-VirtualList)
- `gpui-component` provides a `VirtualList` for variable-size items — [gpui-component source](https://docs.rs/crate/gpui-component/latest/source/src/virtual_list.rs)

**Compose and rich text**
- iced's built-in text editor had no styling support such as bold or italics as of January 2026, and a commenter there argued rich text editing "is not something that exists in the Rust ecosystem yet" — [Reddit snapshot](https://reddit.sentinel-team.org/posts/1qn8qu2/snapshots/2026-01-27T02%3A50%3A36.127967Z)
- The usual suggestion in that thread is to build on Parley or cosmic-text — same source
- `rust_widgets` has a "rich edit" widget, but its docs say it stores no styling, so it is plain text — [rust_widgets source](https://docs.rs/crate/rust_widgets/2.8.4/source/src/widget/input_widgets/rich_edit.rs)

**Accessibility updates since that test**
- gpui gained AccessKit support on 2026-05-27, described as fragile; iced still had no AccessKit at 0.14 [2nd] — [Syrinx wiki](https://github.com/Fledge-Software/Syrinx-app/wiki/Background-06-engine-and-ui-performance)
- A 2026 survey snippet still calls Floem's IME and accessibility broken — [2026 Rust GUI survey](https://blog.wybxc.cc/blog/rust-gui-survey-2026/)

### Inferences
- A uniform row height is the natural fit for a message list, so `uniform_list` and `ListView` should both work. With 100k+ rows the real constraint is likely to be the data model: feed rows on demand rather than holding a full model in memory.
- **Compose should use a separate, trusted webview page with `contenteditable`**, on a different document from untrusted mail. It gets IME, spellcheck, accessibility and HTML output at once. The cost is the WebView2 floor while composing, which is acceptable because compose is a discrete session. Quoted untrusted content in replies must be sanitised first, since scripts are enabled in that context.
- Plain-text compose can stay native, which suits the minimal-footprint goal.
- iced and Floem are effectively ruled out while accessibility fails, particularly for anything that may go commercial.

### Gaps
- No 100k-row benchmark was found for any toolkit: Slint and iced results were absent, and gpui's is a documentation claim.
- Avalonia, WinUI 3 and Qt list virtualisation, compose, IME and accessibility were not sourced in this research. Qt's `QTextEdit` and model/view are generally regarded as strong, from general knowledge only.
- HiDPI and dark mode were not covered by any source found. This part of the question is unanswered, and should be checked in the spike, including mixed-DPI monitors and how dark mode applies inside HTML mail.
- Accessibility results need re-testing on current versions.

## Licensing for later commercial use

### Takeaway
Avalonia, wry/Tauri, Blitz and litehtml are permissive and safest. Slint's royalty-free licence allows proprietary desktop apps at no cost, but its attribution condition needs confirming. Qt is workable under LGPL with dynamic linking and real obligations. Ultralight would become a paid dependency once revenue reaches $100K.

### Cited Findings
- Slint is offered as GPLv3, royalty-free 2.0, or the Slint Software licence — [Slint licence listing](https://components.espressif.com/components/slint/slint/versions/1.17.0/license?language=en)
- Royalty-free covers proprietary desktop, mobile and web applications at no cost, and excludes embedded systems — [Slint pricing](https://slint.dev/pricing)
- A copy of Slint's licence file says the royalty-free licence is free provided you disclose Slint use, for example with `AboutSlint` or a badge — [Slint LICENSE.md 1.16](https://cdn.jsdelivr.net/gh/slint-ui/slint@1.16.0/LICENSE.md); possibly contradicted by [Slint pricing](https://slint.dev/pricing), where a table row says attribution is optional, though the fetched text did not show which plan that row applies to
- Paid Slint tiers exist, including Startup & Individual (up to 10 employees, up to 2M EUR turnover, under 5 years old) and Small Enterprise — [Slint pricing](https://slint.dev/pricing)
- Qt's LGPLv3 obligations are: supply Qt source or a written offer even if unmodified, allow re-linking, provide the licence text and a prominent notice, and add no conflicting restrictions. Some app stores may conflict — [Qt LGPL obligations](https://www.qt.io/licensing/open-source-lgpl-obligations)
- Qt recommends dynamic linking; a statically linked application may itself become subject to the LGPL. Some Qt parts are GPL-only — [Qt LGPL obligations](https://www.qt.io/licensing/open-source-lgpl-obligations)
- Qt sells Application Development and Small Business subscriptions, with a limit of three small-business licences per company and 12-month terms — [Qt pricing](https://www.qt.io/pricing)
- A forum post cites about €4,200 per year for Qt, but it is old and could not be confirmed — [Qt forum](https://forum.qt.io/post/459759)
- Avalonia's core is MIT, and its v12 FAQ says commercial apps can be shipped free — [Avalonia v12 FAQ](https://v12.docs.avaloniaui.net/tools/faq)
- Avalonia's WebView, previously commercial-only, is being open-sourced as part of version 12; it uses native platform web rendering instead of bundling Chromium — [Avalonia WebView announcement](https://avaloniaui.net/accelerate/webview)
- The Accelerate brand is being retired — [Avalonia blog listing](https://avaloniaui.net/blog/authors/1)
- wry is Apache-2.0/MIT — [wry repository](https://github.com/tauri-apps/wry); Blitz is Apache-2.0/MIT, with `stylo_taffy` also under MPL-2.0 — [Blitz repository](https://github.com/DioxusLabs/blitz)
- litehtml is BSD-3-Clause — [litehtml repository](https://github.com/litehtml/litehtml); Servo is MPL-2.0 [2nd] — [Syrinx wiki](https://github.com/Fledge-Software/Syrinx-app/wiki/Background-06-engine-and-ui-performance)

### Inferences
- Slint's royalty-free licence fits a solo commercial product, but it is a vendor licence and not a permissive open-source one, so read the current PDF before committing. An "About" attribution would be cheap to include anyway.
- For Qt under LGPL, a closed-source Windows app with dynamic linking is workable. The Microsoft Store and macOS App Store cases deserve a legal check.
- None of this is legal advice; have a lawyer review before commercial launch.

### Gaps
- Current Qt prices and small-business eligibility limits were not visible in the fetched page.
- Slint's paid plan prices were not visible, and the attribution conflict above is unresolved. Check the [royalty-free licence PDF](https://slint.dev/agreements/slint-royalty-free-license.pdf).
- The final licence of Avalonia's open-source WebView was not confirmed, and neither were its new paid tiers.
- gpui's, Tauri's and egui's licences were not checked in this research.
- WebView2 redistribution terms were not reviewed.

## Comparable real projects

### Takeaway
Serious clients almost universally use a browser engine for the message body, even when the shell is native. That pattern supports the recommendation. Published memory data for email clients is old and anecdotal.

### Cited Findings

| Project | Stack | HTML rendering | Footprint evidence |
|---|---|---|---|
| Thunderbird | Mozilla platform (general knowledge) | Gecko (general knowledge) | [A] wide range, see below |
| Mailspring | Electron, React and TypeScript UI; C++ sync engine with MailCore2, one process per account | Electron | No RAM data found; [repository](https://github.com/Foundry376/Mailspring-Sync) |
| Geary | Vala + WebKitGTK | WebKitGTK | [A] 55MB RSS, one account, fresh start, December 2014; [vpsboard](https://vpsboard.com/goto/post?id=81001) |
| Sylpheed | Native (GTK, general knowledge) | Not researched | [A] about 112MB RSS, December 2014; [vpsboard](https://vpsboard.com/goto/post?id=81001) |
| Icedove (Thunderbird) | As Thunderbird | As Thunderbird | [A] about 1.9GB RSS after long use, November 2014; [vpsboard](https://vpsboard.com/goto/post?id=81001) |
| Mimestream | Swift/AppKit, not Electron | Not confirmed | [C] about 100–200MB and under 1s, from a competitor; [ZenMail blog](https://zenmail-site.pages.dev/blog/mimestream-vs-zenmail) |
| Superhuman | Described as Electron-based | Chromium, if so | No data; [ZenMail blog](https://zenmail-site.pages.dev/blog/best-gmail-clients-mac-2026) |
| Skim | Rust + Tauri 2 + WebView2 | Sanitised, no-script sandboxed iframe | [C] about 5MB installer; [Skim v0.1.0](https://github.com/nikserg/skim/releases/tag/v0.1.0) |
| RAVN | Tauri 2.9 + Nuxt | Webview | No data; [ecosyste.ms](https://awesome.ecosyste.ms/projects/github.com%2Fravnmail%2Fravn) |
| MailLedger | Rust + iced | Apparently no webview | No data; [mirror](https://gh.r-mj.com/mqasimca/mailledger) |
| Thundermail | Rust + egui, Linux-focused hackathon project | Avoids webview stacks | No data; [FOSS Hack](https://fossunited.org/hack/fosshack26/p/e8ai9vond5) |
| meli | Rust TUI (language from general knowledge) | w3m by default, overridable by `pager.html_filter` | No data; [meli repository](https://Github.com/meli/meli) |
| aerc | TUI | Interactive terminal browser; w3m packaged as a Debian dependency | No data; [Debian package](https://packages.debian.org/ja/forky/aerc) |
| Ratmail | Rust TUI | Renders HTML in the terminal | No data; [repository](https://github.com/peter-fm/ratmail) |

- Thunderbird reports vary: about 290MB at startup growing past 4,000MB on one Windows machine, 695–745MB on a Mac, and 140MB rising to about 300MB in a bug report. All are user reports with unknown mailbox sizes, and I could not attribute each number to a specific page beyond the [Mozilla support thread](https://support.mozilla.org/fi/questions/1439008) and [Bugzilla](https://bugzilla.mozilla.org/show_bug.cgi?id=1672933) groupings returned by search.
- Mailspring's licence is inconsistently described: GPLv3 for the sync engine in [Mailspring-Sync](https://github.com/Foundry376/Mailspring-Sync), closed-source in [older forks](https://github.com/gnuhub/Mailspring)
- A third-party review says Geary blocks remote images and disables JavaScript in messages; this is secondary — [itechguides](https://www.itechguides.com/directory/email-clients/geary/)
- Native clients' footprint claims and Superhuman's Electron architecture come mainly from a competitor's marketing — [ZenMail blog](https://zenmail-site.pages.dev/blog/best-gmail-clients-mac-2026)
- gpui as a design reference: renders with DirectX 11 and DirectWrite on Windows — [Neowin](https://www.neowin.net/news/windows-slows-zeds-development-as-platform-differences-bite/); [Zed releases](https://zed.dev/releases/stable?b=59). Zed crashed on Windows from GPU memory allocation failures during the port — [The Register](https://www.theregister.com/2025/08/22/everything_is_different_on_windows/)

### Inferences
- The native shell plus engine-rendered body pattern is established in Geary, and probably in Mimestream, though this was not confirmed.
- Mailspring's split (native sync process, UI layer separate) shows that the UI choice can be isolated from mail logic, which supports keeping the Rust core UI-free.
- Rust clients that avoid a webview (MailLedger, Thundermail) have no public evidence of acceptable fidelity on marketing mail.
- GPU memory is a real Windows cost for GPU-rendered toolkits, supporting a test of Slint's software renderer.

### Gaps
- No controlled benchmark of email clients exists in what I found. The 2014 Geary, Sylpheed and Icedove figures are too old to use as targets, and the WebKitGTK of 2014 is not today's.
- Evolution and Claws Mail were not researched. Himalaya did not appear in results.
- Mimestream's HTML rendering approach was not confirmed.
- Maintenance status of Geary in 2026 could not be established, beyond a packaged 46.0-8 in [Debian](https://packages-pkgmirror-csail.debian.org/ja/sid/geary).
- Superhuman's architecture lacks a first-party source.
