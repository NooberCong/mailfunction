//! Where the window meets Windows: the title bar takes the app's colours, so the frame reads as
//! one piece while Snap Layouts, resizing and the caption buttons stay native.

use std::num::NonZeroIsize;
use std::time::Duration;

use raw_window_handle::{
    DisplayHandle, HandleError, HasDisplayHandle, HasWindowHandle, RawWindowHandle, Win32WindowHandle, WindowHandle,
};
use slint::winit_030::winit::event::{ElementState, WindowEvent};
use slint::winit_030::{EventResult, WinitWindowAccessor};
use mailfunction_core::settings::Placement;
use slint::{Color, ComponentHandle, PhysicalPosition, PhysicalSize};
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::{
    DWMWA_BORDER_COLOR, DWMWA_CAPTION_COLOR, DWMWA_TEXT_COLOR, DWMWA_USE_IMMERSIVE_DARK_MODE, DwmSetWindowAttribute,
    DWMWINDOWATTRIBUTE,
};
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, HMONITOR, MONITOR_DEFAULTTONEAREST, MONITOR_DEFAULTTOPRIMARY, MONITORINFO, MonitorFromPoint,
    MonitorFromRect,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::{AdjustWindowRectExForDpi, GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::Input::KeyboardAndMouse::SetFocus;
use windows::Win32::UI::WindowsAndMessaging::{
    GetSystemMetrics, GetWindowRect, HICON, ICON_BIG, ICON_SMALL, IMAGE_ICON, IsIconic, IsWindowVisible, IsZoomed,
    LR_DEFAULTCOLOR, LoadImageW, SM_CXICON, SM_CXSMICON, SPI_GETCLIENTAREAANIMATION, SW_HIDE, SW_RESTORE, SW_SHOW,
    SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, SendMessageW, SetForegroundWindow, ShowWindow, SystemParametersInfoW,
    WM_SETICON, WS_OVERLAPPEDWINDOW,
};
use windows::core::{BOOL, PCWSTR};

use crate::{App, Theme, on_ui_thread};

/// The resource ids given to the icons in `build.rs`.
pub(crate) const EXE_ICON: usize = 1;
pub(crate) const UNREAD_ICON: usize = 2;

fn colorref(color: Color) -> COLORREF {
    COLORREF(u32::from(color.red()) | u32::from(color.green()) << 8 | u32::from(color.blue()) << 16)
}

fn set<T>(hwnd: HWND, attribute: DWMWINDOWATTRIBUTE, value: &T) {
    // Colours need Windows 11. Elsewhere this fails and the title bar keeps the system look.
    let _ = unsafe { DwmSetWindowAttribute(hwnd, attribute, std::ptr::from_ref(value).cast(), size_of::<T>() as u32) };
}

pub(crate) fn hwnd(app: &App) -> Option<HWND> {
    let window = app.window().window_handle();
    match window.window_handle().ok()?.as_raw() {
        RawWindowHandle::Win32(win32) => Some(HWND(win32.hwnd.get() as *mut _)),
        _ => None,
    }
}

/// An icon embedded in the executable, at a size in pixels.
pub(crate) fn exe_icon(resource: usize, size: i32) -> Option<HICON> {
    let exe = unsafe { GetModuleHandleW(None) }.ok()?;
    let name = PCWSTR(resource as *const u16);
    let icon = unsafe { LoadImageW(Some(exe.into()), name, IMAGE_ICON, size, size, LR_DEFAULTCOLOR) }.ok()?;
    Some(HICON(icon.0))
}

/// Gives the window the icon embedded in the executable, for the title bar and taskbar.
pub fn set_icon(app: &App) {
    let Some(hwnd) = hwnd(app) else { return };
    // Each slot loads its own size: the title bar would otherwise shrink the large icon.
    for (slot, metric) in [(ICON_SMALL, SM_CXSMICON), (ICON_BIG, SM_CXICON)] {
        let Some(icon) = exe_icon(EXE_ICON, unsafe { GetSystemMetrics(metric) }) else { continue };
        unsafe { SendMessageW(hwnd, WM_SETICON, Some(WPARAM(slot as usize)), Some(LPARAM(icon.0 as isize))) };
    }
}

/// What a file dialog needs to sit on top of the window, from any thread.
pub(crate) fn dialog_parent(app: &App) -> DialogParent {
    DialogParent(hwnd(app).map(|hwnd| hwnd.0 as isize))
}

/// A window to own a dialog, which unlike the window itself can be handed to another thread.
pub(crate) struct DialogParent(Option<isize>);

impl HasWindowHandle for DialogParent {
    fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
        let handle = self.0.and_then(NonZeroIsize::new).ok_or(HandleError::Unavailable)?;
        // The window outlives every dialog it owns, which close with it.
        Ok(unsafe { WindowHandle::borrow_raw(RawWindowHandle::Win32(Win32WindowHandle::new(handle))) })
    }
}

