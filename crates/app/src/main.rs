#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use mailfunction::editor::EditorView;
use mailfunction::notify::Toasts;
use mailfunction::webview::Webview;
use mailfunction::{App, Controller, Theme, chrome, instance, on_ui_thread, startup};
use mailfunction_core::oauth;
use mailfunction_core::remote::Web;
use slint::ComponentHandle;
use slint::winit_030::{EventResult, WinitWindowAccessor, winit};

fn env_path(name: &str) -> PathBuf {
    std::env::var_os(name).map(PathBuf::from).unwrap_or_default()
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let data = match std::env::var_os("MAILFUNCTION_DATA_DIR") {
        Some(dir) => PathBuf::from(dir),
        None => env_path("LOCALAPPDATA").join("mailfunction"),
    };
    if !instance::claim(&data) {
        return Ok(());
    }
    let client_file = env_path("USERPROFILE").join(".config").join("mailfunction").join("google-client.json");

    let app = App::new()?;
    app.global::<Theme>().set_motion(chrome::animations_enabled());
    let client = oauth::Client::from_google_json(&client_file).ok();
    if client.is_none() {
        app.set_can_sign_in(false);
        app.set_sign_in_message(format!("Google client file not found:\n{}", client_file.display()).into());
    }
    let pane = Rc::new(Webview::new(data.join("webview")));
    let editor = Rc::new(EditorView::new(data.join("webview")));
    let toasts = Box::new(Toasts::new(&data));
    let controller = Controller::new(app.clone_strong(), &data, client, Box::new(pane), Box::new(editor), toasts, Arc::new(Web::default()))?;
    controller.load_accounts()?;
    controller.start_engine();
    app.on_start_with_windows_changed(startup::set);
    if controller.settings().start_with_windows {
        startup::set(true);
    }
    // Unshown, the native window still comes to exist as the loop starts, with nothing on screen.
    if !std::env::args().any(|argument| argument == startup::BACKGROUND) {
        app.show()?;
    }
    // Slint applies the window's own (empty) icon as the loop starts, so this has to come after.
    let window = app.as_weak();
    slint::invoke_from_event_loop(move || {
        if let Some(app) = window.upgrade() {
            chrome::set_icon(&app);
            chrome::paint_title_bar(&app);
            chrome::watch_taskbar(&app);
        }
        on_ui_thread(|this| this.refresh_badge());
    })?;
    // A file dropped on the window goes on the message being written.
    app.window().on_winit_window_event(|_, event| {
        if let winit::event::WindowEvent::DroppedFile(path) = event {
            let path = path.clone();
            on_ui_thread(move |this| this.attach_paths(&[path]));
        }
        EventResult::Propagate
    });
    // Until the app says so, as there may well be no window showing.
    slint::run_event_loop_until_quit()?;
    Ok(())
}
