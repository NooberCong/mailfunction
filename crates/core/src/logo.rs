//! Finds the logo for a mail domain. A brand's own BIMI record comes first, since publishing one
//! needs strict email authentication. After that comes whatever icon the domain's website offers.
//!
//! Every request goes straight to the domain concerned. A logo service would be simpler and see
//! the list of who writes to the user.

use std::cmp::Reverse;

use image::imageops::{self, FilterType};
use image::{ImageReader, Limits, Rgba, RgbaImage};
use windows::Win32::Foundation::WIN32_ERROR;
use windows::Win32::NetworkManagement::Dns::{
    DNS_QUERY_STANDARD, DNS_RECORDA, DNS_RECORDW, DNS_TYPE_TEXT, DnsFree, DnsFreeRecordList, DnsQuery_W,
};
use windows::core::HSTRING;

use crate::remote::{Logo, Web};

/// Stored at this size, and cut down to each avatar when shown.
const STORED: u32 = 128;
/// Below this an icon is a blur at avatar size, and the initials look better.
const SMALLEST: u32 = 32;
/// BIMI allows 32 KB. The rest is room for icons and home pages.
const MAX_ICON: usize = 1024 * 1024;
const MAX_PAGE: usize = 1024 * 1024;
const MAX_ICONS_TRIED: usize = 4;
/// How much of the tile an icon with its own transparent edge covers.
const ICON_SHARE: f32 = 0.68;

const DNS_NAME_ERROR: u32 = 9003;
const DNS_NO_RECORDS: u32 = 9501;

/// Where millions of unrelated people have addresses, so the logo says nothing about the sender.
const SHARED_MAIL: &[&str] = &[
    "aol.com", "fastmail.com", "gmail.com", "gmx.com", "gmx.de", "gmx.net", "googlemail.com", "hey.com", "hotmail.com",
    "icloud.com", "live.com", "mac.com", "mail.com", "mail.ru", "me.com", "msn.com", "outlook.com", "proton.me",
    "protonmail.com", "qq.com", "tutanota.com", "yahoo.com", "yahoo.com.vn", "yandex.com", "yandex.ru", "zoho.com",
];

/// The domain to ask for an address's logo: `em.canva.com` and `canva.com` are one brand.
fn domain(address: &str) -> Option<String> {
    let host = address.rsplit_once('@')?.1.trim().to_ascii_lowercase();
    psl::domain_str(&host).map(str::to_string)
}

/// As [`domain`], for one of the user's own accounts. gmail.com has no icon of its own to find.
pub fn account_domain(address: &str) -> Option<String> {
    domain(address).map(|d| if matches!(d.as_str(), "gmail.com" | "googlemail.com") { "google.com".into() } else { d })
}

/// As [`domain`], for mail from someone else, who keeps their initials on a shared domain.
pub fn sender_domain(address: &str) -> Option<String> {
    domain(address).filter(|d| !SHARED_MAIL.contains(&d.as_str()))
}

pub(crate) enum Dns {
    Records(Vec<String>),
    /// The name has no such records.
    Nothing,
    Failed,
}

pub(crate) fn find(web: &Web, txt: &dyn Fn(&str) -> Dns, domain: &str) -> Logo {
    let mut answered = false;
    match txt(&format!("default._bimi.{domain}")) {
        Dns::Failed => return Logo::Unknown,
        Dns::Nothing => {}
        Dns::Records(records) => {
            answered = true;
            let logo = records.iter().find_map(|r| bimi_location(r)).and_then(|url| icon(web, &url, &mut answered));
            if let Some(logo) = logo {
                return Logo::Found(logo);
            }
        }
    }

    let root = format!("https://{domain}");
    if let Some(logo) = icon(web, &format!("{root}/apple-touch-icon.png"), &mut answered) {
        return Logo::Found(logo);
    }
    match web.get(&format!("{root}/"), MAX_PAGE) {
        Ok(page) => {
            answered = true;
            let html = String::from_utf8_lossy(&page.bytes);
            for url in icons_in(&html, &page.url).iter().take(MAX_ICONS_TRIED) {
                if let Some(logo) = icon(web, url, &mut answered) {
                    return Logo::Found(logo);
                }
            }
        }
        Err(e) => answered |= matches!(e, ureq::Error::StatusCode(_)),
    }
    if let Some(logo) = icon(web, &format!("{root}/favicon.ico"), &mut answered) {
        return Logo::Found(logo);
    }
    if answered { Logo::Missing } else { Logo::Unknown }
}

