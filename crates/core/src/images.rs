//! The remote images in mail: from disk if they have been seen before, otherwise from the web,
//! a few at a time, so that a page fills in while it is being read.

use std::collections::VecDeque;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use sha2::{Digest, Sha256};

use crate::remote::Network;
use crate::render::image_type;

/// Each page gets its own, so that a slow server in the message just left holds nothing up.
pub(crate) const FETCHES: usize = 8;
const MAX_CACHE: u64 = 512 * 1024 * 1024;
/// How much can be added before the cache is measured, which means reading the whole folder.
const TRIM_EVERY: u64 = MAX_CACHE / 16;

/// What an image's answer goes to: its bytes, or nothing if it could not be had.
type Answer = Box<dyn FnOnce(Option<Vec<u8>>) + Send>;

pub struct Images {
    dir: PathBuf,
    network: Arc<dyn Network>,
    added: AtomicU64,
}

impl Images {
    pub fn new(dir: PathBuf, network: Arc<dyn Network>) -> Arc<Self> {
        // Starts full, so that the first image added also measures what earlier runs left.
        Arc::new(Self { dir, network, added: AtomicU64::new(TRIM_EVERY) })
    }

    /// Starts loading for one page. Dropping what this returns gives up on what has not begun.
    pub fn page(self: &Arc<Self>) -> Page {
        Page(Arc::new(Line { images: self.clone(), waiting: Mutex::default(), left: AtomicBool::new(false) }))
    }

    /// Forgets every image, as they are a record of the mail that was read.
    pub fn clear(&self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }

    fn file(&self, url: &str) -> PathBuf {
        let name = Sha256::digest(url).iter().fold(String::new(), |mut name, byte| {
            let _ = write!(name, "{byte:02x}");
            name
        });
        self.dir.join(name)
    }

    /// Mail images keep their URL for what they show, so one that is held is never asked for
    /// again. That also spares telling a sender each time the message is opened.
    fn fetch(&self, url: &str) -> Option<Vec<u8>> {
        let file = self.file(url);
        if let Some(held) = std::fs::read(&file).ok().filter(|bytes| image_type(bytes).is_some()) {
            return Some(held);
        }
        let bytes = self.network.image(url)?;
        // A cache that cannot be written only costs the next open a fetch.
        let _ = self.keep(&file, &bytes);
        Some(bytes)
    }

    /// Whole or not at all, because a crash must not leave half an image to be shown for good.
    fn keep(&self, file: &Path, bytes: &[u8]) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.dir)?;
        let part = file.with_extension("part");
        std::fs::write(&part, bytes)?;
        std::fs::rename(&part, file)?;
        if self.added.fetch_add(bytes.len() as u64, Ordering::Relaxed) >= TRIM_EVERY {
            self.added.store(0, Ordering::Relaxed);
            self.trim(MAX_CACHE)?;
        }
        Ok(())
    }

    /// Removes the oldest files until the folder fits in `limit` bytes.
    fn trim(&self, limit: u64) -> std::io::Result<()> {
        let mut files = Vec::new();
        for entry in std::fs::read_dir(&self.dir)?.flatten() {
            if let Ok(meta) = entry.metadata() {
                files.push((meta.modified()?, meta.len(), entry.path()));
            }
        }
        let mut total: u64 = files.iter().map(|(_, size, _)| size).sum();
        files.sort_unstable();
        for (_, size, path) in files {
            if total <= limit {
                break;
            }
            if std::fs::remove_file(path).is_ok() {
                total -= size;
            }
        }
        Ok(())
    }
}

/// The images of the page on screen.
pub struct Page(Arc<Line>);

impl Page {
    /// Answers with an image, on another thread. Not at all if the page is left first.
    pub fn get(&self, url: String, answer: impl FnOnce(Option<Vec<u8>>) + Send + 'static) {
        let mut waiting = self.0.waiting.lock().expect("images lock");
        waiting.jobs.push_back((url, Box::new(answer)));
        if waiting.workers < FETCHES {
            waiting.workers += 1;
            let line = self.0.clone();
            std::thread::spawn(move || line.work());
        }
    }
}

impl Drop for Page {
    fn drop(&mut self) {
        self.0.left.store(true, Ordering::Relaxed);
    }
}

#[derive(Default)]
struct Waiting {
    jobs: VecDeque<(String, Answer)>,
    workers: usize,
}

struct Line {
    images: Arc<Images>,
    waiting: Mutex<Waiting>,
    left: AtomicBool,
}

impl Line {
    fn work(&self) {
        while let Some((url, answer)) = self.next() {
            answer(self.images.fetch(&url));
        }
    }

