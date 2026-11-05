//! The body of a message being written: `editor/editor.html` in a WebView2 of its own. The reading
//! pane stays as it is, with scripts off. This one needs them, so it is locked down differently:
//! it can run only the app's own script, and it reaches nothing but the page the app serves it.
//! What it edits includes quoted mail, so that is sanitised before it arrives, and again by the
//! page, and a strict CSP stands behind both.
//!
//! The page reports every change as it happens. Nothing is ever waiting in it, so leaving the
//! composer in any way at all needs no round trip.

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::{Rc, Weak};
use std::time::Duration;

use serde::Deserialize;
use serde_json::json;
use slint::{Color, ComponentHandle, Timer, TimerMode};
use webview2_com::Microsoft::Web::WebView2::Win32::*;
use webview2_com::{NavigationStartingEventHandler, WebResourceRequestedEventHandler, take_pwstr};
use windows::Win32::UI::Shell::SHCreateMemStream;
use windows::core::{HSTRING, Interface, PWSTR, w};
use wry::{WebContext, WebView, WebViewBuilder, WebViewBuilderExtWindows, WebViewExtWindows};

use crate::webview::{BROWSER_ARGS, Bounds, out_of_sight, rect};
use crate::{App, ComposePane, Theme, on_ui_thread};

/// Never resolved: every request for it is answered from memory.
const ORIGIN: &str = "https://editor.invalid/";
const PAGE_URL: &str = "https://editor.invalid/editor.html";
const POLICY: &str = "default-src 'none'; script-src https://editor.invalid/editor.js; style-src 'unsafe-inline'; \
    img-src data:; font-src https://editor.invalid/; base-uri 'none'; form-action 'none'";
/// How long the browser stays alive with nothing being written.
const IDLE_CLOSE: Duration = Duration::from_secs(120);

const PAGE: &str = include_str!("../editor/editor.html");
const SCRIPT: &str = include_str!("../editor/editor.js");
const REGULAR: &[u8] = include_bytes!("../ui/fonts/BeVietnamPro-Regular.ttf");
const SEMIBOLD: &[u8] = include_bytes!("../ui/fonts/BeVietnamPro-SemiBold.ttf");

/// What the page says. It only ever reports: the app decides what anything means.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum EditorEvent {
    /// The page has started, and can be given a message.
    Ready,
    /// A message is in place, in the browser's own way of writing it. This is its starting point.
    Loaded { html: String, text: String },
    Changed { html: String, text: String },
    Send,
    Close,
    /// Shift+Tab, back to the fields above.
    Leave,
    /// Send from the account at this place in the sidebar.
    Sender { index: usize },
    Discard,
    AttachFiles,
    /// A picture in the text, as base64, which the HTML points to as `cid:`.
    Picture { cid: String, name: String, mime: String, data: String },
    /// A file dropped or pasted that is not a picture, as base64.
    File { name: String, mime: String, data: String },
    Problem { text: String },
}

struct Open {
    view: WebView,
    _context: WebContext,
}

/// What the page's messages and the app's calls both reach.
#[derive(Default)]
struct Page {
    open: RefCell<Option<Open>>,
    /// The script has started, so calls into it will land.
    ready: Cell<bool>,
    /// Calls waiting for it to start.
    waiting: RefCell<Vec<String>>,
    slot: Cell<Bounds>,
    /// A message is on its way, and comes into view when it lands.
    wanted: Cell<bool>,
    take_focus: Cell<bool>,
    on_screen: Cell<bool>,
}

impl Page {
    fn run(&self, script: String) {
        if !self.ready.get() {
            self.waiting.borrow_mut().push(script);
        } else if let Some(open) = self.open.borrow().as_ref() {
            let _ = open.view.evaluate_script(&script);
        }
    }

    fn heard(&self, event: &EditorEvent) {
        match event {
            EditorEvent::Ready => {
                self.ready.set(true);
                for script in self.waiting.take() {
                    self.run(script);
                }
            }
            EditorEvent::Loaded { .. } if self.wanted.take() => self.bring_in(),
            _ => {}
        }
    }

    fn bring_in(&self) {
        let open = self.open.borrow();
        let Some(open) = open.as_ref() else { return };
        let _ = open.view.set_bounds(rect(self.slot.get()));
        self.on_screen.set(true);
        if self.take_focus.take() {
            let _ = open.view.focus();
        }
    }
}

pub struct EditorView {
    data_dir: PathBuf,
    page: Rc<Page>,
    idle: Timer,
    /// Each URL the page has asked for that was refused, for checking that the lockdown holds.
    pub refused: Rc<RefCell<Vec<String>>>,
}