/// Fetches an icon and prepares it, noting whether the server said anything at all.
fn icon(web: &Web, url: &str, answered: &mut bool) -> Option<Vec<u8>> {
    match web.get(url, MAX_ICON) {
        Ok(fetched) => {
            *answered = true;
            prepare(&fetched.bytes)
        }
        Err(e) => {
            *answered |= matches!(e, ureq::Error::StatusCode(_));
            None
        }
    }
}

/// The logo's address in a BIMI record, which looks like `v=BIMI1; l=https://x/logo.svg; a=;`.
fn bimi_location(record: &str) -> Option<String> {
    let mut tags = record.split(';').map(str::trim);
    if !tags.next()?.eq_ignore_ascii_case("v=BIMI1") {
        return None;
    }
    let location = tags.find_map(|t| t.strip_prefix("l="))?.trim();
    location.starts_with("https://").then(|| location.to_string())
}

/// The value of an attribute in one tag. `lower` is the same tag in lower case.
fn attribute<'a>(tag: &'a str, lower: &str, name: &str) -> Option<&'a str> {
    let start = [format!(" {name}=\""), format!(" {name}='"), format!(" {name}=")]
        .iter()
        .find_map(|needle| lower.find(needle.as_str()).map(|at| (at + needle.len(), needle.chars().last())))?;
    let (from, quote) = start;
    let rest = &tag[from..];
    let end = match quote {
        Some(q @ ('"' | '\'')) => rest.find(q)?,
        _ => rest.find(|c: char| c.is_whitespace() || c == '>').unwrap_or(rest.len()),
    };
    Some(&rest[..end])
}

/// The icons a page declares, best first: touch icons are made large, then by declared size.
fn icons_in(html: &str, page: &str) -> Vec<String> {
    let lower = html.to_ascii_lowercase();
    let head_end = lower.find("</head").unwrap_or(lower.len());
    let mut icons = Vec::new();
    let mut at = 0;
    while let Some(start) = lower[at..head_end].find("<link").map(|i| i + at) {
        let end = lower[start..head_end].find('>').map_or(head_end, |i| i + start);
        at = end;
        let (tag, tag_lower) = (&html[start..end], &lower[start..end]);
        let rel = attribute(tag_lower, tag_lower, "rel").unwrap_or_default();
        if !rel.split_whitespace().any(|word| matches!(word, "icon" | "apple-touch-icon" | "apple-touch-icon-precomposed")) {
            continue;
        }
        let Some(url) = attribute(tag, tag_lower, "href").and_then(|href| absolute(&href.replace("&amp;", "&"), page)) else {
            continue;
        };
        let size = attribute(tag_lower, tag_lower, "sizes")
            .and_then(|s| s.split(['x', ' ']).next()?.parse::<u32>().ok())
            .unwrap_or(0);
        icons.push((rel.contains("apple"), size, url));
    }
    icons.sort_by_key(|(touch, size, _)| Reverse((*touch, *size)));
    icons.into_iter().map(|(_, _, url)| url).collect()
}

/// Resolves a link on a page, keeping to https.
fn absolute(href: &str, page: &str) -> Option<String> {
    let href = href.trim();
    let after_scheme = page.strip_prefix("https://")?;
    let origin = &page[..page.len() - after_scheme.len() + after_scheme.find('/').unwrap_or(after_scheme.len())];
    if href.starts_with("https://") {
        Some(href.to_string())
    } else if let Some(rest) = href.strip_prefix("//") {
        Some(format!("https://{rest}"))
    } else if href.starts_with('/') {
        Some(format!("{origin}{href}"))
    } else if href.contains(':') {
        None
    } else {
        Some(format!("{origin}/{href}"))
    }
}