impl HasDisplayHandle for DialogParent {
    fn display_handle(&self) -> Result<DisplayHandle<'_>, HandleError> {
        Ok(DisplayHandle::windows())
    }
}

/// Long enough after the last frame for Slint's clock to have moved on from it.
const LAST_FRAME_GONE: Duration = Duration::from_millis(50);

/// How much of the screen the window takes the first time it opens.
const FIRST_SHARE: f32 = 0.8;

/// The first window on a monitor: centred, with room around it.
fn first_placement(work: &RECT) -> Placement {
    let (width, height) = (work.right - work.left, work.bottom - work.top);
    let (own_width, own_height) = ((width as f32 * FIRST_SHARE) as i32, (height as f32 * FIRST_SHARE) as i32);
    Placement {
        left: work.left + (width - own_width) / 2,
        top: work.top + (height - own_height) / 2,
        width: own_width,
        height: own_height,
        maximized: false,
    }
}

/// Moves a window onto a monitor's free space, shrinking it only if it can't fit. Monitors and
/// the taskbar change between runs, so where it was left may no longer all be on screen.
fn fit(placement: Placement, work: &RECT) -> Placement {
    let width = placement.width.min(work.right - work.left);
    let height = placement.height.min(work.bottom - work.top);
    Placement {
        left: placement.left.clamp(work.left, work.right - width),
        top: placement.top.clamp(work.top, work.bottom - height),
        width,
        height,
        ..placement
    }
}

fn work_area(monitor: HMONITOR) -> Option<RECT> {
    let mut info = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
    unsafe { GetMonitorInfoW(monitor, &raw mut info) }.as_bool().then_some(info.rcWork)
}

/// What the frame adds to the inside of a window, across and down, on a monitor.
fn frame_size(monitor: HMONITOR) -> Option<(i32, i32)> {
    let (mut dpi, mut unused) = (0, 0);
    unsafe { GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &raw mut dpi, &raw mut unused) }.ok()?;
    let mut frame = RECT::default();
    unsafe { AdjustWindowRectExForDpi(&raw mut frame, WS_OVERLAPPEDWINDOW, false, Default::default(), dpi) }.ok()?;
    Some((frame.right - frame.left, frame.bottom - frame.top))
}

/// Sets where the window opens, from where it was left if that is known, and says where that is.
/// It has to come before the window shows, which is what keeps it from appearing and then jumping.
pub fn place(app: &App, saved: Option<Placement>) -> Option<Placement> {
    let monitor = match &saved {
        Some(s) => {
            let rect = RECT { left: s.left, top: s.top, right: s.left + s.width, bottom: s.top + s.height };
            unsafe { MonitorFromRect(&raw const rect, MONITOR_DEFAULTTONEAREST) }
        }
        None => unsafe { MonitorFromPoint(POINT::default(), MONITOR_DEFAULTTOPRIMARY) },
    };
    let work = work_area(monitor)?;
    let (frame_width, frame_height) = frame_size(monitor)?;
    let placement = fit(saved.unwrap_or_else(|| first_placement(&work)), &work);
    let window = app.window();
    window.set_position(PhysicalPosition::new(placement.left, placement.top));
    window.set_size(PhysicalSize::new((placement.width - frame_width) as u32, (placement.height - frame_height) as u32));
    window.set_maximized(placement.maximized);
    Some(placement)
}

/// Where the window is now, given where it was. Maximized, its size to come back to stays as it
/// was, and minimized there is nothing to learn: nobody wants a mail app to open that way.
pub(crate) fn placement(app: &App, last: Placement) -> Option<Placement> {
    let hwnd = hwnd(app)?;
    if unsafe { IsIconic(hwnd) }.as_bool() {
        return None;
    }
    if unsafe { IsZoomed(hwnd) }.as_bool() {
        return Some(Placement { maximized: true, ..last });
    }
    let mut frame = RECT::default();
    unsafe { GetWindowRect(hwnd, &raw mut frame) }.ok()?;
    Some(Placement {
        left: frame.left,
        top: frame.top,
        width: frame.right - frame.left,
        height: frame.bottom - frame.top,
        maximized: false,
    })
}

/// Takes the window off the screen and the taskbar, at once, and then tells Slint.
///
/// Windows sends a hidden window no paint message, so a redraw Slint asks for there is never
/// served, and it would keep asking at the display's refresh rate until the window came back. Its
/// own hide cancels a request, but not one made after it: a frame still animating asks again as
/// the loop goes idle, and so does the next change of anything on screen. So Slint hears of this
/// only once no frame has been drawn for a while, which ends the animations, and the change made
/// here is the one it asks about. It asks only once between frames, so nothing can ask again.
pub(crate) fn hide(app: &App) {
    let Some(window) = hwnd(app) else { return };
    let _ = unsafe { ShowWindow(window, SW_HIDE) };
    let app = app.as_weak();
    slint::Timer::single_shot(LAST_FRAME_GONE, move || {
        let Some(app) = app.upgrade() else { return };
        // It may be back already, which leaves Slint with nothing to hear.
        if hwnd(&app).is_some_and(|window| !unsafe { IsWindowVisible(window) }.as_bool()) {
            app.invoke_repaint();
            let _ = app.hide();
        }
    });
}

