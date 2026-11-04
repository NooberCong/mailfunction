//! Scripted gate test. Prints `RESULT name value` lines and writes BMP evidence to `SHOT_DIR`.

use std::cell::Cell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use futures_channel::oneshot;
use slint::{ComponentHandle, PhysicalPosition, PhysicalSize};
use windows::Win32::Foundation::HWND;

use crate::server::Canary;
use crate::{MenuWindow, Shell, slot_rect, win};

fn sleep(ms: u64) -> impl Future<Output = ()> {
    let (tx, rx) = oneshot::channel();
    slint::Timer::single_shot(Duration::from_millis(ms), move || {
        let _ = tx.send(());
    });
    async move {
        let _ = rx.await;
    }
}

fn result(name: &str, value: impl std::fmt::Display) {
    println!("RESULT {name} {value}");
}

fn memory(label: &str) {
    let m = win::tree_mem();
    result(
        &format!("mem.{label}"),
        format_args!(
            "processes={} unreadable={} self_mb={:.1} tree_mb={:.1}",
            m.processes, m.unreadable, m.self_mb, m.total_mb
        ),
    );
}

async fn wait_loaded(shell: &Shell, target: u32) -> bool {
    for _ in 0..200 {
        if shell.events.loads_finished.get() >= target {
            return true;
        }
        sleep(50).await;
    }
    false
}

fn slot_screen(shell: &Shell, hwnd: HWND, dx: f32, dy: f32) -> (i32, i32) {
    let (x, y, _, _) = slot_rect(&shell.app);
    let s = shell.app.window().scale_factor();
    win::client_to_screen(hwnd, (x + dx * s) as i32, (y + dy * s) as i32)
}

const MAGENTA: (u8, u8, u8) = (255, 0, 255);

fn probe(name: &str, (x, y): (i32, i32)) {
    let px = win::pixel(x, y);
    result(name, format_args!("on_top={} pixel={px:?} owner={}", px == MAGENTA, win::owner_at(x, y)));
}