fn decode(bytes: &[u8]) -> Option<RgbaImage> {
    let start = bytes.iter().position(|b| !b.is_ascii_whitespace()).unwrap_or(0);
    let head = String::from_utf8_lossy(&bytes[start..bytes.len().min(start + 512)]).to_ascii_lowercase();
    if head.starts_with("<svg") || head.starts_with("<?xml") || head.starts_with("<!--") || head.contains("<svg") {
        return render_svg(bytes);
    }
    let mut reader = ImageReader::new(std::io::Cursor::new(bytes)).with_guessed_format().ok()?;
    let mut limits = Limits::default();
    limits.max_alloc = Some(64 * 1024 * 1024);
    reader.limits(limits);
    let image = reader.decode().ok()?.into_rgba8();
    (image.width().min(image.height()) >= SMALLEST).then_some(image)
}

fn render_svg(bytes: &[u8]) -> Option<RgbaImage> {
    use resvg::{tiny_skia, usvg};
    // By default a linked image is read from disk, and a logo comes from a stranger.
    let resolver = usvg::ImageHrefResolver { resolve_string: Box::new(|_, _| None), ..Default::default() };
    let options = usvg::Options { image_href_resolver: resolver, ..Default::default() };
    let tree = usvg::Tree::from_data(bytes, &options).ok()?;
    let size = tree.size();
    let scale = STORED as f32 / size.width().max(size.height());
    let mut pixmap = tiny_skia::Pixmap::new(STORED, STORED)?;
    let (x, y) = ((STORED as f32 - size.width() * scale) / 2.0, (STORED as f32 - size.height() * scale) / 2.0);
    resvg::render(&tree, tiny_skia::Transform::from_scale(scale, scale).post_translate(x, y), &mut pixmap.as_mut());
    // The pixmap is premultiplied, which only matters at the edges of shapes.
    let pixels = pixmap.pixels().iter().flat_map(|p| {
        let c = p.demultiply();
        [c.red(), c.green(), c.blue(), c.alpha()]
    });
    RgbaImage::from_raw(STORED, STORED, pixels.collect())
}

/// Squares an image onto a solid tile and returns it as a PNG, so that a logo is legible on any
/// background. An image with solid corners already is a tile, so it simply fills the frame.
fn prepare(bytes: &[u8]) -> Option<Vec<u8>> {
    let source = decode(bytes)?;
    let (w, h) = source.dimensions();
    let solid = [(0, 0), (w - 1, 0), (0, h - 1), (w - 1, h - 1)].iter().all(|&(x, y)| source.get_pixel(x, y)[3] == 255);
    let tile = if solid {
        imageops::resize(&cropped_square(&source), STORED, STORED, FilterType::Lanczos3)
    } else {
        let side = (STORED as f32 * ICON_SHARE) as u32;
        let scale = side as f32 / w.max(h) as f32;
        let (fit_w, fit_h) = (((w as f32 * scale) as u32).max(1), ((h as f32 * scale) as u32).max(1));
        let icon = imageops::resize(&source, fit_w, fit_h, FilterType::Lanczos3);
        let mut tile = RgbaImage::from_pixel(STORED, STORED, plate_for(&source));
        imageops::overlay(&mut tile, &icon, i64::from((STORED - fit_w) / 2), i64::from((STORED - fit_h) / 2));
        tile
    };
    let mut png = Vec::new();
    tile.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).ok()?;
    Some(png)
}

fn cropped_square(image: &RgbaImage) -> RgbaImage {
    let (w, h) = image.dimensions();
    let side = w.min(h);
    imageops::crop_imm(image, (w - side) / 2, (h - side) / 2, side, side).to_image()
}

/// White, unless the icon itself is almost white and would vanish on it.
fn plate_for(icon: &RgbaImage) -> Rgba<u8> {
    let inked = icon.pixels().filter(|p| p[3] > 128);
    let (count, light) = inked.fold((0u64, 0u64), |(n, sum), p| {
        (n + 1, sum + (u64::from(p[0]) * 3 + u64::from(p[1]) * 6 + u64::from(p[2])) / 10)
    });
    if count > 0 && light / count > 225 { Rgba([38, 34, 36, 255]) } else { Rgba([255, 255, 255, 255]) }
}

