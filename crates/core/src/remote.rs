//! Everything the app fetches from the open web: images in mail the user chose to load, and logos.
//!
//! The webview stays offline, so this is the only way out. It sends no cookies and no referrer,
//! and it refuses private addresses: a message can name any URL, and without that check it could
//! make this machine probe the router or anything else on its network.

use std::io::Read;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::time::Duration;

use ureq::config::Config;
use ureq::http::Uri;
use ureq::unversioned::resolver::{DefaultResolver, ResolvedSocketAddrs, Resolver};
use ureq::unversioned::transport::{DefaultConnector, NextTimeout};
use ureq::{Agent, Error, ResponseExt};

use crate::images::FETCHES;
use crate::logo;
use crate::render::{MAX_INLINE_IMAGE, image_type};

/// Some sites refuse anything that does not look like a browser, which would cost a logo.
const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) mailfunction";
const TIMEOUT: Duration = Duration::from_secs(8);

/// What the search for a domain's logo found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Logo {
    /// A PNG, ready to store.
    Found(Vec<u8>),
    /// The domain answered and has no usable logo.
    Missing,
    /// Nothing could be learned, probably for want of a network, so it is worth asking again.
    Unknown,
}

/// The app's way out to the web. Tests supply their own, so that none of them touches it.
pub trait Network: Send + Sync {
    /// An image's bytes, or nothing if it could not be fetched or is not an image.
    fn image(&self, url: &str) -> Option<Vec<u8>>;
    fn logo(&self, domain: &str) -> Logo;
}

/// No network at all.
pub struct Offline;

impl Network for Offline {
    fn image(&self, _: &str) -> Option<Vec<u8>> {
        None
    }

    fn logo(&self, _: &str) -> Logo {
        Logo::Unknown
    }
}

/// Whether anyone on the public internet could be at this address.
fn is_public(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => is_public_v4(ip),
        IpAddr::V6(ip) => match ip.to_ipv4_mapped() {
            Some(ip) => is_public_v4(ip),
            None => is_public_v6(ip),
        },
    }
}

fn is_public_v4(ip: Ipv4Addr) -> bool {
    let [a, b, ..] = ip.octets();
    let shared = a == 100 && (64..128).contains(&b);
    let benchmarking = a == 198 && (18..20).contains(&b);
    !(ip.is_private()
        || ip.is_loopback()
        || ip.is_link_local()
        || ip.is_multicast()
        || ip.is_broadcast()
        || ip.is_unspecified()
        || ip.is_documentation()
        || shared
        || benchmarking
        || a == 0
        || a >= 240)
}

fn is_public_v6(ip: Ipv6Addr) -> bool {
    let segments = ip.segments();
    let v4 = |high: u16, low: u16| Ipv4Addr::from(u32::from(high) << 16 | u32::from(low));
    // These carry an IPv4 address inside, so it is that address that would be reached.
    match segments {
        [0, 0, 0, 0, 0, 0, high, low] | [0x64, 0xff9b, 0, 0, 0, 0, high, low] | [0x2002, high, low, ..] => {
            return is_public_v4(v4(high, low));
        }
        _ => {}
    }
    let first = segments[0];
    let unique_local = first & 0xfe00 == 0xfc00;
    let link_local = first & 0xffc0 == 0xfe80;
    let site_local = first & 0xffc0 == 0xfec0;
    !(ip.is_multicast() || unique_local || link_local || site_local)
}

/// Checks addresses at the moment of connecting, which is the only check that holds: one made
/// beforehand would miss redirects, and a name that resolves differently the second time.
#[derive(Debug)]
struct PublicOnly {
    allow_local: bool,
}

impl Resolver for PublicOnly {
    fn resolve(&self, uri: &Uri, config: &Config, timeout: NextTimeout) -> Result<ResolvedSocketAddrs, Error> {
        let mut allowed = self.empty();
        for address in DefaultResolver::default().resolve(uri, config, timeout)?.iter() {
            if self.allow_local || is_public(address.ip()) {
                allowed.push(*address);
            }
        }
        if allowed.is_empty() { Err(Error::HostNotFound) } else { Ok(allowed) }
    }
}

/// A fetched body, and where it ended up after any redirects.
pub(crate) struct Fetched {
    pub bytes: Vec<u8>,
    pub url: String,
}

pub struct Web {
    agent: Agent,
}

impl Default for Web {
    fn default() -> Self {
        Self::with_resolver(PublicOnly { allow_local: false })
    }
}

