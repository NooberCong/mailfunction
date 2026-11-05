//! Windows notifications for new mail. An unpackaged app has no identity of its own, so it
//! registers one in the current user's hive. Registering a COM activator with it means a click
//! reaches the app from the banner, from the notification centre, and after the app has closed,
//! when Windows starts it again.

use std::ffi::c_void;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{Path, PathBuf};

use windows::Data::Xml::Dom::XmlDocument;
use windows::UI::Notifications::{ToastNotification, ToastNotificationManager, ToastNotifier};
use windows::Win32::Foundation::CLASS_E_NOAGGREGATION;
use windows::Win32::System::Com::{
    CLSCTX_LOCAL_SERVER, COINIT_APARTMENTTHREADED, CoInitializeEx, CoRegisterClassObject, IClassFactory, IClassFactory_Impl,
    REGCLS_MULTIPLEUSE,
};
use windows::Win32::System::Registry::{HKEY_CURRENT_USER, REG_SZ, RegSetKeyValueW};
use windows::Win32::UI::Notifications::{
    INotificationActivationCallback, INotificationActivationCallback_Impl, NOTIFICATION_USER_INPUT_DATA,
};
use windows::core::{BOOL, GUID, HSTRING, IUnknown, Interface, PCWSTR, Ref, implement};

use crate::on_ui_thread;

pub const APP_ID: &str = "mailfunction";
/// Identifies the app's activator to COM. Keep it: delivered notifications point to it.
pub const ACTIVATOR: GUID = GUID::from_u128(0x6f1c_2a54_8d3e_4b7a_9c21_5e0d_7a3b_1f46);
const ICON: &[u8] = include_bytes!("../ui/icon.png");

/// What a notification opens: an account's Inbox, at a message if it is about just one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Opens {
    pub account: i64,
    pub message: Option<i64>,
}

impl Opens {
    fn to_launch(self) -> String {
        match self.message {
            Some(message) => format!("{} {message}", self.account),
            None => self.account.to_string(),
        }
    }

