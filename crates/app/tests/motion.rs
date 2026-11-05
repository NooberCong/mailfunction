//! What the animations cost. Drives the real UI on a mock clock at 60 frames a second and times
//! the software renderer on each frame. Input goes straight into Slint, not to the desktop.
//!
//! The rule that matters is checked in every build: once motion ends, the app paints nothing.
//! The frame budget is checked in release builds, where the numbers mean something:
//!
//!     cargo test --release -p mailfunction --test motion -- --nocapture

use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use mailfunction::{App, Controller, HtmlPane, Pane, Theme};
use mailfunction_core::engine::Event;
use mailfunction_core::remote::Offline;
use mailfunction_core::store::{Account, AuthKind, LabelSpec, NewMessage, Reader, Role, Security, Store};
use slint::platform::software_renderer::{MinimalSoftwareWindow, PremultipliedRgbaColor, RepaintBufferType};
use slint::platform::{Platform, PlatformError, PointerEventButton, WindowAdapter, WindowEvent};
use slint::{ComponentHandle, LogicalPosition, PhysicalSize};

/// A 1080p screen at 150%, the common laptop case, with the window filling it.
const SIZE: (u32, u32) = (1920, 1080);
const SCALE: f32 = 1.5;
const FRAME: Duration = Duration::from_micros(16_667);
/// A third of a 60 Hz frame, so that animation leaves the UI thread mostly free.
const AVERAGE_BUDGET_MS: f64 = 5.5;

struct Clock {
    window: Rc<MinimalSoftwareWindow>,
    now: Rc<Cell<Duration>>,
}

impl Platform for Clock {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, PlatformError> {
        Ok(self.window.clone())
    }

    fn duration_since_start(&self) -> Duration {
        self.now.get()
    }
}

struct NoHtml;

impl HtmlPane for NoHtml {
    fn show(&self, app: &App, _: String) -> bool {
        app.set_pane(Pane::Html);
        true
    }
    fn hide(&self) {}
    fn bounds_changed(&self, _: &App) {}
}

struct Stage {
    window: Rc<MinimalSoftwareWindow>,
    now: Rc<Cell<Duration>>,
    pixels: Vec<PremultipliedRgbaColor>,
}

#[derive(Default)]
struct Cost {
    frames: u32,
    total: Duration,
    worst: Duration,
    area: u64,
}

impl Cost {
    fn average_ms(&self) -> f64 {
        self.total.as_secs_f64() * 1000.0 / f64::from(self.frames.max(1))
    }

    /// The share of the window repainted on an average frame, in percent.
    fn share(&self) -> f64 {
        self.area as f64 / f64::from(self.frames.max(1)) / f64::from(SIZE.0 * SIZE.1) * 100.0
    }
}

impl Stage {
    /// Advances one frame and paints whatever changed.
    fn step(&mut self, cost: &mut Cost) {
        self.now.set(self.now.get() + FRAME);
        slint::platform::update_timers_and_animations();
        let pixels = &mut self.pixels;
        let started = Instant::now();
        let mut area = None;
        self.window.draw_if_needed(|renderer| {
            let region = renderer.render(pixels, SIZE.0 as usize);
            area = Some(region.iter().map(|(_, s)| u64::from(s.width) * u64::from(s.height)).sum::<u64>());
        });
        let spent = started.elapsed();
        if let Some(area) = area {
            cost.frames += 1;
            cost.total += spent;
            cost.worst = cost.worst.max(spent);
            cost.area += area;
        }
    }

    /// Runs until the UI goes quiet, and reports what it cost to get there.
    fn measure(&mut self, name: &str) -> Cost {
        let mut cost = Cost::default();
        for _ in 0..4 {
            self.step(&mut cost);
        }
        let mut limit = 120;
        while self.window.has_active_animations() && limit > 0 {
            self.step(&mut cost);
            limit -= 1;
        }
        assert!(limit > 0, "{name}: still animating after two seconds");
        // The frame that lands on the final value comes after the animation reports it is done.
        self.step(&mut cost);
        self.report(name, &cost);
        cost
    }

