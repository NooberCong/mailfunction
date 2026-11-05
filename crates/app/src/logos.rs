//! Logos for avatars. The list asks for one each time it draws a row, so the answer always comes
//! at once from memory or the store, and anything missing is looked up in the background. Until
//! then the row shows initials, and if the lookup finds nothing it keeps them.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::rc::Rc;
use std::sync::{Arc, Condvar, Mutex, Weak};
use std::time::{SystemTime, UNIX_EPOCH};

use mailfunction_core::logo;
use mailfunction_core::remote::{Logo, Network};
use mailfunction_core::store::{Reader, Store};
use slint::{ComponentHandle, Image, Rgba8Pixel, SharedPixelBuffer};

use crate::{App, on_ui_thread};

const FETCHERS: usize = 4;
/// Brands change their logos, and a domain with none may gain one.
const RECHECK_SECONDS: i64 = 30 * 24 * 3600;

fn now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

/// Domains waiting for a lookup. The newest goes first, as that is what is on screen.
#[derive(Default)]
struct Queue {
    waiting: Mutex<Vec<String>>,
    ready: Condvar,
}

impl Queue {
    fn next(&self) -> String {
        let mut waiting = self.waiting.lock().expect("logo queue");
        loop {
            if let Some(domain) = waiting.pop() {
                return domain;
            }
            waiting = self.ready.wait(waiting).expect("logo queue");
        }
    }
}

fn look_up(queue: &Queue, network: &dyn Network, store: &Weak<Store>) {
    loop {
        let domain = queue.next();
        // A logo is a stranger's file, so one that panics a decoder costs that logo and no more.
        // This cannot catch a stack overflow, which `logo` has to prevent for itself.
        let found = catch_unwind(AssertUnwindSafe(|| network.logo(&domain))).unwrap_or(Logo::Missing);
        let Some(store) = store.upgrade() else { return };
        let saved = match found {
            Logo::Found(png) => store.save_logo(&domain, Some(&png), now()),
            Logo::Missing => store.save_logo(&domain, None, now()),
            Logo::Unknown => continue,
        };
        if saved.is_ok() {
            on_ui_thread(move |this| this.logo_ready(&domain));
        }
    }
}

pub struct Logos {
    reader: Rc<Reader>,
    store: Weak<Store>,
    network: Arc<dyn Network>,
    window: slint::Weak<App>,
    queue: Arc<Queue>,
    /// Off means nothing is shown and nobody is asked.
    enabled: Cell<bool>,
    /// Each domain is looked up at most once a run, however often its rows are drawn.
    asked: RefCell<HashSet<String>>,
    /// Finished images by domain and size in physical pixels. Empty for a domain with no logo.
    images: RefCell<HashMap<(String, u32), Image>>,
}

impl Logos {
    pub fn new(reader: Rc<Reader>, store: &Arc<Store>, network: Arc<dyn Network>, window: slint::Weak<App>) -> Self {
        Self {
            reader,
            store: Arc::downgrade(store),
            network,
            window,
            queue: Arc::default(),
            enabled: Cell::new(true),
            asked: RefCell::default(),
            images: RefCell::default(),
        }
    }

    pub fn set_enabled(&self, enabled: bool) {
        self.enabled.set(enabled);
    }

    /// A domain's logo for an avatar of `size` logical pixels, or an empty image if there is none.
    pub fn image(&self, domain: &str, size: f32) -> Image {
        if !self.enabled.get() {
            return Image::default();
        }
        self.look_up(domain);
        self.stored(domain, size)
    }

    /// A domain's logo as a round PNG of `size` pixels, if one is already held. This never asks,
    /// as it serves notifications, and mail arriving must not tell its sender anything.
    pub fn png(&self, domain: &str, size: u32) -> Option<Vec<u8>> {
        if !self.enabled.get() {
            return None;
        }
        let stored = self.reader.logo(domain).ok().flatten()?.0?;
        logo::png(logo::avatar(&stored, size)?.into_raw(), size)
    }

    /// What the store has for a domain, asking in the background if that may be out of date.
    fn look_up(&self, domain: &str) {
        let known = self.reader.logo(domain).ok().flatten();
        if known.is_none_or(|(_, checked)| now() - checked > RECHECK_SECONDS) {
            self.ask(domain);
        }
    }

    /// As [`Self::image`], for what the store already has, without looking anything up.
    pub fn stored(&self, name: &str, size: f32) -> Image {
        let scale = self.window.upgrade().map_or(1.0, |app| app.window().scale_factor());
        let pixels = (size * scale).round() as u32;
        let key = (name.to_string(), pixels);
        if let Some(image) = self.images.borrow().get(&key) {
            return image.clone();
        }
        let known = self.reader.logo(name).ok().flatten();
        let image = known.and_then(|(png, _)| logo::avatar(&png?, pixels)).map_or_else(Image::default, |avatar| {
            let (width, height) = avatar.dimensions();
            Image::from_rgba8(SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(avatar.as_raw(), width, height))
        });
        self.images.borrow_mut().insert(key, image.clone());
        image
    }

    /// Drops what is held for a domain, so that the store is read again.
    pub fn forget(&self, domain: &str) {
        self.images.borrow_mut().retain(|(held, _), _| held != domain);
    }

    fn ask(&self, domain: &str) {
        let mut asked = self.asked.borrow_mut();
        let first = asked.is_empty();
        if !asked.insert(domain.to_string()) {
            return;
        }
        if first {
            for _ in 0..FETCHERS {
                let (queue, network, store) = (self.queue.clone(), self.network.clone(), self.store.clone());
                std::thread::spawn(move || look_up(&queue, network.as_ref(), &store));
            }
        }
        self.queue.waiting.lock().expect("logo queue").push(domain.to_string());
        self.queue.ready.notify_one();
    }
}
