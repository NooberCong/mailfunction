//! A local server that records every request, so "no remote content" is a counted fact.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Clone)]
pub struct Canary {
    pub port: u16,
    hits: Arc<Mutex<Vec<String>>>,
}

impl Canary {
    pub fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("addr").port();
        let hits = Arc::new(Mutex::new(Vec::new()));
        let log = hits.clone();
        std::thread::spawn(move || {
            for mut stream in listener.incoming().flatten() {
                let log = log.clone();
                std::thread::spawn(move || {
                    let _ = stream.set_read_timeout(Some(Duration::from_millis(500)));
                    let mut buf = [0u8; 2048];
                    let n = stream.read(&mut buf).unwrap_or(0);
                    let head = String::from_utf8_lossy(&buf[..n]);
                    if let Some(path) = head.split_whitespace().nth(1) {
                        log.lock().unwrap().push(path.to_string());
                    }
                    let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
                });
            }
        });
        Self { port, hits }
    }

    pub fn take(&self) -> Vec<String> {
        std::mem::take(&mut *self.hits.lock().unwrap())
    }
}

pub fn hostile_page(port: u16) -> String {
    let u = format!("http://127.0.0.1:{port}");
    format!(
        r#"<!doctype html><html><head><title>original</title>
<meta http-equiv="refresh" content="1;url={u}/meta-refresh">
<link rel="stylesheet" href="{u}/link-css">
<link rel="prefetch" href="{u}/prefetch">
<style>
@import url("{u}/css-import");
@font-face {{ font-family: x; src: url("{u}/font.woff"); }}
body {{ margin: 0; font-family: x, sans-serif; background: #fff url("{u}/css-bg.png"); }}
</style></head><body>
<a href="{u}/clicked" style="display:block;width:220px;height:100px;background:#00aa00;color:#fff">link</a>
<img src="{u}/img.png" width="1" height="1">
<svg width="1" height="1"><image href="{u}/svg-image.png" width="1" height="1"/></svg>
<iframe src="{u}/iframe" width="1" height="1"></iframe>
<video poster="{u}/video-poster.png" width="1" height="1"></video>
<script src="{u}/script.js"></script>
<script>document.title = 'script-ran'; fetch('{u}/fetch');</script>
<div style="height:3000px">Message body</div>
</body></html>"#
    )
}