    fn report(&self, name: &str, cost: &Cost) {
        println!(
            "{name:<22} {:>3} frames, {:>5.2} ms average, {:>5.2} ms worst, {:>4.1}% of the window repainted",
            cost.frames,
            cost.average_ms(),
            cost.worst.as_secs_f64() * 1000.0,
            cost.share(),
        );
        if !cfg!(debug_assertions) {
            assert!(cost.average_ms() < AVERAGE_BUDGET_MS, "{name} is over budget");
        }
    }

    /// The rule: nothing moving means nothing painted.
    fn assert_idle(&mut self, after: &str) {
        let mut cost = Cost::default();
        for _ in 0..30 {
            self.step(&mut cost);
        }
        assert_eq!(cost.frames, 0, "painted while idle after {after}");
        assert!(!self.window.has_active_animations(), "an animation is left running after {after}");
    }
}

fn seed(dir: &std::path::Path) -> (Store, i64) {
    let store = Store::open(dir).unwrap();
    let account = store
        .add_account(&Account {
            id: 0,
            email: "kong@example.com".into(),
            host: "localhost".into(),
            port: 993,
            security: Security::Tls,
            auth: AuthKind::Google,
        })
        .unwrap();
    let inbox = store.folder(account, "INBOX", &LabelSpec::system(Role::Inbox)).unwrap();
    store.folder(account, "Sent", &LabelSpec::system(Role::Sent)).unwrap();
    let messages: Vec<_> = (0..500u32)
        .map(|i| NewMessage {
            uid: i + 1,
            remote_id: format!("m{i}"),
            gm_thread_id: None,
            subject: format!("Lịch họp tuần sau và tài liệu cần chuẩn bị {i}"),
            from_name: format!("Nguyễn Thị Minh Anh {i}"),
            from_addr: format!("person{i}@example.vn"),
            date: 1_793_000_000 - i64::from(i) * 3600,
            size: 1000,
            seen: i % 3 != 0,
            flagged: i % 7 == 0,
            labels: vec![],
        })
        .collect();
    store.save_messages(&inbox, &messages).unwrap();
    let reader = Reader::open(dir).unwrap();
    for row in reader.messages(inbox.label_id, 0, 100).unwrap() {
        let body = format!("From: a@example.vn\r\nTo: b@example.vn\r\nContent-Type: text/plain; charset=utf-8\r\n\r\n{}", "Chào anh, em gửi lịch họp tuần sau.\r\n".repeat(30));
        store.save_body(row.id, body.as_bytes()).unwrap();
    }
    (store, account)
}

/// The id of the message at a row of the only account's Inbox.
fn reader_id(dir: &std::path::Path, row: usize) -> i64 {
    let reader = Reader::open(dir).unwrap();
    let account = reader.accounts().unwrap()[0].id;
    let inbox = reader.labels(account).unwrap()[0].id;
    reader.messages(inbox, row, 1).unwrap()[0].id
}