    fn from_launch(launch: &str) -> Option<Self> {
        let mut parts = launch.split(' ');
        let account = parts.next()?.parse().ok()?;
        let message = parts.next().map(str::parse).transpose().ok()?;
        Some(Self { account, message })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notice {
    pub title: String,
    pub text: String,
    /// Which account this is for, when there are several to tell apart.
    pub account: Option<String>,
    /// A PNG of the sender.
    pub picture: Option<Vec<u8>>,
    pub opens: Opens,
}

/// Where notices go. The real one is Windows; tests record what they are given.
pub trait Notifier {
    fn show(&self, notice: &Notice);
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;").replace('\'', "&apos;")
}

fn toast_xml(notice: &Notice, picture: Option<&Path>) -> String {
    let image = picture.map_or_else(String::new, |path| {
        format!(r#"<image placement="appLogoOverride" src="{}"/>"#, escape(&path.to_string_lossy()))
    });
    let account = notice.account.as_deref().map_or_else(String::new, |account| {
        format!(r#"<text placement="attribution">{}</text>"#, escape(account))
    });
    format!(
        r#"<toast launch="{}"><visual><binding template="ToastGeneric"><text>{}</text><text>{}</text>{image}{account}</binding></visual><audio src="ms-winsoundevent:Notification.Mail"/></toast>"#,
        notice.opens.to_launch(),
        escape(&notice.title),
        escape(&notice.text),
    )
}

#[implement(INotificationActivationCallback)]
struct Clicked;

impl INotificationActivationCallback_Impl for Clicked_Impl {
    fn Activate(
        &self,
        _: &PCWSTR,
        launch: &PCWSTR,
        _: *const NOTIFICATION_USER_INPUT_DATA,
        _: u32,
    ) -> windows::core::Result<()> {
        let launch = unsafe { launch.to_string() }.unwrap_or_default();
        if let Some(opens) = Opens::from_launch(&launch) {
            on_ui_thread(move |this| this.open(opens));
        }
        Ok(())
    }
}

#[implement(IClassFactory)]
struct Activator;

impl IClassFactory_Impl for Activator_Impl {
    fn CreateInstance(&self, outer: Ref<IUnknown>, iid: *const GUID, object: *mut *mut c_void) -> windows::core::Result<()> {
        if outer.is_some() {
            return Err(CLASS_E_NOAGGREGATION.into());
        }
        let clicked: INotificationActivationCallback = Clicked.into();
        unsafe { clicked.query(iid, object) }.ok()
    }

    fn LockServer(&self, _: BOOL) -> windows::core::Result<()> {
        Ok(())
    }
}

fn set_registry(key: &str, name: Option<&str>, value: &str) -> Option<()> {
    let value: Vec<u16> = value.encode_utf16().chain([0]).collect();
    let name = name.map(HSTRING::from).unwrap_or_default();
    let size = (value.len() * size_of::<u16>()) as u32;
    let data = Some(value.as_ptr().cast());
    unsafe { RegSetKeyValueW(HKEY_CURRENT_USER, &HSTRING::from(key), &name, REG_SZ.0, data, size) }.ok().ok()
}

/// Tells Windows who the app is and how to reach it. The exe can move, so this runs at each start.
fn register(pictures: &Path) -> Option<ToastNotifier> {
    let icon = pictures.join("icon.png");
    std::fs::create_dir_all(pictures).ok()?;
    std::fs::write(&icon, ICON).ok()?;
    let exe = std::env::current_exe().ok()?;
    let activator = format!("{ACTIVATOR:?}").to_uppercase();
    let identity = format!(r"Software\Classes\AppUserModelId\{APP_ID}");
    set_registry(&identity, Some("DisplayName"), APP_ID)?;
    set_registry(&identity, Some("IconUri"), &icon.to_string_lossy())?;
    set_registry(&identity, Some("CustomActivator"), &format!("{{{activator}}}"))?;
    let server = format!(r"Software\Classes\CLSID\{{{activator}}}\LocalServer32");
    set_registry(&server, None, &format!(r#""{}" -ToastActivated"#, exe.display()))?;

    unsafe {
        // Already done on this thread by the windowing library, which is fine.
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let factory: IClassFactory = Activator.into();
        // Held until the process ends, as clicks can come at any time.
        CoRegisterClassObject(&ACTIVATOR, &factory, CLSCTX_LOCAL_SERVER, REGCLS_MULTIPLEUSE).ok()?;
    }
    ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from(APP_ID)).ok()
}

/// Notifications from Windows. If registering failed, the app simply goes without them.
pub struct Toasts {
    notifier: Option<ToastNotifier>,
    pictures: PathBuf,
}

impl Toasts {
    pub fn new(data: &Path) -> Self {
        let pictures = data.join("notifications");
        Self { notifier: register(&pictures), pictures }
    }

    /// Windows reads pictures from disk, and again whenever the notification centre opens, so
    /// each stays in a file named after its content. There is one per sender, and they are small.
    fn save(&self, png: &[u8]) -> Option<PathBuf> {
        let mut hasher = DefaultHasher::new();
        png.hash(&mut hasher);
        let path = self.pictures.join(format!("{:016x}.png", hasher.finish()));
        (path.exists() || std::fs::write(&path, png).is_ok()).then_some(path)
    }
}

impl Notifier for Toasts {
    fn show(&self, notice: &Notice) {
        let Some(notifier) = &self.notifier else { return };
        let picture = notice.picture.as_deref().and_then(|png| self.save(png));
        let xml = HSTRING::from(toast_xml(notice, picture.as_deref()));
        let document = XmlDocument::new().and_then(|document| document.LoadXml(&xml).map(|()| document));
        let toast = document.and_then(|document| ToastNotification::CreateToastNotification(&document));
        if let Ok(toast) = toast {
            let _ = notifier.Show(&toast);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn notice() -> Notice {
        Notice {
            title: "An & Co".into(),
            text: "<b>Lunch?</b> \"today\"".into(),
            account: None,
            picture: None,
            opens: Opens { account: 2, message: Some(9) },
        }
    }

    #[test]
    fn a_click_finds_its_way_back() {
        for opens in [Opens { account: 2, message: Some(9) }, Opens { account: 2, message: None }] {
            assert_eq!(Opens::from_launch(&opens.to_launch()), Some(opens));
        }
        assert_eq!(Opens::from_launch(""), None);
        assert_eq!(Opens::from_launch("2 x"), None);
    }

    #[test]
    fn what_a_sender_writes_stays_text() {
        let xml = toast_xml(&notice(), None);
        assert!(xml.contains("<text>An &amp; Co</text>"), "{xml}");
        assert!(xml.contains("&lt;b&gt;Lunch?&lt;/b&gt; &quot;today&quot;"), "{xml}");
        let document = XmlDocument::new().unwrap();
        document.LoadXml(&HSTRING::from(xml)).expect("well-formed");
    }

    #[test]
    fn the_picture_and_account_appear_only_when_there_are_some() {
        let bare = toast_xml(&notice(), None);
        assert!(!bare.contains("<image") && !bare.contains("attribution"), "{bare}");
        let full = Notice { account: Some("me@example.com".into()), ..notice() };
        let xml = toast_xml(&full, Some(Path::new(r"C:\Users\Rog Scar\a.png")));
        assert!(xml.contains(r#"placement="appLogoOverride" src="C:\Users\Rog Scar\a.png""#), "{xml}");
        assert!(xml.contains(r#"<text placement="attribution">me@example.com</text>"#), "{xml}");
    }
}