impl Web {
    fn with_resolver(resolver: PublicOnly) -> Self {
        let config = Agent::config_builder()
            .proxy(None)
            .user_agent(USER_AGENT)
            .timeout_global(Some(TIMEOUT))
            // A newsletter's images tend to share a host, and a new connection costs a handshake.
            .max_idle_connections_per_host(FETCHES)
            .build();
        Self { agent: Agent::with_parts(config, DefaultConnector::default(), resolver) }
    }

    /// The body at a URL, up to `limit` bytes. Anything longer fails rather than arriving cut off.
    pub(crate) fn get(&self, url: &str, limit: usize) -> Result<Fetched, Error> {
        let mut response = self.agent.get(url).call()?;
        let url = response.get_uri().to_string();
        let mut bytes = Vec::new();
        let body = response.body_mut().as_reader();
        body.take(limit as u64 + 1).read_to_end(&mut bytes).map_err(Error::Io)?;
        if bytes.len() > limit {
            return Err(Error::BodyExceedsLimit(limit as u64));
        }
        Ok(Fetched { bytes, url })
    }
}

impl Network for Web {
    fn image(&self, url: &str) -> Option<Vec<u8>> {
        let bytes = self.get(url, MAX_INLINE_IMAGE).ok()?.bytes;
        image_type(&bytes).map(|_| bytes)
    }

    fn logo(&self, domain: &str) -> Logo {
        logo::find(self, &logo::txt_records, domain)
    }
}

#[cfg(test)]
pub(crate) fn web_for_local_tests() -> Web {
    Web::with_resolver(PublicOnly { allow_local: true })
}

#[cfg(test)]
mod tests {
    use std::io::Write;
    use std::net::TcpListener;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    use super::*;

    const PNG: &[u8] = b"\x89PNG\r\n\x1a\nrest";

    /// Answers every request with the same body, for as many requests as arrive.
    fn serve(body: &'static [u8]) -> (String, Arc<AtomicBool>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/a.png", listener.local_addr().unwrap());
        let reached = Arc::new(AtomicBool::new(false));
        let flag = reached.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { return };
                flag.store(true, Ordering::SeqCst);
                let mut request = [0; 2048];
                let _ = std::io::Read::read(&mut stream, &mut request);
                let head = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
                let _ = stream.write_all(head.as_bytes());
                let _ = stream.write_all(body);
            }
        });
        (url, reached)
    }

    #[test]
    fn only_public_addresses_pass() {
        for private in [
            "127.0.0.1", "10.1.2.3", "172.16.0.1", "192.168.1.1", "169.254.169.254", "100.64.0.1", "0.0.0.0",
            "224.0.0.1", "255.255.255.255", "::1", "::", "fe80::1", "fc00::1", "fd12::1", "ff02::1",
            "::ffff:127.0.0.1", "::ffff:192.168.1.1", "198.18.0.1", "198.19.255.1", "fec0::1",
            "::127.0.0.1", "::10.0.0.1", "64:ff9b::7f00:1", "64:ff9b::c0a8:101", "2002:7f00:1::", "2002:c0a8:101::1",
        ] {
            assert!(!is_public(private.parse().unwrap()), "{private} passed");
        }
        for public in ["1.1.1.1", "8.8.8.8", "142.250.1.1", "198.20.0.1", "2606:4700::1111", "::ffff:8.8.8.8", "64:ff9b::808:808", "2002:808:808::"] {
            assert!(is_public(public.parse().unwrap()), "{public} refused");
        }
    }

    #[test]
    fn a_local_address_in_a_message_is_never_contacted() {
        let (url, reached) = serve(PNG);
        assert_eq!(Web::default().image(&url), None);
        assert!(!reached.load(Ordering::SeqCst), "the local server was contacted");
    }

    #[test]
    fn only_images_arrive() {
        let (image, _) = serve(PNG);
        let (page, _) = serve(b"<svg onload=alert(1)>");
        let web = web_for_local_tests();
        assert_eq!(web.image(&image).as_deref(), Some(PNG));
        assert_eq!(web.image(&page), None);
    }

    #[test]
    fn a_body_over_the_limit_fails_instead_of_arriving_cut_off() {
        let (url, _) = serve(b"0123456789");
        let web = web_for_local_tests();
        assert_eq!(web.get(&url, 10).map(|f| f.bytes.len()).ok(), Some(10));
        assert!(web.get(&url, 9).is_err());
    }
}
