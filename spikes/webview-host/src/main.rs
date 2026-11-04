//! Phase 0 gate: can a wry WebView2 live inside a Slint window on Windows?
//!
//! `webview-host`             interactive window, driven with the buttons
//! `webview-host --selftest`  scripted run that injects real input while it holds the foreground

mod host;
mod selftest;
mod server;
mod win;

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use host::{Events, Host, physical_rect};
use slint::{ComponentHandle, Model, ModelNotify, ModelRc, ModelTracker, SharedString};

slint::include_modules!();

/// Rows are computed on demand, so 100k rows cost no memory until they are scrolled into view.
struct Rows {
    count: usize,
    notify: ModelNotify,
}

impl Model for Rows {
    type Data = SharedString;
    fn row_count(&self) -> usize {
        self.count
    }
    fn row_data(&self, row: usize) -> Option<SharedString> {
        (row < self.count).then(|| format!("Message {row}").into())
    }
    fn model_tracker(&self) -> &dyn ModelTracker {
        &self.notify
    }
}

/// The slot rectangle in physical pixels, relative to the window client area.
pub fn slot_rect(app: &App) -> (f32, f32, f32, f32) {
    let s = app.window().scale_factor();
    (app.get_slot_x() * s, app.get_slot_y() * s, app.get_slot_w() * s, app.get_slot_h() * s)
}

pub struct Shell {
    pub app: App,
    pub host: RefCell<Option<Host>>,
    pub events: Rc<Events>,
    pub keys: RefCell<Vec<String>>,
    pub focus_fix: Cell<bool>,
}

impl Shell {
    pub fn open(&self, locked: bool, html: &str) -> Result<(), Box<dyn std::error::Error>> {
        if self.host.borrow().is_none() {
            let (x, y, w, h) = slot_rect(&self.app);
            let created = Host::create(self.app.window(), physical_rect(x, y, w, h), locked, self.events.clone())?;
            *self.host.borrow_mut() = Some(created);
        }
        self.host.borrow().as_ref().expect("host").view.load_html(html)?;
        Ok(())
    }

    pub fn close(&self) {
        self.host.borrow_mut().take();
    }

    fn sync_bounds(&self) {
        if let Some(host) = self.host.borrow().as_ref() {
            let (x, y, w, h) = slot_rect(&self.app);
            let _ = host.view.set_bounds(physical_rect(x, y, w, h));
        }
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let started = std::time::Instant::now();
    let selftest = std::env::args().any(|a| a == "--selftest");

    let app = App::new()?;
    app.set_rows(ModelRc::new(Rows { count: 100_000, notify: ModelNotify::default() }));
    let menu = MenuWindow::new()?;
    let shell = Rc::new(Shell {
        app: app.clone_strong(),
        host: RefCell::new(None),
        events: Rc::new(Events::default()),
        keys: RefCell::new(Vec::new()),
        focus_fix: Cell::new(false),
    });
    let canary = server::Canary::start();
    let page = server::hostile_page(canary.port);

    let s = shell.clone();
    app.on_slot_changed(move || s.sync_bounds());
    let s = shell.clone();
    app.on_key(move |text| {
        s.keys.borrow_mut().push(text.to_string());
        let count = s.keys.borrow().len();
        s.app.set_last_key(format!("Slint got key: {text} (#{count})").into());
    });
    let s = shell.clone();
    app.on_pane_clicked(move || {
        if s.focus_fix.get() || s.app.get_focus_fix() {
            if let Some(host) = s.host.borrow().as_ref() {
                let _ = host.view.focus_parent();
            }
        }
    });
    let s = shell.clone();
    let html = page.clone();
    app.on_open_html(move || match s.open(true, &html) {
        Ok(()) => s.app.set_status("Webview open".into()),
        Err(e) => s.app.set_status(format!("Failed: {e}").into()),
    });
    let s = shell.clone();
    app.on_close_webview(move || {
        s.close();
        s.app.set_status("No webview".into());
    });
    let weak_menu = menu.as_weak();
    app.on_open_menu(move || {
        if let Some(m) = weak_menu.upgrade() {
            let _ = if m.window().is_visible() { m.hide() } else { m.show() };
        }
    });

    app.show()?;
    if selftest {
        let shell = shell.clone();
        let menu = menu.clone_strong();
        slint::spawn_local(async move {
            selftest::run(shell, menu, canary, page, started).await;
            let _ = slint::quit_event_loop();
        })?;
    }
    slint::run_event_loop()?;
    Ok(())
}
