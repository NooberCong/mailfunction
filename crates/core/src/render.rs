//! Turns a raw message into something safe to show. This is the first of three layers against
//! hostile mail: an allowlist sanitiser here, then a CSP header, then a webview that has
//! scripts off and answers every network request with a refusal.

use std::collections::{HashMap, HashSet};

use ammonia::{Builder, UrlRelative};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use mail_parser::{MessageParser, MessageRef, PartKind};

/// Sent as a real response header by the webview layer, so mail cannot remove it.
pub const CONTENT_SECURITY_POLICY: &str =
    "default-src 'none'; img-src data:; style-src 'unsafe-inline'; base-uri 'none'; form-action 'none'";

const SHEET: &str = "<!doctype html><html><head><meta charset=\"utf-8\"><style>\
    html{background:#fff;color:#231417;color-scheme:light}\
    body{margin:0;padding:22px 36px;font:14px/1.6 'Segoe UI Variable Text','Segoe UI',sans-serif;overflow-wrap:anywhere;\
    animation:mf-in .17s cubic-bezier(.22,1,.36,1)}\
    @keyframes mf-in{from{opacity:0;transform:translateY(6px)}}\
    img{max-width:100%;height:auto}\
    ::-webkit-scrollbar{width:10px;height:10px}\
    ::-webkit-scrollbar-thumb{background:#e6dadc;border-radius:5px;border:2px solid #fff}\
    @media (prefers-reduced-motion:reduce){body{animation:none}}\
    @media (prefers-color-scheme:dark){html{background:#1b1516}\
    body{margin:16px 36px;padding:20px 24px;background:#fff;border-radius:12px}\
    ::-webkit-scrollbar-thumb{background:#4a3a3d;border-color:#1b1516}}\
    </style></head><body>";

/// Inline images larger than this are dropped rather than embedded in the page.
const MAX_INLINE_IMAGE: usize = 8 * 1024 * 1024;

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
}

/// A message's body and details, or `None` if the bytes are not a message at all.
pub fn message(raw: &[u8]) -> Option<Rendered> {
    let parsed = MessageParser::default().parse(raw)?;
    let message = parsed.root();
    let html = message.html_body().find(|p| matches!(p.kind(), PartKind::Html));
    let body = match html.and_then(|_| message.body_html(0)) {
        Some(html) => Body::Html(document(&html, &inline_images(message))),
        None => Body::Text(message.body_text(0).map(|t| t.into_owned()).unwrap_or_default()),
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
    Some(Rendered { body, recipients, attachments })
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

fn is_safe_data_image(value: &str) -> bool {
    ["png", "jpeg", "jpg", "gif", "webp"]
        .iter()
        .any(|t| value.to_ascii_lowercase().starts_with(&format!("data:image/{t};base64,")))
}

/// Sanitises mail HTML into a complete document.
pub fn document(html: &str, inline_images: &HashMap<String, String>) -> String {
    let images = inline_images.clone();
    let cleaned = Builder::default()
        .add_tags(["center", "font", "style", "tbody", "thead", "tfoot"])
        .rm_clean_content_tags(["style"])
        .add_generic_attributes(["align", "bgcolor", "class", "dir", "height", "id", "style", "valign", "width"])
        .add_tag_attributes("font", ["color", "face", "size"])
        .add_tag_attributes("table", ["border", "cellpadding", "cellspacing"])
        .add_tag_attributes("td", ["colspan", "rowspan", "background"])
        .add_tag_attributes("th", ["colspan", "rowspan"])
        .url_schemes(HashSet::from(["http", "https", "mailto", "cid", "data"]))
        .url_relative(UrlRelative::Deny)
        .attribute_filter(move |tag, attribute, value| match (tag, attribute) {
            // Remote images are the classic tracking pixel, so they go first of all.
            ("img", "src") => match value.strip_prefix("cid:") {
                Some(id) => images.get(id).cloned().map(Into::into),
                None => is_safe_data_image(value).then(|| value.to_string().into()),
            },
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
    format!("{SHEET}{cleaned}</body></html>")
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
    fn cid_images_become_data_urls_and_svg_data_is_dropped() {
        let images = HashMap::from([("logo@x".to_string(), "data:image/png;base64,AAAA".to_string())]);
        assert!(document("<img src='cid:logo@x'>", &images).contains("data:image/png;base64,AAAA"));
        assert!(!document("<img src='cid:other'>", &images).contains("cid:"));
        assert!(!clean("<img src='data:image/svg+xml;base64,AAAA'>").contains("svg"));
        assert!(clean("<img src='data:image/gif;base64,AAAA'>").contains("data:image/gif"));
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
