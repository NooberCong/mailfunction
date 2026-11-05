//! The reading pane for HTML mail: a child WebView2 created on first use and closed when idle,
//! because this one window accounts for almost all of the app's memory.
//!
//! Sanitised HTML is served from an in-memory URL through the same interception hook that
//! refuses everything else, so there is no size limit and the CSP is a real header. Mail is
//! hostile: scripts are off, nothing can reach the network, and links open in the browser.

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use mailfunction_core::render::CONTENT_SECURITY_POLICY;
use slint::{ComponentHandle, Timer, TimerMode};
use webview2_com::Microsoft::Web::WebView2::Win32::*;
use webview2_com::{
    AcceleratorKeyPressedEventHandler, CapturePreviewCompletedHandler, NavigationStartingEventHandler,
    WebResourceRequestedEventHandler, take_pwstr,
};
use windows::Win32::UI::Shell::{SHCreateMemStream, ShellExecuteW};
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
use windows::core::{BOOL, HSTRING, Interface, PWSTR, w};
use wry::dpi::{PhysicalPosition, PhysicalSize};
use wry::{Rect, WebContext, WebView, WebViewBuilder, WebViewBuilderExtWindows, WebViewExtWindows};

use crate::{App, HtmlPane, Pane, Theme};

/// Never resolved: every request for it is answered from memory.
const DOCUMENT_ORIGIN: &str = "https://message.invalid/";
const VK_ESCAPE: u32 = 0x1B;
/// The page is served from memory, so the browser never needs a network. Refusing requests in
/// the hook is not enough: Chromium preconnects to hosts it sees in the HTML before any request
/// exists, and a bare TCP connection already tells a tracker the mail was opened. A dead proxy
/// and failing DNS leave it no route out. The first flag repeats wry's defaults, which setting
/// these arguments replaces, and adds PaintHolding, which would keep the last message's frame
/// up until the next one has painted.
const BROWSER_ARGS: &str = "--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection,PaintHolding \
    --proxy-server=http://127.0.0.1:9 --proxy-bypass-list=<-loopback> \
    --host-resolver-rules=\"MAP * ~NOTFOUND\"";
/// How long the browser processes stay alive with no HTML message on screen.
const IDLE_CLOSE: Duration = Duration::from_secs(120);
/// How long a page out of sight waits for a successor before it is emptied, so that nothing
/// keeps animating unseen. Long enough that moving between messages never gets here.
const EMPTY_AFTER: Duration = Duration::from_millis(500);

/// A rectangle of the window, in physical pixels: left, top, width and height.
type Bounds = (f32, f32, f32, f32);

fn rect((x, y, w, h): Bounds) -> Rect {
    Rect {
        position: PhysicalPosition::new(x as i32, y as i32).into(),
        size: PhysicalSize::new(w as u32, h as u32).into(),
    }
}

/// Clipped away by the window, so nothing shows, while the browser still counts as visible.
fn out_of_sight((_, y, w, h): Bounds) -> Rect {
    rect((-w - 64.0, y, w, h))
}

/// Hands a clicked link to the default browser, for schemes that cannot touch local files.
pub fn open_externally(url: &str) {
    let lower = url.to_ascii_lowercase();
    if ["http:", "https:", "mailto:"].iter().any(|s| lower.starts_with(s)) {
        unsafe {
            ShellExecuteW(None, w!("open"), &HSTRING::from(url), None, None, SW_SHOWNORMAL);
        }
    }
}

#[derive(Default)]
struct Document {
    url: String,
    html: String,
    /// False for a page nobody is waiting for: the user has left it, or it is the empty one.
    wanted: bool,
}

/// What the page has tried to do, for checking that the lockdown holds.
#[derive(Default)]
pub struct Activity {
    pub refused: RefCell<Vec<String>>,
    pub cancelled_navigations: RefCell<Vec<String>>,
    pub loaded: Cell<u32>,
}

struct Open {
    view: WebView,
    _context: WebContext,
}

/// Where the view belongs, and whether its page is ready to be there.
#[derive(Default, Clone, Copy)]
struct Placement {
    slot: Bounds,
    revealed: bool,
}

