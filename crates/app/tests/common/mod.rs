//! An offscreen window for driving the real UI. Key presses go straight into Slint, so no input
//! reaches the desktop. Screenshots land in `target/ui`.

#![allow(dead_code)]

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant};

use mailfunction::notify::{Notice, Notifier};
use mailfunction::{App, HtmlPane, Pane};
use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
use slint::platform::{Platform, PlatformError, PointerEventButton, WindowAdapter, WindowEvent};
use slint::{ComponentHandle, LogicalPosition};

struct Headless {
    window: Rc<MinimalSoftwareWindow>,
    started: Instant,
}

impl Platform for Headless {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, PlatformError> {
        Ok(self.window.clone())
    }

    fn duration_since_start(&self) -> Duration {
        self.started.elapsed()
    }
}

/// Installs the offscreen platform. Once per process, so one test per file.
pub fn offscreen() -> Rc<MinimalSoftwareWindow> {
    let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
    slint::platform::set_platform(Box::new(Headless { window: window.clone(), started: Instant::now() })).unwrap();
    window
}

#[derive(Default)]
pub struct RecordingPane {
    pub shown: Rc<RefCell<Vec<String>>>,
}

impl HtmlPane for RecordingPane {
    fn show(&self, app: &App, html: String) -> bool {
        self.shown.borrow_mut().push(html);
        app.set_pane(Pane::Html);
        true
    }
    fn hide(&self) {}
    fn bounds_changed(&self, _: &App) {}
}

/// Keeps what the app would have shown Windows.
#[derive(Default, Clone)]
pub struct RecordingNotifier {
    pub shown: Rc<RefCell<Vec<Notice>>>,
}

impl Notifier for RecordingNotifier {
    fn show(&self, notice: &Notice) {
        self.shown.borrow_mut().push(notice.clone());
    }
}

pub fn shot(app: &App, name: &str) {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/ui");
    std::fs::create_dir_all(&dir).unwrap();
    let buffer = app.window().take_snapshot().unwrap();
    let file = std::fs::File::create(dir.join(format!("{name}.png"))).unwrap();
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), buffer.width(), buffer.height());
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.write_header().unwrap().write_image_data(buffer.as_bytes()).unwrap();
}

pub fn press(app: &App, text: &str) {
    app.window().dispatch_event(WindowEvent::KeyPressed { text: text.into() });
    app.window().dispatch_event(WindowEvent::KeyReleased { text: text.into() });
    settle();
}

/// Types into whatever has the focus, one key at a time.
pub fn typed(app: &App, text: &str) {
    for c in text.chars() {
        let key: slint::SharedString = c.to_string().into();
        app.window().dispatch_event(WindowEvent::KeyPressed { text: key.clone() });
        app.window().dispatch_event(WindowEvent::KeyReleased { text: key });
    }
    settle();
}

/// Presses a key with Control held.
pub fn with_control(app: &App, text: &str) {
    let control: slint::SharedString = "\u{11}".into();
    app.window().dispatch_event(WindowEvent::KeyPressed { text: control.clone() });
    press(app, text);
    app.window().dispatch_event(WindowEvent::KeyReleased { text: control });
}

/// Runs the UI forward until every entrance and transition has finished.
pub fn settle() {
    for _ in 0..6 {
        slint::platform::update_timers_and_animations();
        std::thread::sleep(Duration::from_millis(60));
    }
    slint::platform::update_timers_and_animations();
}

/// Long enough for anything on a timer, such as a thread counting as read.
pub fn linger() {
    settle();
    settle();
    settle();
}

pub fn click(app: &App, x: f32, y: f32) {
    let position = LogicalPosition::new(x, y);
    let window = app.window();
    window.dispatch_event(WindowEvent::PointerMoved { position });
    window.dispatch_event(WindowEvent::PointerPressed { position, button: PointerEventButton::Left });
    window.dispatch_event(WindowEvent::PointerReleased { position, button: PointerEventButton::Left });
    settle();
}

pub fn fresh_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("mailfunction-ui-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}