pub async fn run(shell: Rc<Shell>, menu: MenuWindow, canary: Canary, page: String, started: Instant) {
    let dir = std::env::var("SHOT_DIR").unwrap_or_else(|_| "shots".into());
    let _ = std::fs::create_dir_all(&dir);
    let app = &shell.app;
    result("startup_to_loop_ms", started.elapsed().as_millis());
    app.set_pinned(true);
    sleep(1500).await;
    let hwnd = win::hwnd_of(app.window());

    result("backend", std::env::var("SLINT_BACKEND").unwrap_or_else(|_| "default".into()));
    result("scale_factor", app.window().scale_factor());
    memory("before_webview");

    // Control: the inline popup must be visible at the probe point with no webview present,
    // or a failed probe later says nothing about the webview.
    let _ = win::bring_to_front(hwnd);
    let popup_center = slot_screen(&shell, hwnd, 130.0, 210.0);
    app.invoke_show_popup();
    sleep(500).await;
    probe("popup.control_no_webview", popup_center);
    app.invoke_hide_popup();
    sleep(300).await;

    // Lazy creation from inside the running Slint event loop. The heartbeat shows whether
    // Slint keeps running during creation or the UI freezes.
    let beat = Rc::new(Cell::new(Instant::now()));
    let worst = Rc::new(Cell::new(Duration::ZERO));
    let ticks = Rc::new(Cell::new(0u32));
    let heartbeat = slint::Timer::default();
    let (b, wst, tk) = (beat.clone(), worst.clone(), ticks.clone());
    heartbeat.start(slint::TimerMode::Repeated, Duration::from_millis(10), move || {
        wst.set(wst.get().max(b.get().elapsed()));
        b.set(Instant::now());
        tk.set(tk.get() + 1);
    });
    sleep(100).await;
    worst.set(Duration::ZERO);
    ticks.set(0);
    let t = Instant::now();
    if let Err(e) = shell.open(true, &page) {
        result("create", format_args!("FAILED {e}"));
        return;
    }
    result("cold.create_ms", t.elapsed().as_millis());
    let stall_in_create = beat.get().elapsed().max(worst.get());
    result("cold.ui_stall_ms", format_args!("{} ticks_during_create={}", stall_in_create.as_millis(), ticks.get()));
    heartbeat.stop();
    result("cold.loaded", wait_loaded(&shell, 1).await);
    result("cold.open_total_ms", t.elapsed().as_millis());
    sleep(2500).await;
    memory("webview_open");
    win::screenshot(hwnd, &format!("{dir}/01-open.bmp"));

    // Lockdown: zero server hits, no script, and the requests were seen and denied.
    let hits = canary.take();
    let title = shell.host.borrow().as_ref().map(|h| h.title()).unwrap_or_default();
    result("locked.server_hits", format_args!("{} {hits:?}", hits.len()));
    result("locked.title", &title);
    result("locked.denied_requests", format_args!("{:?}", shell.events.denied.borrow()));
    result("locked.navigations", format_args!("{:?}", shell.events.navigations.borrow()));

    // Resize tracking.
    for (w, h) in [(900u32, 600u32), (1300, 820), (1000, 700)] {
        app.window().set_size(PhysicalSize::new(w, h));
        sleep(500).await;
        let (sx, sy, sw, sh) = slot_rect(app);
        let (ex, ey) = win::client_to_screen(hwnd, sx as i32, sy as i32);
        let child = win::first_child_rect(hwnd);
        let bounds = shell.host.borrow().as_ref().map(|h| h.bounds());
        let aligned = child.is_some_and(|c| {
            (c.left - ex).abs() <= 1
                && (c.top - ey).abs() <= 1
                && (c.right - c.left - sw as i32).abs() <= 1
                && (c.bottom - c.top - sh as i32).abs() <= 1
        });
        result(
            &format!("resize.{w}x{h}"),
            format_args!("aligned={aligned} expected=({ex},{ey},{sw},{sh}) child={child:?} controller={bounds:?}"),
        );
    }

    // Popups over the webview.
    result("foreground", win::bring_to_front(hwnd));
    app.invoke_show_popup();
    sleep(500).await;
    probe("popup.inline_over_webview", popup_center);
    win::screenshot(hwnd, &format!("{dir}/02-inline-popup.bmp"));

    if let Some(h) = shell.host.borrow().as_ref() {
        let _ = h.view.set_visible(false);
    }
    app.window().request_redraw();
    sleep(400).await;
    probe("popup.inline_webview_hidden", popup_center);
    if let Some(h) = shell.host.borrow().as_ref() {
        let _ = h.view.set_visible(true);
    }
    app.invoke_hide_popup();
    sleep(300).await;

    let _ = menu.show();
    menu.window().set_position(PhysicalPosition::new(popup_center.0 - 90, popup_center.1 - 60));
    sleep(500).await;
    probe("popup.toplevel_window", popup_center);
    win::screenshot(hwnd, &format!("{dir}/03-toplevel-popup.bmp"));
    let _ = menu.hide();
    sleep(300).await;

    // Focus and keyboard, with real input. Skipped unless we really hold the foreground.
    if win::bring_to_front(hwnd) {
        let saved = win::cursor();
        let (sx, sy, _, _) = slot_rect(app);
        let scale = app.window().scale_factor();
        let row = win::client_to_screen(hwnd, (100.0 * scale) as i32, (sy + 60.0 * scale) as i32);
        let body = win::client_to_screen(hwnd, (sx + 400.0 * scale) as i32, (sy + 300.0 * scale) as i32);
        let link = slot_screen(&shell, hwnd, 60.0, 40.0);

        // One click-then-keypress. Returns None if another app took the foreground, because
        // then the outcome says nothing about us.
        let step = async |target: (i32, i32), letter: char| -> Option<(bool, &'static str)> {
            if !win::bring_to_front(hwnd) {
                return None;
            }
            shell.keys.borrow_mut().clear();
            if !win::click(hwnd, target.0, target.1) {
                return None;
            }
            sleep(350).await;
            let focus = win::focus_owner();
            if !win::key(hwnd, letter.to_ascii_uppercase() as u16) {
                return None;
            }
            sleep(250).await;
            if !win::is_foreground(hwnd) {
                return None;
            }
            let received = shell.keys.borrow().iter().any(|k| k.eq_ignore_ascii_case(&letter.to_string()));
            Some((received, focus))
        };

        // Each row is: click the target, press a key, did Slint get it, who held OS focus.
        let trial = async |name: &str, fix: bool| {
            let (mut valid, mut invalid) = (0, 0);
            let (mut list_ok, mut web_owned, mut web_key_leaked) = (0, 0, 0);
            shell.focus_fix.set(fix);
            for _ in 0..5 {
                let in_webview = step(body, 'b').await;
                let on_list = step(row, 'c').await;
                let (Some((leaked, web_focus)), Some((got, _))) = (in_webview, on_list) else {
                    invalid += 1;
                    continue;
                };
                valid += 1;
                web_owned += (web_focus == "webview") as u32;
                web_key_leaked += leaked as u32;
                list_ok += got as u32;
            }
            shell.focus_fix.set(false);
            result(
                name,
                format_args!(
                    "valid={valid} invalid={invalid} webview_held_focus={web_owned}                      key_reached_slint_from_webview={web_key_leaked} key_reached_slint_after_list_click={list_ok}"
                ),
            );
        };

        trial("focus.no_fix", false).await;
        trial("focus.focus_parent_on_click", true).await;
        result("focus.accelerator_keys_seen", format_args!("{:?}", shell.events.accelerator_keys.borrow()));

        // A click on a link must not navigate.
        shell.events.navigations.borrow_mut().clear();
        canary.take();
        result("link.clicked", win::click(hwnd, link.0, link.1));
        sleep(800).await;
        result("link.navigations", format_args!("{:?}", shell.events.navigations.borrow()));
        result("link.new_windows", format_args!("{:?}", shell.events.new_windows.borrow()));
        result("link.server_hits", canary.take().len());
        win::move_cursor(saved.0, saved.1);
    } else {
        result("focus", "SKIPPED could not take foreground");
    }

    // Lifecycle: Close must really free the browser processes, and reopening must work.
    for cycle in 1..=3 {
        shell.close();
        sleep(2500).await;
        memory(&format!("closed_cycle{cycle}"));
        let before = shell.events.loads_finished.get();
        let t = Instant::now();
        if let Err(e) = shell.open(true, &page) {
            result("reopen", format_args!("FAILED {e}"));
            return;
        }
        let loaded = wait_loaded(&shell, before + 1).await;
        result(&format!("reopen.cycle{cycle}_ms"), format_args!("{} loaded={loaded}", t.elapsed().as_millis()));
        sleep(1200).await;
    }
    shell.close();
    sleep(1500).await;

    // Control: the same page with lockdown off must hit the server, or zero hits above prove nothing.
    canary.take();
    let before = shell.events.loads_finished.get();
    match shell.open(false, &page) {
        Ok(()) => {
            wait_loaded(&shell, before + 1).await;
            sleep(800).await;
            let title = shell.host.borrow().as_ref().map(|h| h.title()).unwrap_or_default();
            let hits = canary.take();
            result("control.server_hits", format_args!("{} {hits:?}", hits.len()));
            result("control.title", title);
        }
        Err(e) => result("control", format_args!("FAILED {e}")),
    }
    shell.close();
    sleep(1500).await;
    memory("end");
    result("done", true);
}