fn css(colour: Color) -> String {
    format!("#{:02x}{:02x}{:02x}", colour.red(), colour.green(), colour.blue())
}

fn slot(app: &App) -> Bounds {
    let s = app.window().scale_factor();
    (app.get_editor_x() * s, app.get_editor_y() * s, app.get_editor_w() * s, app.get_editor_h() * s)
}

fn theme(app: &App) -> serde_json::Value {
    let theme = app.global::<Theme>();
    json!({
        "dark": theme.get_dark(),
        "sheet": css(theme.get_sheet().color()),
        "text": css(theme.get_text().color()),
        "muted": css(theme.get_muted().color()),
        "line": css(theme.get_line().color()),
        "hover": css(theme.get_hover().color()),
        "selected": css(theme.get_selected().color()),
        "accent": css(theme.get_accent().color()),
        "faint": css(theme.get_faint().color()),
        "size": format!("{}px", theme.get_reading()),
        "gutter": format!("{}px", theme.get_gutter()),
    })
}

impl EditorView {
    pub fn new(data_dir: PathBuf) -> Self {
        Self { data_dir, page: Rc::default(), idle: Timer::default(), refused: Rc::default() }
    }

    fn create(&self, app: &App) -> wry::Result<Open> {
        let mut context = WebContext::new(Some(self.data_dir.clone()));
        let page = Rc::downgrade(&self.page);
        let sheet = app.global::<Theme>().get_sheet().color();
        let handle = app.window().window_handle();
        let view = WebViewBuilder::new_with_web_context(&mut context)
            .with_bounds(out_of_sight(self.page.slot.get()))
            .with_background_color((sheet.red(), sheet.green(), sheet.blue(), 255))
            .with_focused(false)
            .with_devtools(false)
            .with_additional_browser_args(BROWSER_ARGS)
            .with_new_window_req_handler(|_, _| wry::NewWindowResponse::Deny)
            .with_ipc_handler(move |request| listen(&page, request.body()))
            .build_as_child(&handle)?;
        lock_down(&view, self.refused.clone()).map_err(|e| wry::Error::WebView2Error(webview2_com::Error::WindowsError(e)))?;
        view.load_url(PAGE_URL)?;
        Ok(Open { view, _context: context })
    }

    /// Whether the page is in the window.
    pub fn is_on_screen(&self) -> bool {
        self.page.on_screen.get()
    }

    pub fn with_view<T>(&self, f: impl FnOnce(&WebView) -> T) -> Option<T> {
        self.page.open.borrow().as_ref().map(|o| f(&o.view))
    }
}

/// One message from the page. Anything that does not parse came from nowhere the app knows.
fn listen(page: &Weak<Page>, message: &str) {
    let (Some(page), Ok(event)) = (page.upgrade(), serde_json::from_str::<EditorEvent>(message)) else { return };
    page.heard(&event);
    on_ui_thread(move |this| this.editor_event(event));
}

impl ComposePane for Rc<EditorView> {
    fn open(&self, app: &App, html: &str, take_focus: bool) -> bool {
        self.idle.stop();
        self.page.slot.set(slot(app));
        if self.page.open.borrow().is_none() {
            self.page.ready.set(false);
            self.page.waiting.borrow_mut().clear();
            match self.create(app) {
                Ok(created) => *self.page.open.borrow_mut() = Some(created),
                Err(_) => return false,
            }
        }
        self.page.wanted.set(true);
        self.page.take_focus.set(take_focus);
        self.page.run(format!("editor.theme({});editor.load({});", theme(app), json!(html)));
        true
    }

    fn focus(&self) {
        self.page.run("editor.focus();".to_string());
        if let Some(open) = self.page.open.borrow().as_ref() {
            let _ = open.view.focus();
        }
    }

    fn release_focus(&self) {
        if let Some(open) = self.page.open.borrow().as_ref() {
            let _ = open.view.focus_parent();
        }
    }

    fn sign(&self, html: &str) {
        self.page.run(format!("editor.sign({});", json!(html)));
    }

    /// Stays alive for a while, as the next message tends not to be long coming.
    fn hide(&self) {
        self.page.wanted.set(false);
        self.page.on_screen.set(false);
        if let Some(open) = self.page.open.borrow().as_ref() {
            let _ = open.view.focus_parent();
            let _ = open.view.set_bounds(out_of_sight(self.page.slot.get()));
        }
        let page = self.page.clone();
        self.idle.start(TimerMode::SingleShot, IDLE_CLOSE, move || {
            page.open.borrow_mut().take();
        });
    }

