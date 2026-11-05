//! Where the window meets Windows: the title bar takes the app's colours, so the frame reads as
//! one piece while Snap Layouts, resizing and the caption buttons stay native.

use std::num::NonZeroIsize;

use raw_window_handle::{
    DisplayHandle, HandleError, HasDisplayHandle, HasWindowHandle, RawWindowHandle, Win32WindowHandle, WindowHandle,
};
use slint::{Color, ComponentHandle};
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, WPARAM};
use windows::Win32::Graphics::Dwm::{
    DWMWA_BORDER_COLOR, DWMWA_CAPTION_COLOR, DWMWA_TEXT_COLOR, DWMWA_USE_IMMERSIVE_DARK_MODE, DwmSetWindowAttribute,
    DWMWINDOWATTRIBUTE,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    GetSystemMetrics, HICON, ICON_BIG, ICON_SMALL, IMAGE_ICON, IsIconic, IsWindowVisible, LR_DEFAULTCOLOR, LoadImageW,
    SM_CXICON, SM_CXSMICON, SPI_GETCLIENTAREAANIMATION, SW_HIDE, SW_RESTORE, SW_SHOW,
    SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, SendMessageW, SetForegroundWindow, ShowWindow, SystemParametersInfoW,
    WM_SETICON,
};
use windows::core::{BOOL, PCWSTR};

use crate::{App, Theme};

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

/// Takes the window off the screen and the taskbar. Slint's own hide would destroy the native
/// window, and everything that hangs from it: the webview, the tray icon and their listeners.
pub(crate) fn hide(app: &App) {
    if let Some(hwnd) = hwnd(app) {
        let _ = unsafe { ShowWindow(hwnd, SW_HIDE) };
    }
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
        }
        if IsIconic(hwnd).as_bool() {
            let _ = ShowWindow(hwnd, SW_RESTORE);
        }
        let _ = SetForegroundWindow(hwnd);
        hidden
    }
}

/// Keeps the unread count on the taskbar from now on, on the button and in the notification area.
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