pub struct Webview {
    data_dir: PathBuf,
    policy: String,
    document: Rc<RefCell<Document>>,
    serial: Rc<Cell<u64>>,
    open: Rc<RefCell<Option<Open>>>,
    app: Rc<RefCell<slint::Weak<App>>>,
    placement: Rc<Cell<Placement>>,
    empty: Timer,
    idle: Timer,
    backdrop: Cell<(u8, u8, u8)>,
    pub activity: Rc<Activity>,
}

impl Webview {
    pub fn new(data_dir: PathBuf) -> Self {
        Self::with_policy(data_dir, CONTENT_SECURITY_POLICY)
    }

    /// The CSP is one layer of several. Tests loosen it to prove the others hold on their own.
    pub fn with_policy(data_dir: PathBuf, policy: &str) -> Self {
        Self {
            data_dir,
            policy: policy.to_string(),
            document: Rc::default(),
            serial: Rc::default(),
            open: Rc::default(),
            app: Rc::default(),
            placement: Rc::default(),
            empty: Timer::default(),
            idle: Timer::default(),
            backdrop: Cell::new((255, 255, 255)),
            activity: Rc::default(),
        }
    }

    fn create(&self, window: &slint::Window, bounds: Rect) -> wry::Result<Open> {
        let mut context = WebContext::new(Some(self.data_dir.clone()));
        let activity = self.activity.clone();
        let reveal = Reveal {
            document: self.document.clone(),
            open: self.open.clone(),
            app: self.app.clone(),
            placement: self.placement.clone(),
        };
        let handle = window.window_handle();
        let view = WebViewBuilder::new_with_web_context(&mut context)
            .with_bounds(bounds)
            .with_background_color({
                let (red, green, blue) = self.backdrop.get();
                (red, green, blue, 255)
            })
            .with_focused(false)
            .with_devtools(false)
            .with_javascript_disabled()
            .with_additional_browser_args(BROWSER_ARGS)
            .with_on_page_load_handler(move |event, url| {
                if !matches!(event, wry::PageLoadEvent::Finished) {
                    return;
                }
                activity.loaded.set(activity.loaded.get() + 1);
                reveal.after_first_frame(url);
            })
            .with_new_window_req_handler(|_, _| wry::NewWindowResponse::Deny)
            .build_as_child(&handle)?;
        lock_down(&view, &self.policy, self.document.clone(), self.activity.clone())
            .map_err(|e| wry::Error::WebView2Error(webview2_com::Error::WindowsError(e)))?;
        Ok(Open { view, _context: context })
    }

    /// Loads a sanitised document, to appear in the given rectangle when it is ready.
    ///
    /// A webview shows its old page until the next one paints. Hiding it does not help: a hidden
    /// webview paints nothing, so it would flash that page on being shown again. So the view is
    /// never hidden. It waits out of the window, and comes in once the new page has a frame.
    pub fn load(&self, window: &slint::Window, slot: Bounds, html: String) -> wry::Result<()> {
        self.empty.stop();
        self.idle.stop();
        if self.open.borrow().is_none() {
            let created = self.create(window, out_of_sight(slot))?;
            *self.open.borrow_mut() = Some(created);
        }
        self.placement.set(Placement { slot, revealed: false });
        let open = self.open.borrow();
        let view = &open.as_ref().expect("created above").view;
        view.set_bounds(out_of_sight(slot))?;
        navigate(view, &self.document, &self.serial, html, true)
    }

    /// Whether a page is in the window.
    pub fn is_on_screen(&self) -> bool {
        self.placement.get().revealed
    }

    /// Ends the browser processes, which frees their memory.
    pub fn close(&self) {
        self.empty.stop();
        self.idle.stop();
        self.open.borrow_mut().take();
    }

    pub fn is_open(&self) -> bool {
        self.open.borrow().is_some()
    }

    pub fn with_view<T>(&self, f: impl FnOnce(&WebView) -> T) -> Option<T> {
        self.open.borrow().as_ref().map(|o| f(&o.view))
    }
}

