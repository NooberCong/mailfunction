//! Turns a raw message into something safe to show. This is the first of three layers against
//! hostile mail: an allowlist sanitiser here, then a CSP header, then a webview that has
//! scripts off and answers every network request with a refusal.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use ammonia::{Builder, UrlRelative};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use mail_parser::{MessageParser, MessageRef, PartKind};

/// Sent as a real response header by the webview layer, so mail cannot remove it.
pub const CONTENT_SECURITY_POLICY: &str =
    "default-src 'none'; img-src data:; style-src 'unsafe-inline'; base-uri 'none'; form-action 'none'";

/// Mail is designed for a white page and brings its own spacing, so a newsletter fills the pane
/// in either theme. Only mail with no layout of its own, such as a personal note, gets a margin.
const SHEET: &str = "<!doctype html><html><head><meta charset=\"utf-8\"><style>\
    html{background:#fff;color:#231417;color-scheme:light}\
    body{margin:0;padding:0;font:14px/1.6 'Segoe UI Variable Text','Segoe UI',sans-serif;overflow-wrap:anywhere;\
    animation:mf-in .17s cubic-bezier(.22,1,.36,1)}\
    body:not(:has(table)){padding:22px 36px}\
    @keyframes mf-in{from{opacity:0;transform:translateY(6px)}}\
    img{max-width:100%;height:auto}\
    ::-webkit-scrollbar{width:10px;height:10px}\
    ::-webkit-scrollbar-thumb{background:#e6dadc;border-radius:5px;border:2px solid #fff}\
    @media (prefers-reduced-motion:reduce){body{animation:none}}\
    </style></head><body>";

