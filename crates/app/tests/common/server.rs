//! What tests against the real servers share: waiting from inside the event loop, and reaching
//! Dovecot and mailpit from the outside.

use std::cell::Cell;
use std::future::Future;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::Storage::Xps::{PRINT_WINDOW_FLAGS, PrintWindow};
use windows::Win32::UI::WindowsAndMessaging::GetClientRect;

/// Another session may be using the usual port, so it can be moved.
pub fn dovecot_port() -> u16 {
    std::env::var("MAILFUNCTION_IMAP_PORT").map_or(14143, |port| port.parse().unwrap())
}

pub fn sleep(ms: u64) -> impl Future<Output = ()> {
    let (tx, rx) = mpsc::channel::<()>();
    let done = Rc::new(Cell::new(false));
    let flag = done.clone();
    slint::Timer::single_shot(Duration::from_millis(ms), move || {
        flag.set(true);
        let _ = tx.send(());
    });
    Wait { done, rx }
}

/// Polls from the Slint executor, which wakes on timers.
struct Wait {
    done: Rc<Cell<bool>>,
    rx: mpsc::Receiver<()>,
}

impl Future for Wait {
    type Output = ();
    fn poll(self: std::pin::Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> std::task::Poll<()> {
        if self.done.get() {
            let _ = self.rx.try_recv();
            return std::task::Poll::Ready(());
        }
        let waker = cx.waker().clone();
        slint::Timer::single_shot(Duration::from_millis(10), move || waker.wake());
        std::task::Poll::Pending
    }
}

pub async fn until(what: &str, mut ready: impl FnMut() -> bool) {
    let started = Instant::now();
    while !ready() {
        assert!(started.elapsed() < Duration::from_secs(30), "timed out waiting for {what}");
        sleep(25).await;
    }
    println!("  {what}: {} ms", started.elapsed().as_millis());
}

/// Delivers mail the way another client would, from its own thread and runtime.
pub fn deliver(user: String, messages: Vec<Vec<u8>>) {
    std::thread::spawn(move || {
        tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
            let tcp = tokio::net::TcpStream::connect(("127.0.0.1", dovecot_port()))
                .await
                .expect("start Dovecot: scripts/test-imap.ps1");
            let mut client = async_imap::Client::new(tcp);
            client.read_response().await.unwrap();
            let mut session = client.login(&user, "pass").await.map_err(|(e, _)| e).unwrap();
            for message in messages {
                session.append("INBOX", None, None, &message).await.unwrap();
            }
        });
    })
    .join()
    .unwrap();
}

/// How many messages a folder holds on the server, and how many of them are read.
pub fn on_server(user: &str, folder: &str) -> (u32, usize) {
    let (user, folder) = (user.to_string(), folder.to_string());
    std::thread::spawn(move || {
        tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
            let tcp = tokio::net::TcpStream::connect(("127.0.0.1", dovecot_port())).await.unwrap();
            let mut client = async_imap::Client::new(tcp);
            client.read_response().await.unwrap();
            let mut session = client.login(&user, "pass").await.map_err(|(e, _)| e).unwrap();
            let exists = session.select(&folder).await.unwrap().exists;
            (exists, session.search("SEEN").await.unwrap().len())
        })
    })
    .join()
    .unwrap()
}

pub fn create_folders(user: String) {
    std::thread::spawn(move || {
        tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
            let tcp = tokio::net::TcpStream::connect(("127.0.0.1", dovecot_port())).await.unwrap();
            let mut client = async_imap::Client::new(tcp);
            client.read_response().await.unwrap();
            let mut session = client.login(&user, "pass").await.map_err(|(e, _)| e).unwrap();
            for folder in ["Trash", "Sent", "Drafts"] {
                session.create(folder).await.unwrap();
            }
        });
    })
    .join()
    .unwrap();
}

pub fn delivered_to_mailpit(subject: &str) -> usize {
    let url = format!("http://127.0.0.1:18025/api/v1/search?query=subject:{subject}");
    let mut response = ureq::get(&url).call().expect("start mailpit: scripts/test-imap.ps1");
    let body = response.body_mut().read_to_string().unwrap();
    body.split("\"ID\"").count() - 1
}

/// The message as mailpit received it, headers and all.
pub fn received_by_mailpit(subject: &str) -> String {
    let url = format!("http://127.0.0.1:18025/api/v1/search?query=subject:{subject}");
    let found = ureq::get(&url).call().unwrap().body_mut().read_to_string().unwrap();
    let id = found.split("\"ID\":\"").nth(1).unwrap().split('"').next().unwrap().to_string();
    let url = format!("http://127.0.0.1:18025/api/v1/message/{id}/raw");
    ureq::get(&url).call().unwrap().body_mut().read_to_string().unwrap()
}

/// Counts every connection made to it. Any hit at all means mail reached the network.
pub fn canary() -> (u16, Arc<AtomicU32>) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    let hits = Arc::new(AtomicU32::new(0));
    let count = hits.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            count.fetch_add(1, Ordering::SeqCst);
            let mut stream = stream;
            let mut line = String::new();
            let _ = stream.set_read_timeout(Some(Duration::from_millis(500)));
            let _ = BufReader::new(&stream).read_line(&mut line);
            println!("  canary hit: {:?}", line.trim());
            let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
        }
    });
    (port, hits)
}

/// The window's whole client area, webview included, as blue, green, red and a spare byte.
pub fn client_pixels(hwnd: isize) -> Option<(i32, i32, Vec<u8>)> {
    let hwnd = HWND(hwnd as _);
    unsafe {
        let mut client = RECT::default();
        GetClientRect(hwnd, &mut client).ok()?;
        let (w, h) = (client.right, client.bottom);
        let dc = GetDC(Some(hwnd));
        let memory = CreateCompatibleDC(Some(dc));
        let bitmap = CreateCompatibleBitmap(dc, w, h);
        let previous = SelectObject(memory, bitmap.into());
        // PW_CLIENTONLY | PW_RENDERFULLCONTENT: the second is what includes the webview.
        let _ = PrintWindow(hwnd, memory, PRINT_WINDOW_FLAGS(3));
        let mut info = BITMAPINFO::default();
        info.bmiHeader.biSize = size_of::<BITMAPINFOHEADER>() as u32;
        info.bmiHeader.biWidth = w;
        info.bmiHeader.biHeight = -h;
        info.bmiHeader.biPlanes = 1;
        info.bmiHeader.biBitCount = 32;
        let mut pixels = vec![0u8; (w * h * 4) as usize];
        GetDIBits(memory, bitmap, 0, h as u32, Some(pixels.as_mut_ptr().cast()), &mut info, DIB_RGB_COLORS);
        SelectObject(memory, previous);
        let _ = DeleteObject(bitmap.into());
        let _ = DeleteDC(memory);
        ReleaseDC(Some(hwnd), dc);
        Some((w, h, pixels))
    }
}