fn navigate(
    view: &WebView,
    document: &RefCell<Document>,
    serial: &Cell<u64>,
    html: String,
    wanted: bool,
) -> wry::Result<()> {
    serial.set(serial.get() + 1);
    let url = format!("{DOCUMENT_ORIGIN}{}", serial.get());
    *document.borrow_mut() = Document { url: url.clone(), html, wanted };
    view.load_url(&url)
}

/// What it takes to bring a loaded page into view.
struct Reveal {
    document: Rc<RefCell<Document>>,
    open: Rc<RefCell<Option<Open>>>,
    app: Rc<RefCell<slint::Weak<App>>>,
    placement: Rc<Cell<Placement>>,
}

impl Reveal {
    /// Brings the page in once it has drawn a frame. "Loaded" does not mean that, and until then
    /// the view would show its previous page. WebView2 has no first-paint event, but a capture
    /// only completes after a frame of the current page, so asking for one is the signal.
    fn after_first_frame(&self, url: String) {
        let (document, open, app, placement) =
            (self.document.clone(), self.open.clone(), self.app.clone(), self.placement.clone());
        let bring_in = move || {
            // A page the user has already left finishes too, and must stay out of sight.
            if url != document.borrow().url || !document.borrow().wanted {
                return;
            }
            let Some(moved) = open.borrow().as_ref().map(|o| o.view.set_bounds(rect(placement.get().slot))) else {
                return;
            };
            drop(moved);
            placement.set(Placement { revealed: true, ..placement.get() });
            if let Some(app) = app.borrow().upgrade() {
                app.set_pane(Pane::Html);
            }
        };
        let open = self.open.borrow();
        let Some(open) = open.as_ref() else { return };
        let asked = unsafe {
            SHCreateMemStream(None).ok_or(windows::core::Error::empty()).and_then(|stream| {
                let done = CapturePreviewCompletedHandler::create(Box::new(move |_| {
                    bring_in();
                    Ok(())
                }));
                open.view.webview().CapturePreview(COREWEBVIEW2_CAPTURE_PREVIEW_IMAGE_FORMAT_JPEG, &stream, &done)
            })
        };
        debug_assert!(asked.is_ok(), "a capture could not start: {asked:?}");
    }
}

fn slot(app: &App) -> Bounds {
    let s = app.window().scale_factor();
    (app.get_slot_x() * s, app.get_slot_y() * s, app.get_slot_w() * s, app.get_slot_h() * s)
}

impl HtmlPane for Rc<Webview> {
    fn show(&self, app: &App, html: String) -> bool {
        // What shows before the first document paints, so dark mode never flashes white.
        let sheet = app.global::<Theme>().get_sheet().color();
        self.backdrop.set((sheet.red(), sheet.green(), sheet.blue()));
        *self.app.borrow_mut() = app.as_weak();
        self.load(app.window(), slot(app), html).is_ok()
    }

    fn hide(&self) {
        let placement = self.placement.get();
        self.placement.set(Placement { revealed: false, ..placement });
        // Also stops a page that is still loading from being revealed.
        self.document.borrow_mut().wanted = false;
        let Some(moved) = self.open.borrow().as_ref().map(|o| {
            // Takes the keyboard back from the page, which a click on Slint alone does not do.
            let _ = o.view.focus_parent();
            o.view.set_bounds(out_of_sight(placement.slot))
        }) else {
            return;
        };
        drop(moved);
        let (open, document, serial) = (self.open.clone(), self.document.clone(), self.serial.clone());
        self.empty.start(TimerMode::SingleShot, EMPTY_AFTER, move || {
            if let Some(open) = open.borrow().as_ref() {
                let _ = navigate(&open.view, &document, &serial, String::new(), false);
            }
        });
        if !self.idle.running() {
            let closing = self.open.clone();
            self.idle.start(TimerMode::SingleShot, IDLE_CLOSE, move || {
                closing.borrow_mut().take();
            });
        }
    }

    fn bounds_changed(&self, app: &App) {
        let bounds = slot(app);
        let placement = self.placement.get();
        self.placement.set(Placement { slot: bounds, ..placement });
        if placement.revealed
            && let Some(open) = self.open.borrow().as_ref()
        {
            let _ = open.view.set_bounds(rect(bounds));
        }
    }
}