/// Inline images larger than this are dropped rather than embedded in the page.
pub(crate) const MAX_INLINE_IMAGE: usize = 8 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Body {
    Text(String),
    /// A complete, sanitised document.
    Html(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rendered {
    pub body: Body,
    pub recipients: String,
    pub attachments: Vec<String>,
    /// Images the body asks for that were not supplied, and so are not in it.
    pub remote_images: Vec<String>,
}

/// A message's body and details, or `None` if the bytes are not a message at all.
pub fn message(raw: &[u8]) -> Option<Rendered> {
    message_with_images(raw, &HashMap::new())
}

/// As [`message`], with the remote images that have been fetched, as `data:` URLs by URL.
pub fn message_with_images(raw: &[u8], fetched: &HashMap<String, String>) -> Option<Rendered> {
    let parsed = MessageParser::default().parse(raw)?;
    let message = parsed.root();
    let html = message.html_body().find(|p| matches!(p.kind(), PartKind::Html));
    let (body, remote_images) = match html.and_then(|_| message.body_html(0)) {
        Some(html) => {
            let (document, remote) = sanitise(&html, &inline_images(message), fetched);
            (Body::Html(document), remote)
        }
        None => (Body::Text(message.body_text(0).map(|t| t.into_owned()).unwrap_or_default()), Vec::new()),
    };
    let recipients = message
        .to()
        .map(|to| {
            to.mailboxes()
                .filter_map(|m| m.name().or(m.address()))
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_default();
    let attachments = message
        .attachments()
        .filter(|p| !(p.is_inline() && p.content_id().is_some()))
        .map(|p| p.attachment_name().unwrap_or("Attachment").to_string())
        .collect();
    Some(Rendered { body, recipients, attachments, remote_images })
}

/// `cid:` targets as `data:` URLs, restricted to image types a browser cannot run code from.
fn inline_images(message: MessageRef) -> HashMap<String, String> {
    let mut images = HashMap::new();
    for part in message.attachments() {
        let Some(id) = part.content_id() else { continue };
        let Some(kind) = part.content_type() else { continue };
        let subtype = kind.subtype().unwrap_or_default().to_ascii_lowercase();
        if !kind.ctype().eq_ignore_ascii_case("image") || !matches!(subtype.as_str(), "png" | "jpeg" | "jpg" | "gif" | "webp") {
            continue;
        }
        let bytes = part.decoded();
        if bytes.len() <= MAX_INLINE_IMAGE {
            images.insert(id.to_string(), format!("data:image/{subtype};base64,{}", STANDARD.encode(bytes.as_ref())));
        }
    }
    images
}

/// An image from the network as a `data:` URL, going by its bytes, because a server can claim
/// anything. Nothing that a browser could run code from gets through, SVG included.
pub(crate) fn data_url(bytes: &[u8]) -> Option<String> {
    let kind = if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        "png"
    } else if bytes.starts_with(b"\xff\xd8\xff") {
        "jpeg"
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        "gif"
    } else if bytes.len() > 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        "webp"
    } else {
        return None;
    };
    Some(format!("data:image/{kind};base64,{}", STANDARD.encode(bytes)))
}

fn is_safe_data_image(value: &str) -> bool {
    ["png", "jpeg", "jpg", "gif", "webp"]
        .iter()
        .any(|t| value.to_ascii_lowercase().starts_with(&format!("data:image/{t};base64,")))
}

/// Sanitises mail HTML into a complete document.
pub fn document(html: &str, inline_images: &HashMap<String, String>) -> String {
    sanitise(html, inline_images, &HashMap::new()).0
}

/// A complete document, and the remote images it had to leave out. Those in `fetched` go in as
/// `data:` URLs, so the page still loads nothing from anywhere.
fn sanitise(
    html: &str,
    inline_images: &HashMap<String, String>,
    fetched: &HashMap<String, String>,
) -> (String, Vec<String>) {
    let images = inline_images.clone();
    let fetched = fetched.clone();
    let blocked = Arc::new(Mutex::new(Vec::new()));
    let seen = blocked.clone();
    let cleaned = Builder::default()
        .add_tags(["center", "font", "style", "tbody", "thead", "tfoot"])
        .rm_clean_content_tags(["style"])
        // A removed tag leaves its text behind, and a title would land at the top of the body.
        .add_clean_content_tags(["title"])
        .add_generic_attributes(["align", "bgcolor", "class", "dir", "height", "id", "style", "valign", "width"])
        .add_tag_attributes("font", ["color", "face", "size"])
        .add_tag_attributes("table", ["border", "cellpadding", "cellspacing"])
        .add_tag_attributes("td", ["colspan", "rowspan", "background"])
        .add_tag_attributes("th", ["colspan", "rowspan"])
        .url_schemes(HashSet::from(["http", "https", "mailto", "cid", "data"]))
        .url_relative(UrlRelative::Deny)
        .attribute_filter(move |tag, attribute, value| match (tag, attribute) {
            // Remote images are the classic tracking pixel, so they go first of all.
            ("img", "src") => {
                let lower = value.trim_start().to_ascii_lowercase();
                if let Some(id) = value.strip_prefix("cid:") {
                    images.get(id).cloned().map(Into::into)
                } else if lower.starts_with("https://") || lower.starts_with("http://") {
                    let image = fetched.get(value).cloned();
                    if image.is_none() {
                        let mut blocked = seen.lock().expect("blocked images");
                        if !blocked.iter().any(|url| url == value) {
                            blocked.push(value.to_string());
                        }
                    }
                    image.map(Into::into)
                } else {
                    is_safe_data_image(value).then(|| value.to_string().into())
                }
            }
            ("a", "href") => {
                let lower = value.trim_start().to_ascii_lowercase();
                let allowed = ["http:", "https:", "mailto:"].iter().any(|s| lower.starts_with(s));
                allowed.then(|| value.into())
            }
            (_, "background") => None,
            _ => Some(value.into()),
        })
        .clean(html)
        .to_string();
    let blocked = std::mem::take(&mut *blocked.lock().expect("blocked images"));
    (format!("{SHEET}{cleaned}</body></html>"), blocked)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clean(html: &str) -> String {
        document(html, &HashMap::new()).strip_prefix(SHEET).unwrap().strip_suffix("</body></html>").unwrap().to_string()
    }

    #[test]
    fn scripts_and_active_content_are_removed() {
        for attack in [
            "<script>alert(1)</script>",
            "<img src=x onerror=alert(1)>",
            "<svg onload=alert(1)><script>alert(1)</script></svg>",
            "<svg><style><img src=x onerror=alert(1)></style></svg>",
            "<math><mtext><style><img src=x onerror=alert(1)></style></mtext></math>",
            "<iframe src=https://evil.example></iframe>",
            "<object data=https://evil.example></object>",
            "<embed src=https://evil.example>",
            "<form action=https://evil.example><input name=p></form>",
            "<meta http-equiv=refresh content='0;url=https://evil.example'>",
            "<link rel=stylesheet href=https://evil.example/a.css>",
            "<base href=https://evil.example/>",
            "<a href='javascript:alert(1)'>x</a>",
            "<a href='  JaVaScRiPt:alert(1)'>x</a>",
            "<a href='data:text/html,<script>alert(1)</script>'>x</a>",
            "<a href='vbscript:x'>x</a>",
        ] {
            let out = clean(attack).to_lowercase();
            for banned in ["<script", "onerror", "onload", "<iframe", "<object", "<embed", "<form", "<input", "<meta", "<link", "<base", "<svg", "<math", "javascript:", "vbscript:", "data:text"] {
                assert!(!out.contains(banned), "{attack} left {banned} in {out}");
            }
        }
    }

    #[test]
    fn links_that_reach_local_files_or_shares_are_removed() {
        for href in ["file:///c:/windows/win.ini", "\\\\evil.example\\share\\a", "ms-msdt:/id", "relative/path", "//evil.example/x"] {
            let out = clean(&format!("<a href='{href}'>x</a>"));
            assert!(!out.contains("href"), "{href} survived in {out}");
        }
    }

    #[test]
    fn ordinary_links_stay() {
        assert!(clean("<a href='https://example.com/a?b=1'>x</a>").contains("href=\"https://example.com/a?b=1\""));
        assert!(clean("<a href='mailto:a@example.com'>x</a>").contains("mailto:a@example.com"));
    }

    #[test]
    fn remote_images_lose_their_source_but_keep_their_size() {
        let out = clean("<img src='https://tracker.example/p.gif' width=1 height=1 alt=logo>");
        assert!(!out.contains("tracker.example"));
        assert!(out.contains("alt=\"logo\""));
    }

    #[test]
    fn remote_images_are_listed_once_until_they_are_supplied() {
        let html = "<img src='https://a.example/1.png?x=1&amp;y=2'><img src='https://a.example/1.png?x=1&amp;y=2'>\
                    <img src='HTTP://b.example/2.gif'><img src='data:image/gif;base64,AAAA'>";
        let (out, blocked) = sanitise(html, &HashMap::new(), &HashMap::new());
        assert_eq!(blocked, ["https://a.example/1.png?x=1&y=2", "HTTP://b.example/2.gif"]);
        assert!(!out.contains("a.example") && !out.contains("b.example"), "{out}");

        let fetched = HashMap::from([(blocked[0].clone(), "data:image/png;base64,BBBB".to_string())]);
        let (out, blocked) = sanitise(html, &HashMap::new(), &fetched);
        assert_eq!(out.matches("data:image/png;base64,BBBB").count(), 2, "{out}");
        assert_eq!(blocked, ["HTTP://b.example/2.gif"]);
        assert!(!out.contains("a.example"), "{out}");
    }

    #[test]
    fn only_real_images_become_data_urls() {
        assert!(data_url(b"\x89PNG\r\n\x1a\nA").unwrap().starts_with("data:image/png;"));
        assert!(data_url(b"\xff\xd8\xffA").unwrap().starts_with("data:image/jpeg;"));
        assert!(data_url(b"GIF89aA").unwrap().starts_with("data:image/gif;"));
        assert!(data_url(b"RIFF0000WEBPVP8").unwrap().starts_with("data:image/webp;"));
        for not_an_image in [&b"<svg onload=alert(1)>"[..], b"<html>", b""] {
            assert_eq!(data_url(not_an_image), None);
        }
    }

    #[test]
    fn cid_images_become_data_urls_and_svg_data_is_dropped() {
        let images = HashMap::from([("logo@x".to_string(), "data:image/png;base64,AAAA".to_string())]);
        assert!(document("<img src='cid:logo@x'>", &images).contains("data:image/png;base64,AAAA"));
        assert!(!document("<img src='cid:other'>", &images).contains("cid:"));
        assert!(!clean("<img src='data:image/svg+xml;base64,AAAA'>").contains("svg"));
        assert!(clean("<img src='data:image/gif;base64,AAAA'>").contains("data:image/gif"));
    }

    #[test]
    fn a_title_is_not_body_text_and_hidden_preview_text_stays_hidden() {
        let out = clean(
            "<html><head><title>The subject again</title></head><body>\
             <div style='display:none;max-height:0'>Preview</div><p>Hello</p></body></html>",
        );
        assert!(!out.contains("The subject again"), "{out}");
        assert!(out.contains("display:none") && out.contains("Hello"), "{out}");
    }

    #[test]
    fn mail_fills_the_pane_and_only_mail_without_a_layout_gets_a_margin() {
        assert!(SHEET.contains("body{margin:0;padding:0;"));
        assert!(SHEET.contains("body:not(:has(table)){padding:22px 36px}"));
        assert!(!SHEET.contains("prefers-color-scheme"), "the page must not change with the theme");
    }

    #[test]
    fn newsletter_layout_survives() {
        let out = clean(
            "<html><head><style>.a{color:red}</style></head><body>\
             <table width=600 cellpadding=0 bgcolor='#fff'><tr><td colspan=2 style='padding:4px' class=a>hi</td></tr></table>\
             </body></html>",
        );
        assert!(out.contains("<style>.a{color:red}</style>"), "{out}");
        for kept in ["width=\"600\"", "cellpadding=\"0\"", "bgcolor", "colspan=\"2\"", "padding:4px", "class=\"a\""] {
            assert!(out.contains(kept), "lost {kept} in {out}");
        }
    }

    const MIXED: &[u8] = b"From: An <an@example.com>\r\nTo: =?UTF-8?Q?B=C3=ACnh?= <b@example.com>, c@example.com\r\n\
Subject: x\r\nContent-Type: multipart/mixed; boundary=b\r\n\r\n\
--b\r\nContent-Type: multipart/related; boundary=c\r\n\r\n\
--c\r\nContent-Type: text/html; charset=utf-8\r\n\r\n<p>Xin ch\xc3\xa0o</p><img src=\"cid:i1\">\r\n\
--c\r\nContent-Type: image/png\r\nContent-Transfer-Encoding: base64\r\nContent-ID: <i1>\r\nContent-Disposition: inline\r\n\r\nAAAA\r\n\
--c--\r\n\
--b\r\nContent-Type: application/pdf; name=\"h\xc3\xb3a-\xc4\x91\xc6\xa1n.pdf\"\r\nContent-Disposition: attachment; filename=\"h\xc3\xb3a-\xc4\x91\xc6\xa1n.pdf\"\r\n\r\nPDF\r\n\
--b--\r\n";

    #[test]
    fn html_mail_renders_with_inline_images_and_lists_real_attachments() {
        let rendered = message(MIXED).unwrap();
        let Body::Html(html) = rendered.body else { panic!("expected html") };
        assert!(html.contains("Xin chào"));
        assert!(html.contains("data:image/png;base64,AAAA"), "{html}");
        assert_eq!(rendered.attachments, ["hóa-đơn.pdf"]);
        assert_eq!(rendered.recipients, "Bìnhc@example.com".replace("Bình", "Bình, "));
    }

    #[test]
    fn plain_mail_stays_plain_so_no_webview_is_needed() {
        let raw = b"From: a@example.com\r\nSubject: x\r\nContent-Type: text/plain; charset=utf-8\r\n\r\nHello <b>there</b>\r\n";
        let rendered = message(raw).unwrap();
        assert_eq!(rendered.body, Body::Text("Hello <b>there</b>\r\n".into()));
    }

    #[test]
    fn alternative_mail_prefers_html() {
        let raw = b"From: a@example.com\r\nContent-Type: multipart/alternative; boundary=b\r\n\r\n\
--b\r\nContent-Type: text/plain\r\n\r\nplain\r\n--b\r\nContent-Type: text/html\r\n\r\n<b>rich</b>\r\n--b--\r\n";
        assert!(matches!(message(raw).unwrap().body, Body::Html(h) if h.contains("<b>rich</b>")));
    }
}