    fn next(&self) -> Option<(String, Answer)> {
        let mut waiting = self.waiting.lock().expect("images lock");
        let job = if self.left.load(Ordering::Relaxed) { None } else { waiting.jobs.pop_front() };
        if job.is_none() {
            waiting.workers -= 1;
        }
        job
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc::{Receiver, channel};
    use std::time::Duration;

    use super::*;
    use crate::remote::Logo;

    const PNG: &[u8] = b"\x89PNG\r\n\x1a\nrest";
    const PATIENCE: Duration = Duration::from_secs(5);

    /// Counts what reaches the web, and with a gate answers only once the test lets it.
    #[derive(Default)]
    struct Web {
        asked: Mutex<Vec<String>>,
        gate: Mutex<Option<Receiver<()>>>,
    }

    impl Web {
        fn asked(&self) -> usize {
            self.asked.lock().unwrap().len()
        }

        fn wait_for(&self, count: usize) {
            for _ in 0..1000 {
                if self.asked() >= count {
                    return;
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            panic!("{} of {count} reached the web", self.asked());
        }
    }

    impl Network for Web {
        fn image(&self, url: &str) -> Option<Vec<u8>> {
            self.asked.lock().unwrap().push(url.to_string());
            // Holding the lock, so the others wait their turn at the gate.
            if let Some(gate) = self.gate.lock().unwrap().as_ref() {
                let _ = gate.recv_timeout(PATIENCE);
            }
            url.ends_with(".png").then(|| PNG.to_vec())
        }

        fn logo(&self, _: &str) -> Logo {
            Logo::Unknown
        }
    }

    fn images(name: &str) -> (Arc<Images>, Arc<Web>) {
        let dir = std::env::temp_dir().join(format!("mailfunction-images-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let web = Arc::new(Web::default());
        (Images::new(dir, web.clone()), web)
    }

    fn get(page: &Page, url: &str) -> Option<Vec<u8>> {
        let (sender, answer) = channel();
        page.get(url.to_string(), move |bytes| sender.send(bytes).unwrap());
        answer.recv_timeout(PATIENCE).unwrap()
    }

    #[test]
    fn an_image_is_fetched_once_even_across_runs() {
        let (images, web) = images("once");
        assert_eq!(get(&images.page(), "https://a.example/1.png").as_deref(), Some(PNG));
        assert_eq!(get(&images.page(), "https://a.example/1.png").as_deref(), Some(PNG));
        assert_eq!(web.asked(), 1);

        let again = Images::new(images.dir.clone(), web.clone());
        assert_eq!(get(&again.page(), "https://a.example/1.png").as_deref(), Some(PNG));
        assert_eq!(web.asked(), 1);
    }

    #[test]
    fn a_failure_is_not_kept_and_a_damaged_file_is_fetched_again() {
        let (images, web) = images("failure");
        assert_eq!(get(&images.page(), "https://a.example/gone"), None);
        assert_eq!(get(&images.page(), "https://a.example/gone"), None);
        assert_eq!(web.asked(), 2);

        let url = "https://a.example/1.png";
        get(&images.page(), url);
        std::fs::write(images.file(url), b"<svg onload=alert(1)>").unwrap();
        assert_eq!(get(&images.page(), url).as_deref(), Some(PNG));
        assert_eq!(web.asked(), 4);
    }

    #[test]
    fn the_cache_loses_its_oldest_files_and_clearing_loses_them_all() {
        let (images, _) = images("trim");
        for n in 0..3 {
            get(&images.page(), &format!("https://a.example/{n}.png"));
            std::thread::sleep(Duration::from_millis(20));
        }
        images.trim(2 * PNG.len() as u64).unwrap();
        assert!(!images.file("https://a.example/0.png").exists());
        assert!(images.file("https://a.example/2.png").exists());
        images.clear();
        assert!(!images.file("https://a.example/2.png").exists());
    }

    #[test]
    fn a_page_left_behind_drops_what_has_not_started() {
        let (images, web) = images("left");
        let (open, gate) = channel();
        *web.gate.lock().unwrap() = Some(gate);
        let page = images.page();
        for n in 0..FETCHES * 3 {
            page.get(format!("https://a.example/{n}.png"), |_| {});
        }
        web.wait_for(FETCHES);
        drop(page);
        for _ in 0..FETCHES {
            open.send(()).unwrap();
        }
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(web.asked(), FETCHES, "a page nobody is reading went on fetching");
    }

    #[test]
    fn images_load_side_by_side_and_no_more_than_that() {
        let (images, web) = images("parallel");
        let (open, gate) = channel();
        *web.gate.lock().unwrap() = Some(gate);
        let page = images.page();
        let (sender, answers) = channel();
        for n in 0..FETCHES + 2 {
            let sender = sender.clone();
            page.get(format!("https://a.example/{n}.png"), move |bytes| sender.send(bytes).unwrap());
        }
        web.wait_for(FETCHES);
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(web.asked(), FETCHES);
        for _ in 0..FETCHES + 2 {
            let _ = open.send(());
            answers.recv_timeout(PATIENCE).unwrap();
        }
    }
}