/// A stored logo at `size` pixels as RGBA, cut to a circle. The software renderer neither smooths
/// a scaled image nor clips one to a curve, so both are done here, once per logo.
pub fn avatar(png: &[u8], size: u32) -> Option<RgbaImage> {
    let stored = image::load_from_memory_with_format(png, image::ImageFormat::Png).ok()?.into_rgba8();
    let mut image = imageops::resize(&stored, size, size, FilterType::Lanczos3);
    let radius = size as f32 / 2.0;
    for (x, y, pixel) in image.enumerate_pixels_mut() {
        let distance = (x as f32 + 0.5 - radius).hypot(y as f32 + 0.5 - radius);
        pixel[3] = ((radius - distance + 0.5).clamp(0.0, 1.0) * f32::from(pixel[3])) as u8;
    }
    Some(image)
}

/// A name's TXT records, through the same resolver as everything else on this machine.
pub(crate) fn txt_records(name: &str) -> Dns {
    let mut results: *mut DNS_RECORDA = std::ptr::null_mut();
    let status = unsafe { DnsQuery_W(&HSTRING::from(name), DNS_TYPE_TEXT, DNS_QUERY_STANDARD, None, &raw mut results, None) };
    match status {
        WIN32_ERROR(0) => {}
        WIN32_ERROR(DNS_NAME_ERROR | DNS_NO_RECORDS) => return Dns::Nothing,
        _ => return Dns::Failed,
    }
    let mut records = Vec::new();
    // DnsQuery_W fills in the wide form of the record, whatever its declared type says.
    let mut next = results.cast::<DNS_RECORDW>();
    while let Some(record) = unsafe { next.as_ref() } {
        if record.wType == DNS_TYPE_TEXT.0 {
            let data = unsafe { &record.Data.TXT };
            let strings = unsafe { std::slice::from_raw_parts(data.pStringArray.as_ptr(), data.dwStringCount as usize) };
            records.push(strings.iter().filter_map(|s| unsafe { s.to_string().ok() }).collect::<String>());
        }
        next = record.pNext;
    }
    unsafe { DnsFree(Some(results.cast()), DnsFreeRecordList) };
    Dns::Records(records)
}

#[cfg(test)]
mod tests {
    use image::{ImageFormat, Rgba};

    use super::*;

    fn png(image: &RgbaImage) -> Vec<u8> {
        let mut bytes = Vec::new();
        image.write_to(&mut std::io::Cursor::new(&mut bytes), ImageFormat::Png).unwrap();
        bytes
    }

    fn stored(bytes: &[u8]) -> RgbaImage {
        image::load_from_memory(bytes).unwrap().into_rgba8()
    }

    #[test]
    fn subdomains_share_a_brand_and_shared_mail_has_no_logo() {
        assert_eq!(sender_domain("news@em.canva.com").as_deref(), Some("canva.com"));
        assert_eq!(sender_domain("a@mail.shopee.vn").as_deref(), Some("shopee.vn"));
        assert_eq!(sender_domain("a@x.co.uk.example.co.uk").as_deref(), Some("example.co.uk"));
        assert_eq!(sender_domain("friend@gmail.com"), None);
        assert_eq!(sender_domain("friend@GMAIL.COM"), None);
        assert_eq!(sender_domain("not an address"), None);
        assert_eq!(account_domain("me@gmail.com").as_deref(), Some("google.com"));
        assert_eq!(account_domain("me@rocketjump.io").as_deref(), Some("rocketjump.io"));
    }

    #[test]
    fn a_bimi_record_gives_its_https_location() {
        assert_eq!(bimi_location("v=BIMI1; l=https://a.example/l.svg; a=;").as_deref(), Some("https://a.example/l.svg"));
        assert_eq!(bimi_location("V=bimi1;l=https://a.example/l.svg").as_deref(), Some("https://a.example/l.svg"));
        assert_eq!(bimi_location("v=BIMI1; l=http://a.example/l.svg"), None);
        assert_eq!(bimi_location("v=BIMI1; l=;"), None);
        assert_eq!(bimi_location("v=spf1 include:x -all"), None);
    }

