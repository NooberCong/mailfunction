//! The unread count on the taskbar button: a red disc with a number, which Windows lays over the
//! corner of the app's icon.

use std::cell::{Cell, RefCell};

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateBitmap, CreateDIBSection, DIB_RGB_COLORS, DeleteObject,
};
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx};
use windows::Win32::UI::Shell::{DefSubclassProc, ITaskbarList3, SetWindowSubclass, TaskbarList};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateIconIndirect, DestroyIcon, GetSystemMetrics, HICON, ICONINFO, RegisterWindowMessageW, SM_CXSMICON,
};
use windows::core::{BOOL, HSTRING, PCWSTR, w};

use crate::{disc, on_ui_thread};

const RED: [u8; 3] = [0xd8, 0x0d, 0x20];
const WHITE: [u8; 3] = [255; 3];
/// How large the number is, as a share of the disc.
const NUMBER_SCALE: f32 = 0.7;
/// Anything above this reads as "a lot", and more digits would not fit.
const MOST_SHOWN: i64 = 99;
const SUBCLASS_ID: usize = 1;

/// What the disc says, or nothing when there is nothing unread.
fn label(unread: i64) -> Option<String> {
    match unread {
        ..=0 => None,
        1..=MOST_SHOWN => Some(unread.to_string()),
        _ => Some(format!("{MOST_SHOWN}+")),
    }
}

fn draw(text: &str, size: u32) -> Vec<[u8; 4]> {
    disc::draw(text, size, RED, WHITE, NUMBER_SCALE)
}

/// Makes an icon from RGBA pixels. Windows copies what it is given, so the bitmaps go at once.
fn icon(pixels: &[[u8; 4]], size: u32) -> Option<HICON> {
    let info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: size as i32,
            biHeight: -(size as i32), // negative, so that rows run from the top
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut bits = std::ptr::null_mut();
    unsafe {
        let colour = CreateDIBSection(None, &raw const info, DIB_RGB_COLORS, &raw mut bits, None, 0).ok()?;
        let bgra = std::slice::from_raw_parts_mut(bits.cast::<[u8; 4]>(), pixels.len());
        for (to, [r, g, b, a]) in bgra.iter_mut().zip(pixels) {
            *to = [*b, *g, *r, *a];
        }
        // Ignored for an icon with alpha, but one has to exist.
        let mask = CreateBitmap(size as i32, size as i32, 1, 1, None);
        let details = ICONINFO { fIcon: BOOL::from(true), hbmMask: mask, hbmColor: colour, ..Default::default() };
        let icon = CreateIconIndirect(&raw const details).ok();
        let _ = DeleteObject(colour.into());
        let _ = DeleteObject(mask.into());
        icon
    }
}

fn taskbar_created_message() -> u32 {
    unsafe { RegisterWindowMessageW(w!("TaskbarButtonCreated")) }
}

/// Windows forgets an overlay when the taskbar restarts, and refuses one before the button
/// exists. It announces the button either way, so that is the moment to put the badge back.
unsafe extern "system" fn watch_taskbar(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _: usize,
    created: usize,
) -> LRESULT {
    if message as usize == created {
        on_ui_thread(|this| this.refresh_badge());
    }
    unsafe { DefSubclassProc(hwnd, message, wparam, lparam) }
}

/// Starts listening for the taskbar. The window must exist.
pub fn watch(hwnd: HWND) {
    let created = taskbar_created_message() as usize;
    let _ = unsafe { SetWindowSubclass(hwnd, Some(watch_taskbar), SUBCLASS_ID, created) };
}

#[derive(Default)]
pub struct Badge {
    taskbar: RefCell<Option<ITaskbarList3>>,
    /// What the button shows now. Sync reports often, and most reports change nothing.
    shown: Cell<Option<i64>>,
}

impl Badge {
    fn taskbar(&self) -> Option<ITaskbarList3> {
        if self.taskbar.borrow().is_none() {
            let list = unsafe {
                // Already done on this thread by the windowing library, which is fine.
                let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
                let list: ITaskbarList3 = CoCreateInstance(&TaskbarList, None, CLSCTX_INPROC_SERVER).ok()?;
                list.HrInit().ok()?;
                list
            };
            *self.taskbar.borrow_mut() = Some(list);
        }
        self.taskbar.borrow().clone()
    }

    /// Shows the count on the window's taskbar button.
    pub fn show(&self, hwnd: HWND, unread: i64) {
        if self.shown.get() == Some(unread) {
            return;
        }
        let Some(taskbar) = self.taskbar() else { return };
        let applied = match label(unread) {
            None => unsafe { taskbar.SetOverlayIcon(hwnd, HICON::default(), PCWSTR::null()) },
            Some(text) => {
                let size = unsafe { GetSystemMetrics(SM_CXSMICON) }.max(16) as u32;
                let Some(icon) = icon(&draw(&text, size), size) else { return };
                let description = HSTRING::from(format!("{unread} unread"));
                let applied = unsafe { taskbar.SetOverlayIcon(hwnd, icon, &description) };
                let _ = unsafe { DestroyIcon(icon) };
                applied
            }
        };
        self.shown.set(applied.is_ok().then_some(unread));
    }

    /// Forgets what the button shows, so that the next [`Self::show`] sets it whatever the count.
    pub fn forget(&self) {
        self.shown.set(None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_label_counts_up_to_a_limit_and_nothing_shows_nothing() {
        assert_eq!(label(0), None);
        assert_eq!(label(-1), None);
        assert_eq!(label(1).as_deref(), Some("1"));
        assert_eq!(label(99).as_deref(), Some("99"));
        assert_eq!(label(100).as_deref(), Some("99+"));
        assert_eq!(label(45_000).as_deref(), Some("99+"));
    }

    #[test]
    fn windows_accepts_it_as_an_icon() {
        let made = icon(&draw("47", 24), 24).expect("an icon");
        assert!(unsafe { DestroyIcon(made) }.is_ok());
    }

    #[test]
    fn previews_for_looking_at() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/ui");
        std::fs::create_dir_all(&dir).unwrap();
        for (name, text, size) in [("7", "7", 24), ("47", "47", 24), ("99plus", "99+", 24), ("47-small", "47", 16)] {
            let file = std::fs::File::create(dir.join(format!("badge-{name}.png"))).unwrap();
            let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), size, size);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            encoder.write_header().unwrap().write_image_data(draw(text, size).as_flattened()).unwrap();
        }
    }
}
