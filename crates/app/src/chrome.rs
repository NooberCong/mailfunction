//! Where the window meets Windows: the title bar takes the app's colours, so the frame reads as
//! one piece while Snap Layouts, resizing and the caption buttons stay native.

use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use slint::{Color, ComponentHandle};
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, WPARAM};
use windows::Win32::Graphics::Dwm::{
    DWMWA_BORDER_COLOR, DWMWA_CAPTION_COLOR, DWMWA_TEXT_COLOR, DWMWA_USE_IMMERSIVE_DARK_MODE, DwmSetWindowAttribute,
    DWMWINDOWATTRIBUTE,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    GetSystemMetrics, ICON_BIG, ICON_SMALL, IMAGE_ICON, LR_DEFAULTCOLOR, LoadImageW, SM_CXICON, SM_CXSMICON,
    SPI_GETCLIENTAREAANIMATION, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, SendMessageW, SystemParametersInfoW, WM_SETICON,
};
use windows::core::{BOOL, PCWSTR};

use crate::{App, Theme};

/// The resource id given to the icon in `build.rs`.
const EXE_ICON: usize = 1;

fn colorref(color: Color) -> COLORREF {
    COLORREF(u32::from(color.red()) | u32::from(color.green()) << 8 | u32::from(color.blue()) << 16)
}

fn set<T>(hwnd: HWND, attribute: DWMWINDOWATTRIBUTE, value: &T) {
    // Colours need Windows 11. Elsewhere this fails and the title bar keeps the system look.
    let _ = unsafe { DwmSetWindowAttribute(hwnd, attribute, std::ptr::from_ref(value).cast(), size_of::<T>() as u32) };
}

fn hwnd(app: &App) -> Option<HWND> {
    let window = app.window().window_handle();
    match window.window_handle().ok()?.as_raw() {
        RawWindowHandle::Win32(win32) => Some(HWND(win32.hwnd.get() as *mut _)),
        _ => None,
    }
}

/// Gives the window the icon embedded in the executable, for the title bar and taskbar.
pub fn set_icon(app: &App) {
    let Some(hwnd) = hwnd(app) else { return };
    let resource = PCWSTR(EXE_ICON as *const u16);
    let Ok(exe) = (unsafe { GetModuleHandleW(None) }) else { return };
    // Each slot loads its own size: the title bar would otherwise shrink the large icon.
    for (slot, metric) in [(ICON_SMALL, SM_CXSMICON), (ICON_BIG, SM_CXICON)] {
        let size = unsafe { GetSystemMetrics(metric) };
        let Ok(icon) = (unsafe { LoadImageW(Some(exe.into()), resource, IMAGE_ICON, size, size, LR_DEFAULTCOLOR) }) else {
            continue;
        };
        unsafe { SendMessageW(hwnd, WM_SETICON, Some(WPARAM(slot as usize)), Some(LPARAM(icon.0 as isize))) };
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