    fn bounds_changed(&self, app: &App) {
        self.page.slot.set(slot(app));
        if self.page.on_screen.get()
            && let Some(open) = self.page.open.borrow().as_ref()
        {
            let _ = open.view.set_bounds(rect(slot(app)));
        }
    }
}

/// The page and what it asks for, by URL.
fn resource(url: &str) -> Option<(&'static [u8], &'static str)> {
    match url.strip_prefix(ORIGIN)? {
        "editor.html" => Some((PAGE.as_bytes(), "text/html; charset=utf-8")),
        "editor.js" => Some((SCRIPT.as_bytes(), "text/javascript; charset=utf-8")),
        "regular.ttf" => Some((REGULAR, "font/ttf")),
        "semibold.ttf" => Some((SEMIBOLD, "font/ttf")),
        _ => None,
    }
}

/// Everything the page may not do. Scripts run, and the right-click menu stays for spelling, but
/// the page can go nowhere, load nothing of anyone else's, and reach nothing of the app's but
/// [`listen`].
fn lock_down(view: &WebView, refused: Rc<RefCell<Vec<String>>>) -> windows::core::Result<()> {
    unsafe {
        let core = view.webview();
        let s = core.Settings()?;
        s.SetAreHostObjectsAllowed(false)?;
        s.SetAreDefaultScriptDialogsEnabled(false)?;
        s.SetAreDevToolsEnabled(false)?;
        s.SetIsStatusBarEnabled(false)?;
        s.cast::<ICoreWebView2Settings3>()?.SetAreBrowserAcceleratorKeysEnabled(false)?;
        let s4 = s.cast::<ICoreWebView2Settings4>()?;
        s4.SetIsPasswordAutosaveEnabled(false)?;
        s4.SetIsGeneralAutofillEnabled(false)?;

        let mut token = Default::default();
        core.add_NavigationStarting(
            &NavigationStartingEventHandler::create(Box::new(|_, args| {
                let Some(args) = args else { return Ok(()) };
                let mut uri = PWSTR::null();
                args.Uri(&mut uri)?;
                if take_pwstr(uri) != PAGE_URL {
                    args.SetCancel(true)?;
                }
                Ok(())
            })),
            &mut token,
        )?;

        let env = view.environment();
        core.AddWebResourceRequestedFilter(w!("*"), COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL)?;
        core.add_WebResourceRequested(
            &WebResourceRequestedEventHandler::create(Box::new(move |_, args| {
                let Some(args) = args else { return Ok(()) };
                let mut uri = PWSTR::null();
                args.Request()?.Uri(&mut uri)?;
                let uri = take_pwstr(uri);
                let response = match resource(&uri) {
                    Some((bytes, kind)) => {
                        let stream = SHCreateMemStream(Some(bytes));
                        let headers = format!("Content-Type: {kind}\r\nCache-Control: no-store\r\nContent-Security-Policy: {POLICY}");
                        env.CreateWebResourceResponse(stream.as_ref(), 200, w!("OK"), &HSTRING::from(headers))?
                    }
                    None => {
                        refused.borrow_mut().push(uri);
                        env.CreateWebResourceResponse(None, 403, w!("Blocked"), w!(""))?
                    }
                };
                args.SetResponse(&response)?;
                Ok(())
            })),
            &mut token,
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_apps_own_files_are_served() {
        assert!(resource(PAGE_URL).is_some());
        assert!(resource("https://editor.invalid/editor.js").is_some());
        assert!(resource("https://editor.invalid/../secret").is_none());
        assert!(resource("https://tracker.example/editor.js").is_none());
    }

    #[test]
    fn the_page_only_loads_what_its_policy_allows() {
        assert!(PAGE.contains(r#"src="/editor.js""#));
        assert!(!POLICY.contains("script-src 'unsafe"), "scripts are the app's own file, never inline");
        assert!(!PAGE.contains("<script>"), "an inline script would be refused");
    }

    #[test]
    fn what_the_page_says_is_understood() {
        let event: EditorEvent = serde_json::from_str(r#"{"kind":"changed","html":"<b>x</b>","text":"x"}"#).unwrap();
        assert_eq!(event, EditorEvent::Changed { html: "<b>x</b>".into(), text: "x".into() });
        assert_eq!(serde_json::from_str::<EditorEvent>(r#"{"kind":"attach-files"}"#).unwrap(), EditorEvent::AttachFiles);
        assert_eq!(serde_json::from_str::<EditorEvent>(r#"{"kind":"sender","index":1}"#).unwrap(), EditorEvent::Sender { index: 1 });
        assert!(serde_json::from_str::<EditorEvent>(r#"{"kind":"format-disk"}"#).is_err());
    }
}