/// Brings the window to the front, from wherever it was. Returns whether it had been hidden.
pub(crate) fn raise(app: &App) -> bool {
    let Some(hwnd) = hwnd(app) else { return false };
    unsafe {
        let hidden = !IsWindowVisible(hwnd).as_bool();
        // A window started in the background has yet to be shown even once, which is Slint's job.
        let _ = app.show();
        if hidden {
            let _ = ShowWindow(hwnd, SW_SHOW);
            // Windows dropped what the window showed, which Slint knows only if it heard of the hide.
            app.invoke_repaint();
        }
        if IsIconic(hwnd).as_bool() {
            let _ = ShowWindow(hwnd, SW_RESTORE);
        }
        let _ = SetForegroundWindow(hwnd);
        hidden
    }
}

/// Keeps the unread count on the taskbar from now on, on the button and in the notification area.
pub fn watch_window(app: &App) {
    let weak = app.as_weak();
    app.window().on_winit_window_event(move |_, event| {
        match event {
            // A press that gets here missed the webviews, and one of them may still have the
            // keyboard: Slint moves its focus only among its own items, and Windows moves it for
            // no window on a click. The window takes it, whatever the press lands on.
            WindowEvent::MouseInput { state: ElementState::Pressed, .. } => {
                if let Some(hwnd) = weak.upgrade().as_ref().and_then(hwnd) {
                    let _ = unsafe { SetFocus(Some(hwnd)) };
                }
            }
            WindowEvent::Moved(_) | WindowEvent::Resized(_) => on_ui_thread(|this| this.window_changed()),
            // A file dropped on the window goes on the message being written.
            WindowEvent::DroppedFile(path) => {
                let path = path.clone();
                on_ui_thread(move |this| this.attach_paths(&[path]));
            }
            _ => {}
        }
        EventResult::Propagate
    });
}

pub fn watch_taskbar(app: &App) {
    if let Some(hwnd) = hwnd(app) {
        crate::badge::watch(hwnd);
        crate::tray::watch(hwnd);
    }
}

/// Paints the title bar to match the frame. The window must be showing.
pub fn paint_title_bar(app: &App) {
    let Some(hwnd) = hwnd(app) else { return };
    let frame = colorref(app.global::<Theme>().get_frame_top());
    // Light glyphs on the caption buttons, since the frame is dark in both themes.
    set(hwnd, DWMWA_USE_IMMERSIVE_DARK_MODE, &BOOL::from(true));
    set(hwnd, DWMWA_CAPTION_COLOR, &frame);
    set(hwnd, DWMWA_BORDER_COLOR, &frame);
    set(hwnd, DWMWA_TEXT_COLOR, &COLORREF(0x00FF_FFFF));
}

/// Whether Windows is set to show animations.
pub fn animations_enabled() -> bool {
    let mut enabled = BOOL::from(true);
    let read = unsafe {
        SystemParametersInfoW(
            SPI_GETCLIENTAREAANIMATION,
            0,
            Some(std::ptr::from_mut(&mut enabled).cast()),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
    };
    read.is_err() || enabled.as_bool()
}

#[cfg(test)]
mod tests {
    use super::*;

    const WORK: RECT = RECT { left: 0, top: 0, right: 2560, bottom: 1392 };

    fn at(left: i32, top: i32, width: i32, height: i32) -> Placement {
        Placement { left, top, width, height, maximized: false }
    }

    #[test]
    fn a_window_on_screen_stays_exactly_where_it_was() {
        assert_eq!(fit(at(100, 50, 1600, 900), &WORK), at(100, 50, 1600, 900));
    }

    #[test]
    fn a_window_past_an_edge_comes_back_whole() {
        assert_eq!(fit(at(100, 800, 1600, 900), &WORK), at(100, 492, 1600, 900));
        assert_eq!(fit(at(-3000, -40, 1600, 900), &WORK), at(0, 0, 1600, 900));
    }

    #[test]
    fn a_window_too_big_for_the_monitor_shrinks_to_it() {
        assert_eq!(fit(at(500, 500, 3840, 2100), &WORK), at(0, 0, 2560, 1392));
    }

    #[test]
    fn the_first_window_is_centred_on_a_monitor_wherever_it_sits() {
        let work = RECT { left: 2560, top: 100, right: 3560, bottom: 1100 };
        assert_eq!(first_placement(&work), at(2660, 200, 800, 800));
    }
}