#[test]
fn motion_is_cheap_and_stops() {
    let window = MinimalSoftwareWindow::new(RepaintBufferType::ReusedBuffer);
    let now = Rc::new(Cell::new(Duration::ZERO));
    slint::platform::set_platform(Box::new(Clock { window: window.clone(), now: now.clone() })).unwrap();

    let dir = std::env::temp_dir().join(format!("mailfunction-motion-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let (store, account) = seed(&dir);
    let app = App::new().unwrap();
    window.dispatch_event(WindowEvent::ScaleFactorChanged { scale_factor: SCALE });
    window.set_size(PhysicalSize::new(SIZE.0, SIZE.1));
    let controller = Controller::new(app.clone_strong(), &dir, None, Box::new(NoHtml), Arc::new(Offline)).unwrap();
    app.show().unwrap();
    window.dispatch_event(WindowEvent::WindowActiveChanged(true));
    controller.load_accounts().unwrap();

    let pixels = vec![PremultipliedRgbaColor::default(); (SIZE.0 * SIZE.1) as usize];
    let mut stage = Stage { window: window.clone(), now, pixels };
    for _ in 0..30 {
        stage.step(&mut Cost::default());
    }
    stage.assert_idle("the first paint");

    let key = |text: &str| {
        window.dispatch_event(WindowEvent::KeyPressed { text: text.into() });
        window.dispatch_event(WindowEvent::KeyReleased { text: text.into() });
    };

    // Hover: the pointer moves onto a row.
    window.dispatch_event(WindowEvent::PointerMoved { position: LogicalPosition::new(420.0, 240.0) });
    stage.measure("hover a row");
    stage.assert_idle("hover");

    // Opening a message: the marker slides, and the header and body fade in.
    key("j");
    let opened = stage.measure("open a message");
    assert!(opened.frames > 5, "opening a message should animate");
    stage.assert_idle("opening a message");
    std::thread::sleep(Duration::from_millis(200));

    key("j");
    let next = stage.measure("next message");
    assert!(next.frames > 5 && next.share() > 20.0, "the reading pane should fade, not jump");
    stage.assert_idle("the next message");

    // The sidebar: its marker slides and the list fades in.
    controller.select_label(1);
    stage.measure("switch label");
    stage.assert_idle("switching label");
    controller.select_label(0);
    stage.measure("switch back");

    // Wheel scrolling through the list.
    let at = LogicalPosition::new(420.0, 300.0);
    let mut scroll = Cost::default();
    for _ in 0..30 {
        window.dispatch_event(WindowEvent::PointerScrolled { position: at, delta_x: 0.0, delta_y: -60.0 });
        stage.step(&mut scroll);
    }
    while window.has_active_animations() {
        stage.step(&mut scroll);
    }
    stage.step(&mut scroll);
    stage.report("wheel scroll", &scroll);
    stage.assert_idle("scrolling");

    // A click on a row.
    let row_before_click = app.get_selected_row();
    window.dispatch_event(WindowEvent::PointerPressed { position: at, button: PointerEventButton::Left });
    window.dispatch_event(WindowEvent::PointerReleased { position: at, button: PointerEventButton::Left });
    stage.measure("click a row");
    assert_ne!(app.get_selected_row(), row_before_click, "the click should select the row under it");
    stage.assert_idle("a click");

    // A message on its way: the wave runs until it arrives, and stops with it.
    controller.select_row(499);
    assert_eq!(app.get_pane(), Pane::Loading);
    let mut loading = Cost::default();
    for _ in 0..90 {
        stage.step(&mut loading);
    }
    assert!(loading.frames > 60, "the loading lines should be moving");
    stage.report("loading a message", &loading);
    controller.on_event(account, Event::BodyFailed(reader_id(&dir, 499)));
    for _ in 0..30 {
        stage.step(&mut Cost::default());
    }
    stage.assert_idle("loading");

    // The sync bar runs for as long as syncing does, and must stop with it.
    controller.on_event(account, Event::Syncing);
    let mut sync = Cost::default();
    for _ in 0..120 {
        stage.step(&mut sync);
    }
    assert!(sync.frames > 100, "the sync bar should be moving");
    stage.report("sync bar", &sync);
    controller.on_event(account, Event::UpToDate);
    // The list refresh that follows a sync runs on a timer, so wait it out before listening.
    for _ in 0..40 {
        stage.step(&mut Cost::default());
    }
    stage.assert_idle("sync");

    // With animations off in Windows, everything lands on the next frame.
    app.global::<Theme>().set_motion(false);
    std::thread::sleep(Duration::from_millis(200));
    key("k");
    let instant = stage.measure("reduced motion");
    assert!(instant.frames <= 6, "took {} frames with animations off", instant.frames);
    stage.assert_idle("reduced motion");

    drop(controller);
    drop(store);
    let _ = std::fs::remove_dir_all(&dir);
}
