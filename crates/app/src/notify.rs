//! Windows notifications for new mail. An unpackaged app has no identity of its own, so it
//! registers one in the current user's hive. Registering a COM activator with it means a click
//! reaches the app from the banner, from the notification centre, and after the app has closed,
//! when Windows starts it again.

use std::cell::RefCell;
use std::collections::HashMap;
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
use windows::Win32::System::Registry::{HKEY_CURRENT_USER, REG_SZ, RegDeleteTreeW, RegSetKeyValueW};
use windows::Win32::UI::Notifications::{
    INotificationActivationCallback, INotificationActivationCallback_Impl, NOTIFICATION_USER_INPUT_DATA,
};
use windows::core::{BOOL, GUID, HSTRING, IUnknown, Interface, PCWSTR, Ref, implement};

use crate::on_ui_thread;

pub const APP_ID: &str = "mailfunction";
/// Identifies the app's activator to COM. Keep it: delivered notifications point to it.
pub const ACTIVATOR: GUID = GUID::from_u128(0x6f1c_2a54_8d3e_4b7a_9c21_5e0d_7a3b_1f46);
const ICON: &[u8] = include_bytes!("../ui/icon.png");

/// Who an account's notifications come from. Windows heads a notification with the name of the
/// identity that sent it and offers no other way to set that line, so each account has its own.
pub fn identity(account: i64) -> String {
    format!("{APP_ID}.{account}")
}

fn identity_key(identity: &str) -> String {
    format!(r"Software\Classes\AppUserModelId\{identity}")
}

fn activator_id() -> String {
    format!("{{{}}}", format!("{ACTIVATOR:?}").to_uppercase())
}

/// Tells Windows an identity's name, and that clicks on its notifications come to the activator.
fn declare(identity: &str, name: &str, icon: &Path) -> Option<()> {
    let key = identity_key(identity);
    set_registry(&key, Some("DisplayName"), name)?;
    set_registry(&key, Some("IconUri"), &icon.to_string_lossy())?;
    set_registry(&key, Some("CustomActivator"), &activator_id())
}

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
    /// The address it came to, which heads the notification.
    pub account: String,
    /// A PNG of the sender.
    pub picture: Option<Vec<u8>>,
    /// It appears without a sound.
    pub silent: bool,
    pub opens: Opens,
}

/// Where notices go. The real one is Windows; tests record what they are given.
pub trait Notifier {
    fn show(&self, notice: &Notice);
    /// The account is gone, and nothing of it should stay behind.
    fn forget(&self, _account: i64) {}
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;").replace('\'', "&apos;")
}

fn toast_xml(notice: &Notice, picture: Option<&Path>) -> String {
    let image = picture.map_or_else(String::new, |path| {
        format!(r#"<image placement="appLogoOverride" src="{}"/>"#, escape(&path.to_string_lossy()))
    });
    let audio = if notice.silent { r#"silent="true""# } else { r#"src="ms-winsoundevent:Notification.Mail""# };
    format!(
        r#"<toast launch="{}"><visual><binding template="ToastGeneric"><text>{}</text><text>{}</text>{image}</binding></visual><audio {audio}/></toast>"#,
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

pub(crate) fn set_registry(key: &str, name: Option<&str>, value: &str) -> Option<()> {
    let value: Vec<u16> = value.encode_utf16().chain([0]).collect();
    let name = name.map(HSTRING::from).unwrap_or_default();
    let size = (value.len() * size_of::<u16>()) as u32;
    let data = Some(value.as_ptr().cast());
    unsafe { RegSetKeyValueW(HKEY_CURRENT_USER, &HSTRING::from(key), &name, REG_SZ.0, data, size) }.ok().ok()
}

/// Tells Windows how to reach the app. The exe can move, so this runs at each start. The app's own
/// identity stays for the notifications it already delivered, which still point to it.
fn register(pictures: &Path) -> Option<PathBuf> {
    let icon = pictures.join("icon.png");
    std::fs::create_dir_all(pictures).ok()?;
    std::fs::write(&icon, ICON).ok()?;
    let exe = std::env::current_exe().ok()?;
    declare(APP_ID, APP_ID, &icon)?;
    let server = format!(r"Software\Classes\CLSID\{}\LocalServer32", activator_id());
    set_registry(&server, None, &format!(r#""{}" -ToastActivated"#, exe.display()))?;

    unsafe {
        // Already done on this thread by the windowing library, which is fine.
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let factory: IClassFactory = Activator.into();
        // Held until the process ends, as clicks can come at any time.
        CoRegisterClassObject(&ACTIVATOR, &factory, CLSCTX_LOCAL_SERVER, REGCLS_MULTIPLEUSE).ok()?;
    }
    Some(icon)
}

/// Notifications from Windows. If registering failed, the app simply goes without them.
pub struct Toasts {
    /// The app's icon on disk, if registering went well.
    icon: Option<PathBuf>,
    /// By account, as each one comes first needed.
    notifiers: RefCell<HashMap<i64, ToastNotifier>>,
    pictures: PathBuf,
}

impl Toasts {
    pub fn new(data: &Path) -> Self {
        let pictures = data.join("notifications");
        Self { icon: register(&pictures), notifiers: RefCell::default(), pictures }
    }

    fn notifier(&self, notice: &Notice) -> Option<ToastNotifier> {
        let account = notice.opens.account;
        if let Some(notifier) = self.notifiers.borrow().get(&account) {
            return Some(notifier.clone());
        }
        let identity = identity(account);
        declare(&identity, &notice.account, self.icon.as_ref()?)?;
        let notifier = ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from(identity)).ok()?;
        self.notifiers.borrow_mut().insert(account, notifier.clone());
        Some(notifier)
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
        let Some(notifier) = self.notifier(notice) else { return };
        let picture = notice.picture.as_deref().and_then(|png| self.save(png));
        let xml = HSTRING::from(toast_xml(notice, picture.as_deref()));
        let document = XmlDocument::new().and_then(|document| document.LoadXml(&xml).map(|()| document));
        let toast = document.and_then(|document| ToastNotification::CreateToastNotification(&document));
        if let Ok(toast) = toast {
            let _ = notifier.Show(&toast);
        }
    }

    fn forget(&self, account: i64) {
        self.notifiers.borrow_mut().remove(&account);
        let identity = identity(account);
        if let Ok(history) = ToastNotificationManager::History() {
            let _ = history.ClearWithId(&HSTRING::from(&identity));
        }
        let _ = unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, &HSTRING::from(identity_key(&identity))) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn notice() -> Notice {
        Notice {
            title: "An & Co".into(),
            text: "<b>Lunch?</b> \"today\"".into(),
            account: "me@example.com".into(),
            picture: None,
            silent: false,
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
    fn a_silent_notice_has_no_sound_to_play() {
        assert!(toast_xml(&notice(), None).contains("Notification.Mail"));
        let xml = toast_xml(&Notice { silent: true, ..notice() }, None);
        assert!(xml.contains(r#"<audio silent="true"/>"#) && !xml.contains("Notification.Mail"), "{xml}");
    }

    #[test]
    fn the_picture_appears_only_when_there_is_one() {
        assert!(!toast_xml(&notice(), None).contains("<image"));
        let xml = toast_xml(&notice(), Some(Path::new(r"C:\Users\Rog Scar\a.png")));
        assert!(xml.contains(r#"placement="appLogoOverride" src="C:\Users\Rog Scar\a.png""#), "{xml}");
    }
}