    #[test]
    fn a_page_gives_its_icons_best_first_and_https_only() {
        let html = r#"<html><HEAD>
            <link rel="stylesheet" href="/a.css">
            <link rel="icon" sizes="16x16" href="/16.png">
            <LINK REL='icon' SIZES='192x192' HREF='/192.png?a=1&amp;b=2'>
            <link rel="mask-icon" href="/mask.svg">
            <link href="https://cdn.example/touch.png" rel="apple-touch-icon">
            <link rel=icon href=rel.png>
            <link rel="shortcut icon" href="//cdn.example/s.ico">
            <link rel="icon" href="http://cdn.example/plain.png">
            <link rel="icon" href="data:image/png;base64,AAAA">
            </head><body><link rel="icon" href="/late.png"></body>"#;
        assert_eq!(
            icons_in(html, "https://www.example.com/en/"),
            [
                "https://cdn.example/touch.png",
                "https://www.example.com/192.png?a=1&b=2",
                "https://www.example.com/16.png",
                "https://www.example.com/rel.png",
                "https://cdn.example/s.ico",
            ]
        );
    }

    #[test]
    fn a_full_tile_fills_the_frame() {
        let tile = stored(&prepare(&png(&RgbaImage::from_pixel(180, 180, Rgba([200, 10, 30, 255])))).unwrap());
        assert_eq!(tile.dimensions(), (STORED, STORED));
        assert_eq!(*tile.get_pixel(2, 2), Rgba([200, 10, 30, 255]));
    }

    #[test]
    fn a_transparent_icon_gets_a_plate_it_shows_up_on() {
        let dark = stored(&prepare(&png(&RgbaImage::from_fn(64, 64, |x, _| {
            if x == 0 { Rgba([0, 0, 0, 0]) } else { Rgba([10, 10, 10, 255]) }
        })))
        .unwrap());
        assert_eq!(*dark.get_pixel(1, 1), Rgba([255, 255, 255, 255]));
        assert_eq!(dark.get_pixel(64, 64)[0], 10);

        let white = stored(&prepare(&png(&RgbaImage::from_fn(64, 64, |x, _| {
            if x == 0 { Rgba([0, 0, 0, 0]) } else { Rgba([255, 255, 255, 255]) }
        })))
        .unwrap());
        assert!(white.get_pixel(1, 1)[0] < 60);
    }

    #[test]
    fn an_icon_too_small_to_read_and_anything_undecodable_are_refused() {
        assert!(prepare(&png(&RgbaImage::new(16, 16))).is_none());
        assert!(prepare(b"<html>not an image</html>").is_none());
        assert!(prepare(b"").is_none());
    }

    #[test]
    fn an_svg_logo_is_drawn() {
        let svg = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 10 10"><rect width="10" height="10" fill="#c4122f"/></svg>"##;
        let tile = stored(&prepare(svg).unwrap());
        assert_eq!(*tile.get_pixel(64, 64), Rgba([0xc4, 0x12, 0x2f, 255]));
    }

    #[test]
    fn an_svg_cannot_read_a_local_file() {
        let dir = std::env::temp_dir().join(format!("mf-logo-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("secret.png");
        std::fs::write(&file, png(&RgbaImage::from_pixel(32, 32, Rgba([1, 200, 3, 255])))).unwrap();
        let svg = format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" viewBox="0 0 10 10"><image width="10" height="10" xlink:href="{}"/></svg>"#,
            file.display()
        );
        let image = render_svg(svg.as_bytes());
        assert!(image.is_none_or(|i| i.get_pixel(64, 64)[1] != 200), "the file was drawn");
    }

    #[test]
    fn an_avatar_is_a_soft_edged_circle_at_exactly_its_size() {
        let logo = png(&RgbaImage::from_pixel(STORED, STORED, Rgba([200, 10, 30, 255])));
        let circle = avatar(&logo, 57).unwrap();
        assert_eq!(circle.dimensions(), (57, 57));
        assert_eq!(circle.get_pixel(0, 0)[3], 0);
        assert_eq!(circle.get_pixel(28, 28)[3], 255);
    }
}