/// Every setting and hook the security design depends on.
fn lock_down(
    view: &WebView,
    policy: &str,
    document: Rc<RefCell<Document>>,
    activity: Rc<Activity>,
) -> windows::core::Result<()> {
    unsafe {
        let core = view.webview();
        let s = core.Settings()?;
        s.SetIsScriptEnabled(false)?;
        s.SetIsWebMessageEnabled(false)?;
        s.SetAreHostObjectsAllowed(false)?;
        s.SetAreDefaultScriptDialogsEnabled(false)?;
        s.SetAreDefaultContextMenusEnabled(false)?;
        s.SetAreDevToolsEnabled(false)?;
        s.SetIsStatusBarEnabled(false)?;
        s.cast::<ICoreWebView2Settings3>()?.SetAreBrowserAcceleratorKeysEnabled(false)?;
        let s4 = s.cast::<ICoreWebView2Settings4>()?;
        s4.SetIsPasswordAutosaveEnabled(false)?;
        s4.SetIsGeneralAutofillEnabled(false)?;

        let env = view.environment();
        let headers = HSTRING::from(format!(
            "Content-Type: text/html; charset=utf-8\r\nCache-Control: no-store\r\nContent-Security-Policy: {policy}"
        ));
        let mut token = Default::default();

        // Only our own document may load. A link opens in the browser, but only for a real
        // click: if a redirect ever got past the sanitiser, it must not be able to open a URL.
        let (allowed, cancelled) = (document.clone(), activity.clone());
        core.add_NavigationStarting(
            &NavigationStartingEventHandler::create(Box::new(move |_, args| {
                let Some(args) = args else { return Ok(()) };
                let mut uri = PWSTR::null();
                args.Uri(&mut uri)?;
                let uri = take_pwstr(uri);
                if uri != allowed.borrow().url {
                    args.SetCancel(true)?;
                    let mut by_user = BOOL::default();
                    args.IsUserInitiated(&mut by_user)?;
                    if by_user.as_bool() {
                        open_externally(&uri);
                    }
                    cancelled.cancelled_navigations.borrow_mut().push(uri);
                }
                Ok(())
            })),
            &mut token,
        )?;

        core.AddWebResourceRequestedFilter(w!("*"), COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL)?;
        core.add_WebResourceRequested(
            &WebResourceRequestedEventHandler::create(Box::new(move |_, args| {
                let Some(args) = args else { return Ok(()) };
                let mut uri = PWSTR::null();
                args.Request()?.Uri(&mut uri)?;
                let uri = take_pwstr(uri);
                let document = document.borrow();
                let response = if uri == document.url {
                    let stream = SHCreateMemStream(Some(document.html.as_bytes()));
                    env.CreateWebResourceResponse(stream.as_ref(), 200, w!("OK"), &headers)?
                } else {
                    activity.refused.borrow_mut().push(uri);
                    env.CreateWebResourceResponse(None, 403, w!("Blocked"), w!(""))?
                };
                args.SetResponse(&response)?;
                Ok(())
            })),
            &mut token,
        )?;

        // Esc leaves the page, so the list's keys work again without reaching for the mouse.
        let controller = view.controller();
        let parent_focus = controller.clone();
        controller.add_AcceleratorKeyPressed(
            &AcceleratorKeyPressedEventHandler::create(Box::new(move |_, args| {
                let Some(args) = args else { return Ok(()) };
                let mut kind = COREWEBVIEW2_KEY_EVENT_KIND::default();
                args.KeyEventKind(&mut kind)?;
                let mut key = 0u32;
                args.VirtualKey(&mut key)?;
                if kind == COREWEBVIEW2_KEY_EVENT_KIND_KEY_DOWN && key == VK_ESCAPE {
                    parent_focus.MoveFocus(COREWEBVIEW2_MOVE_FOCUS_REASON_PROGRAMMATIC)?;
                }
                Ok(())
            })),
            &mut token,
        )?;
    }
    Ok(())
}
