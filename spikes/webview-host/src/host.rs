//! The webview itself: lazy creation as a child of the Slint window, plus the lockdown.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use webview2_com::Microsoft::Web::WebView2::Win32::*;
use webview2_com::{AcceleratorKeyPressedEventHandler, FocusChangedEventHandler, WebResourceRequestedEventHandler, take_pwstr};
use windows::Win32::Foundation::RECT;
use windows::core::{Interface, PWSTR, w};
use wry::dpi::{PhysicalPosition, PhysicalSize};
use wry::{PageLoadEvent, Rect, WebView, WebViewBuilder, WebViewExtWindows};

#[derive(Default)]
pub struct Events {
    pub got_focus: Cell<u32>,
    pub lost_focus: Cell<u32>,
    pub loads_finished: Cell<u32>,
    pub navigations: RefCell<Vec<(String, bool)>>,
    pub new_windows: RefCell<Vec<String>>,
    pub denied: RefCell<Vec<String>>,
    /// Virtual keys seen by `AcceleratorKeyPressed` while the webview holds focus.
    pub accelerator_keys: RefCell<Vec<u32>>,
}

pub struct Host {
    pub view: WebView,
}

pub fn physical_rect(x: f32, y: f32, w: f32, h: f32) -> Rect {
    Rect {
        position: PhysicalPosition::new(x as i32, y as i32).into(),
        size: PhysicalSize::new(w as u32, h as u32).into(),
    }
}

fn is_initial_document(url: &str) -> bool {
    url.starts_with("about:") || url.starts_with("data:")
}

impl Host {
    pub fn create(
        window: &slint::Window,
        bounds: Rect,
        locked: bool,
        events: Rc<Events>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let handle = window.window_handle();
        let loads = events.clone();
        let navs = events.clone();
        let wins = events.clone();
        let mut builder = WebViewBuilder::new()
            .with_bounds(bounds)
            .with_focused(false)
            .with_devtools(false)
            .with_on_page_load_handler(move |event, _| {
                if matches!(event, PageLoadEvent::Finished) {
                    loads.loads_finished.set(loads.loads_finished.get() + 1);
                }
            })
            .with_navigation_handler(move |url| {
                let allow = !locked || is_initial_document(&url);
                navs.navigations.borrow_mut().push((url, allow));
                allow
            })
            .with_new_window_req_handler(move |url, _| {
                wins.new_windows.borrow_mut().push(url);
                wry::NewWindowResponse::Deny
            });
        if locked {
            builder = builder.with_javascript_disabled();
        }
        let view = builder.build_as_child(&handle)?;
        if locked {
            lock_down(&view, events.clone())?;
        }
        watch_focus(&view, events)?;
        Ok(Self { view })
    }

    pub fn bounds(&self) -> RECT {
        let mut r = RECT::default();
        unsafe {
            let _ = self.view.controller().Bounds(&mut r);
        }
        r
    }

    pub fn title(&self) -> String {
        let mut p = PWSTR::null();
        unsafe {
            let _ = self.view.webview().DocumentTitle(&mut p);
        }
        take_pwstr(p)
    }
}

/// Every setting and hook the report's security design depends on. Any error here is a
/// finding: it means wry does not give us the reach we need.
fn lock_down(view: &WebView, events: Rc<Events>) -> windows::core::Result<()> {
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
        let mut token = Default::default();
        core.AddWebResourceRequestedFilter(w!("*"), COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL)?;
        core.add_WebResourceRequested(
            &WebResourceRequestedEventHandler::create(Box::new(move |_, args| {
                let Some(args) = args else { return Ok(()) };
                let mut uri = PWSTR::null();
                args.Request()?.Uri(&mut uri)?;
                events.denied.borrow_mut().push(take_pwstr(uri));
                args.SetResponse(&env.CreateWebResourceResponse(None, 403, w!("Blocked"), w!(""))?)?;
                Ok(())
            })),
            &mut token,
        )?;
    }
    Ok(())
}

fn watch_focus(view: &WebView, events: Rc<Events>) -> windows::core::Result<()> {
    unsafe {
        let controller = view.controller();
        let mut token = Default::default();
        let keys = events.clone();
        controller.add_AcceleratorKeyPressed(
            &AcceleratorKeyPressedEventHandler::create(Box::new(move |_, args| {
                let Some(args) = args else { return Ok(()) };
                let mut kind = COREWEBVIEW2_KEY_EVENT_KIND::default();
                args.KeyEventKind(&mut kind)?;
                if kind == COREWEBVIEW2_KEY_EVENT_KIND_KEY_DOWN {
                    let mut vk = 0u32;
                    args.VirtualKey(&mut vk)?;
                    keys.accelerator_keys.borrow_mut().push(vk);
                }
                Ok(())
            })),
            &mut token,
        )?;
        let got = events.clone();
        controller.add_GotFocus(
            &FocusChangedEventHandler::create(Box::new(move |_, _| {
                got.got_focus.set(got.got_focus.get() + 1);
                Ok(())
            })),
            &mut token,
        )?;
        controller.add_LostFocus(
            &FocusChangedEventHandler::create(Box::new(move |_, _| {
                events.lost_focus.set(events.lost_focus.get() + 1);
                Ok(())
            })),
            &mut token,
        )?;
    }
    Ok(())
}
