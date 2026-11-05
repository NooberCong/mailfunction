//! An IMAP session. Sync opens folders with EXAMINE and reads bodies with BODY.PEEK, so reading
//! changes nothing, not even a message's read state. Only [`crate::ops`] changes a mailbox, and
//! it does so through the commands at the end of [`Imap`], on a folder opened with SELECT.
//!
//! Commands go through [`Imap::run`] rather than async-imap's typed helpers, because those
//! offer no `EXAMINE (CONDSTORE)` and hide X-GM-THRID.

use std::fmt::Debug;
use std::sync::Arc;
use std::time::Duration;

use async_imap::imap_proto::{AttributeValue, MailboxDatum, NameAttribute, Response, ResponseCode, Status};
use async_imap::imap_proto::rfc4315::UidSetMember;
use async_imap::{Authenticator, Client, Session};
use base64::Engine as _;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpStream;
use tokio::sync::Notify;
use tokio_rustls::TlsConnector;
use tokio_rustls::rustls::pki_types::ServerName;
use tokio_rustls::rustls::{ClientConfig, RootCertStore};

use crate::store::{Account, Role, Security};
use crate::{Error, Result};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);
/// A stalled socket must fail, or sync would hang forever with no error to act on.
const COMMAND_TIMEOUT: Duration = Duration::from_secs(120);
/// A message is read in pieces this big, so that no single response can be unbounded.
const BODY_CHUNK: usize = 4 * 1024 * 1024;
/// Gmail's own limit is about 50 MB, so anything beyond this is not mail.
const MAX_MESSAGE: usize = 96 * 1024 * 1024;
/// The bytes a second an upload is given time for.
const SLOWEST_UPLOAD: u64 = 16 * 1024;
/// A search of the server keeps this many of the newest matches.
const MAX_SEARCH_RESULTS: usize = 200;

pub trait Stream: AsyncRead + AsyncWrite + Unpin + Send + Debug {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send + Debug> Stream for T {}

pub enum Credential {
    Password(String),
    AccessToken(String),
}

struct XOAuth2 {
    payload: String,
    sent: bool,
}

impl Authenticator for XOAuth2 {
    type Response = String;

    /// After a rejected token the server sends an error challenge, which must be answered
    /// with an empty response before it will finish the exchange.
    fn process(&mut self, _challenge: &[u8]) -> String {
        if std::mem::replace(&mut self.sent, true) { String::new() } else { self.payload.clone() }
    }
}

async fn open_stream(account: &Account) -> Result<Box<dyn Stream>> {
    let tcp = tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect((account.host.as_str(), account.port)))
        .await
        .map_err(|_| Error::Protocol(format!("timed out connecting to {}", account.host)))??;
    tcp.set_nodelay(true)?;
    match account.security {
        Security::Tls => {
            let roots = RootCertStore::from_iter(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
            let config = ClientConfig::builder().with_root_certificates(roots).with_no_client_auth();
            let name = ServerName::try_from(account.host.clone())
                .map_err(|_| Error::Protocol(format!("bad host name {}", account.host)))?;
            Ok(Box::new(TlsConnector::from(Arc::new(config)).connect(name, tcp).await?))
        }
        Security::Plain if tcp.peer_addr()?.ip().is_loopback() => Ok(Box::new(tcp)),
        Security::Plain => Err(Error::Protocol("refusing an unencrypted connection to a remote host".into())),
    }
}

/// What EXAMINE reports about a folder.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct FolderStatus {
    pub exists: u32,
    pub uid_validity: u32,
    pub uid_next: u32,
    pub highest_modseq: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteFolder {
    /// Exactly as the server spelled it, for use in commands.
    pub path: String,
    /// The folder's name for display.
    pub name: String,
    pub role: Role,
}

/// One FETCH response.
#[derive(Debug, Default)]
pub struct Fetched {
    pub uid: u32,
    pub seen: bool,
    pub flagged: bool,
    pub date: i64,
    pub size: u32,
    pub gm_message_id: Option<u64>,
    pub gm_thread_id: Option<u64>,
    /// Decoded; system labels keep their leading backslash.
    pub labels: Option<Vec<String>>,
    pub data: Option<Vec<u8>>,
}

/// imap-proto hands back quoted strings still escaped, so `"\\Inbox"` arrives with two backslashes.
/// A bare `\Inbox` has nothing to undo: only `\\` and `\"` are escapes.
fn unescape(quoted: &str) -> String {
    quoted.replace("\\\\", "\\").replace("\\\"", "\"")
}

impl Fetched {
    fn parse(attrs: &[AttributeValue]) -> Self {
        let mut f = Self::default();
        for attr in attrs {
            match attr {
                AttributeValue::Uid(uid) => f.uid = *uid,
                AttributeValue::Rfc822Size(size) => f.size = *size,
                AttributeValue::GmailMsgId(id) => f.gm_message_id = Some(*id),
                AttributeValue::GmailThrId(id) => f.gm_thread_id = Some(*id),
                AttributeValue::GmailLabels(labels) => {
                    f.labels = Some(labels.iter().map(|l| decode_utf7(&unescape(l))).collect());
                }
                AttributeValue::InternalDate(date) => {
                    f.date = chrono::DateTime::parse_from_str(date.trim(), "%d-%b-%Y %H:%M:%S %z")
                        .map_or(0, |d| d.timestamp());
                }
                AttributeValue::Flags(flags) => {
                    f.seen = flags.iter().any(|x| x.eq_ignore_ascii_case("\\Seen"));
                    f.flagged = flags.iter().any(|x| x.eq_ignore_ascii_case("\\Flagged"));
                }
                AttributeValue::BodySection { data: Some(data), .. } => f.data = Some(data.to_vec()),
                _ => {}
            }
        }
        f
    }
}

pub struct Imap {
    session: Session<Box<dyn Stream>>,
    /// True on Gmail, which adds labels and stable IDs (X-GM-EXT-1).
    pub gmail: bool,
    /// The folder open for writing, if any, and its UIDVALIDITY.
    selected: Option<(String, u32)>,
}

impl Imap {
    pub async fn connect(account: &Account, credential: &Credential) -> Result<Self> {
        let mut client = Client::new(open_stream(account).await?);
        client.read_response().await?.ok_or_else(|| Error::Protocol("no server greeting".into()))?;
        let auth_failed = |(e, _)| match e {
            async_imap::error::Error::No(m) | async_imap::error::Error::Bad(m) => Error::Auth(m),
            other => Error::Imap(other),
        };
        let mut session = match credential {
            Credential::Password(password) => client
                .login(&account.email, password)
                .await
                .map_err(auth_failed)?,
            Credential::AccessToken(token) => {
                let payload = format!("user={}\x01auth=Bearer {token}\x01\x01", account.email);
                client
                    .authenticate("XOAUTH2", XOAuth2 { payload, sent: false })
                    .await
                    .map_err(auth_failed)?
            }
        };
        let capabilities = session.capabilities().await?;
        if !capabilities.has_str("CONDSTORE") {
            return Err(Error::Protocol(format!("{} does not support CONDSTORE", account.host)));
        }
        let gmail = capabilities.has_str("X-GM-EXT-1");
        Ok(Self { session, gmail, selected: None })
    }

    /// Runs one command, handing each untagged response to `on_response`.
    async fn run(&mut self, command: &str, mut on_response: impl FnMut(&Response)) -> Result<()> {
        let tag = self.session.run_command(command).await?;
        loop {
            let next = tokio::time::timeout(COMMAND_TIMEOUT, self.session.read_response())
                .await
                .map_err(|_| Error::Protocol("the server stopped responding".into()))??
                .ok_or(async_imap::error::Error::ConnectionLost)?;
            match next.parsed() {
                Response::Done { tag: done, status, outcome } if *done == tag => {
                    on_response(next.parsed());
                    return match status {
                        Status::Ok => Ok(()),
                        _ => Err(Error::Refused(format!(
                            "{}: {}",
                            command.split(' ').take(2).collect::<Vec<_>>().join(" "),
                            outcome.information.as_deref().unwrap_or("refused")
                        ))),
                    };
                }
                response => on_response(response),
            }
        }
    }

    pub async fn list(&mut self) -> Result<Vec<RemoteFolder>> {
        let mut folders = Vec::new();
        self.run("LIST \"\" \"*\"", |response| {
            if let Response::MailboxData(MailboxDatum::List(item)) = response {
                let selectable = !item.name_attributes.iter().any(|a| matches!(a, NameAttribute::NoSelect));
                if selectable {
                    let path = item.name.to_string();
                    let role = folder_role(&path, &item.name_attributes);
                    let decoded = decode_utf7(&path);
                    let leaf = item.delimiter.as_deref().and_then(|d| decoded.rsplit(d).next());
                    let name = leaf.unwrap_or(&decoded).to_string();
                    folders.push(RemoteFolder { path, name, role });
                }
            }
        })
        .await?;
        Ok(folders)
    }

    /// Opens a folder read-only.
    pub async fn examine(&mut self, path: &str) -> Result<FolderStatus> {
        self.selected = None;
        let mut status = FolderStatus::default();
        self.run(&format!("EXAMINE {} (CONDSTORE)", quote(path)?), |response| match response {
            Response::MailboxData(MailboxDatum::Exists(n)) => status.exists = *n,
            Response::Data { outcome, .. } => match outcome.code {
                Some(ResponseCode::UidValidity(v)) => status.uid_validity = v,
                Some(ResponseCode::UidNext(v)) => status.uid_next = v,
                Some(ResponseCode::HighestModSeq(v)) => status.highest_modseq = v,
                _ => {}
            },
            _ => {}
        })
        .await?;
        Ok(status)
    }

    async fn fetch(&mut self, command: &str) -> Result<Vec<Fetched>> {
        let mut fetched = Vec::new();
        self.run(command, |response| {
            if let Response::Fetch(_, attrs) = response {
                fetched.push(Fetched::parse(attrs));
            }
        })
        .await?;
        Ok(fetched)
    }

    fn gmail_items(&self) -> &'static str {
        if self.gmail { " X-GM-MSGID X-GM-THRID X-GM-LABELS" } else { "" }
    }

    /// Headers and state for a UID range. `high` of `None` means "to the end of the folder".
    pub async fn fetch_headers(&mut self, low: u32, high: Option<u32>) -> Result<Vec<Fetched>> {
        let high = high.map_or("*".to_string(), |h| h.to_string());
        let extra = self.gmail_items();
        let mut fetched = self
            .fetch(&format!(
                "UID FETCH {low}:{high} (UID FLAGS INTERNALDATE RFC822.SIZE{extra} \
                 BODY.PEEK[HEADER.FIELDS (FROM SUBJECT MESSAGE-ID REFERENCES IN-REPLY-TO)])"
            ))
            .await?;
        // `n:*` always returns the last message, even when its UID is below n.
        fetched.retain(|f| f.uid >= low);
        Ok(fetched)
    }

    /// Flags and labels of every message changed since `modseq`.
    pub async fn fetch_changes(&mut self, modseq: u64) -> Result<Vec<Fetched>> {
        let labels = if self.gmail { " X-GM-LABELS" } else { "" };
        self.fetch(&format!("UID FETCH 1:* (UID FLAGS{labels}) (CHANGEDSINCE {modseq})")).await
    }

    pub async fn all_uids(&mut self) -> Result<Vec<u32>> {
        let mut uids = Vec::new();
        self.run("UID SEARCH ALL", |response| {
            if let Response::MailboxData(MailboxDatum::Search(found)) = response {
                uids.extend_from_slice(found);
            }
        })
        .await?;
        Ok(uids)
    }

    /// The whole raw message, or `None` if the UID no longer exists. It comes in pieces, and a
    /// message past [`MAX_MESSAGE`] is refused rather than held in memory.
    pub async fn fetch_body(&mut self, uid: u32) -> Result<Option<Vec<u8>>> {
        let mut raw = Vec::new();
        loop {
            let command = format!("UID FETCH {uid} (UID BODY.PEEK[]<{}.{BODY_CHUNK}>)", raw.len());
            let fetched = self.fetch(&command).await?;
            let Some(piece) = fetched.into_iter().find(|f| f.uid == uid) else {
                return Ok(None);
            };
            let piece = piece.data.unwrap_or_default();
            raw.extend_from_slice(&piece);
            if piece.len() < BODY_CHUNK {
                return Ok(Some(raw));
            }
            if raw.len() > MAX_MESSAGE {
                return Err(Error::Refused(format!("UID {uid} is too large to open")));
            }
        }
    }

    /// The newest UIDs in the open folder that match `query`, on the server's own index, which
    /// knows the text of mail we have not downloaded. Gmail's search syntax works there.
    pub async fn search_text(&mut self, query: &str) -> Result<Vec<u32>> {
        let key = if self.gmail { "X-GM-RAW" } else { "TEXT" };
        let mut uids = self.search(&format!("{key} {}", quote(query)?)).await?;
        uids.sort_unstable_by(|a, b| b.cmp(a));
        uids.truncate(MAX_SEARCH_RESULTS);
        Ok(uids)
    }

    async fn search(&mut self, criteria: &str) -> Result<Vec<u32>> {
        let mut uids = Vec::new();
        self.run(&format!("UID SEARCH CHARSET UTF-8 {criteria}"), |response| {
            if let Response::MailboxData(MailboxDatum::Search(found)) = response {
                uids.extend_from_slice(found);
            }
        })
        .await?;
        Ok(uids)
    }

    /// UIDs in the open folder with this Message-ID, given without its angle brackets.
    pub async fn find_message_id(&mut self, mid: &str) -> Result<Vec<u32>> {
        self.search(&format!("HEADER Message-ID {}", quote(&format!("<{mid}>"))?)).await
    }

    /// Opens a folder for writing, unless it already is, and returns its UIDVALIDITY.
    pub async fn select(&mut self, path: &str) -> Result<u32> {
        if let Some((_, validity)) = self.selected.as_ref().filter(|(open, _)| open == path) {
            return Ok(*validity);
        }
        let mut validity = 0;
        self.run(&format!("SELECT {}", quote(path)?), |response| {
            if let Response::Data { outcome, .. } = response
                && let Some(ResponseCode::UidValidity(v)) = outcome.code
            {
                validity = v;
            }
        })
        .await?;
        self.selected = Some((path.to_string(), validity));
        Ok(validity)
    }

    pub async fn store_flag(&mut self, uids: &[u32], flag: &str, on: bool) -> Result<()> {
        let sign = if on { '+' } else { '-' };
        let uids = uids.iter().map(u32::to_string).collect::<Vec<_>>().join(",");
        self.run(&format!("UID STORE {uids} {sign}FLAGS.SILENT ({flag})"), |_| {}).await
    }

    /// Adds or removes a Gmail label, spelled as in X-GM-LABELS.
    pub async fn store_label(&mut self, uid: u32, label: &str, on: bool) -> Result<()> {
        let sign = if on { '+' } else { '-' };
        // System labels are atoms, as Google's documentation writes them.
        let label = if label.starts_with('\\') { label.to_string() } else { quote(&encode_utf7(label))? };
        self.run(&format!("UID STORE {uid} {sign}X-GM-LABELS ({label})"), |_| {}).await
    }

    /// Moves a message from the open folder, and returns its UID in `target` if the server says.
    pub async fn move_to(&mut self, uid: u32, target: &str) -> Result<Option<u32>> {
        let mut moved = None;
        self.run(&format!("UID MOVE {uid} {}", quote(target)?), |response| {
            let outcome = match response {
                Response::Data { outcome, .. } | Response::Done { outcome, .. } => outcome,
                _ => return,
            };
            if let Some(ResponseCode::CopyUid(_, _, targets)) = &outcome.code
                && let Some(UidSetMember::Uid(new)) = targets.first()
            {
                moved = Some(*new);
            }
        })
        .await?;
        Ok(moved)
    }

    /// Adds a message to a folder. Anything open stays open, so this can run at any time.
    pub async fn append(&mut self, path: &str, flags: &str, raw: &[u8]) -> Result<()> {
        // A large message on a slow line takes as long as it takes, so this allows for that.
        let limit = COMMAND_TIMEOUT + Duration::from_secs(raw.len() as u64 / SLOWEST_UPLOAD);
        tokio::time::timeout(limit, self.session.append(path, Some(flags), None, raw))
            .await
            .map_err(|_| Error::Protocol("the server stopped responding".into()))?
            .map_err(|e| match e {
                async_imap::error::Error::No(m) | async_imap::error::Error::Bad(m) => Error::Refused(m),
                other => Error::Imap(other),
            })
    }

    /// Deletes these messages from the open folder for good.
    pub async fn delete(&mut self, uids: &[u32]) -> Result<()> {
        if uids.is_empty() {
            return Ok(());
        }
        let set = uids.iter().map(u32::to_string).collect::<Vec<_>>().join(",");
        self.run(&format!("UID STORE {set} +FLAGS.SILENT (\\Deleted)"), |_| {}).await?;
        self.run(&format!("UID EXPUNGE {set}"), |_| {}).await
    }

    /// Waits on the open folder until the server reports a change, `limit` passes or `wake`
    /// fires, or there is `queued` work. Takes the session and gives it back because IDLE owns the
    /// connection meanwhile. The flag says whether it was queued work that ended the wait.
    pub async fn idle(self, limit: Duration, wake: &Notify, queued: &Notify) -> Result<(Self, bool)> {
        let gmail = self.gmail;
        let mut handle = self.session.idle();
        handle.init().await?;
        let mut for_queue = false;
        {
            let (wait, _stop) = handle.wait_with_timeout(limit);
            tokio::select! {
                result = wait => { result?; }
                () = wake.notified() => {}
                () = queued.notified() => for_queue = true,
            }
        }
        let session = tokio::time::timeout(COMMAND_TIMEOUT, handle.done())
            .await
            .map_err(|_| Error::Protocol("the server stopped responding".into()))??;
        Ok((Self { session, gmail, selected: None }, for_queue))
    }
}

fn folder_role(path: &str, attributes: &[NameAttribute]) -> Role {
    for attribute in attributes {
        let role = match attribute {
            NameAttribute::All => Role::All,
            NameAttribute::Archive => Role::Archive,
            NameAttribute::Junk => Role::Spam,
            NameAttribute::Trash => Role::Trash,
            NameAttribute::Sent => Role::Sent,
            NameAttribute::Drafts => Role::Drafts,
            _ => continue,
        };
        return role;
    }
    match path.to_ascii_lowercase().as_str() {
        "inbox" => Role::Inbox,
        "sent" | "sent messages" | "sent items" => Role::Sent,
        "drafts" => Role::Drafts,
        "archive" => Role::Archive,
        "trash" | "deleted messages" | "deleted items" => Role::Trash,
        "junk" | "spam" => Role::Spam,
        _ => Role::None,
    }
}

/// Folder names come from the server, so a line break must not reach a command.
fn quote(s: &str) -> Result<String> {
    if s.contains(['\r', '\n', '\0']) {
        return Err(Error::Protocol("folder name contains control characters".into()));
    }
    Ok(format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\"")))
}

const UTF7_ALPHABET: base64::alphabet::Alphabet = match base64::alphabet::Alphabet::new(
    "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+,",
) {
    Ok(a) => a,
    Err(_) => panic!("alphabet"),
};

fn utf7_engine() -> base64::engine::GeneralPurpose {
    base64::engine::GeneralPurpose::new(
        &UTF7_ALPHABET,
        base64::engine::GeneralPurposeConfig::new()
            .with_encode_padding(false)
            .with_decode_padding_mode(base64::engine::DecodePaddingMode::RequireNone),
    )
}

/// Encodes a name in IMAP's modified UTF-7 (RFC 3501 §5.1.3), as the server spells it.
pub fn encode_utf7(s: &str) -> String {
    let engine = utf7_engine();
    let mut out = String::with_capacity(s.len());
    let mut units: Vec<u8> = Vec::new();
    let flush = |units: &mut Vec<u8>, out: &mut String| {
        if !units.is_empty() {
            out.push('&');
            out.push_str(&engine.encode(&units));
            out.push('-');
            units.clear();
        }
    };
    for c in s.chars() {
        if matches!(c, ' '..='~') {
            flush(&mut units, &mut out);
            out.push(c);
            if c == '&' {
                out.push('-');
            }
        } else {
            let mut buffer = [0u16; 2];
            units.extend(c.encode_utf16(&mut buffer).iter().flat_map(|u| u.to_be_bytes()));
        }
    }
    flush(&mut units, &mut out);
    out
}

/// Decodes IMAP's modified UTF-7 (RFC 3501 §5.1.3), which folder and label names use for
/// anything outside ASCII. Text that does not decode is kept as it was.
pub fn decode_utf7(s: &str) -> String {
    let engine = utf7_engine();
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(start) = rest.find('&') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        let Some(end) = after.find('-') else {
            out.push_str(&rest[start..]);
            return out;
        };
        let encoded = &after[..end];
        if encoded.is_empty() {
            out.push('&');
        } else {
            let units: Option<Vec<u16>> = engine.decode(encoded).ok().and_then(|bytes| {
                (bytes.len() % 2 == 0)
                    .then(|| bytes.as_chunks::<2>().0.iter().map(|b| u16::from_be_bytes(*b)).collect())
            });
            match units.and_then(|u| String::from_utf16(&u).ok()) {
                Some(text) => out.push_str(&text),
                None => out.push_str(&rest[start..start + end + 2]),
            }
        }
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_modified_utf7() {
        assert_eq!(decode_utf7("INBOX"), "INBOX");
        assert_eq!(decode_utf7("Tom &- Jerry"), "Tom & Jerry");
        assert_eq!(decode_utf7("&ZeVnLIqe-"), "日本語");
        assert_eq!(decode_utf7("H&APM-a &AREBoQ-n"), "Hóa đơn");
        assert_eq!(decode_utf7("&Jjo-!"), "☺!");
    }

    #[test]
    fn encodes_modified_utf7_as_it_decodes() {
        assert_eq!(encode_utf7("INBOX"), "INBOX");
        assert_eq!(encode_utf7("Tom & Jerry"), "Tom &- Jerry");
        assert_eq!(encode_utf7("Hóa đơn"), "H&APM-a &AREBoQ-n");
        for name in ["日本語", "☺!", "a&b", "😀 x", "\\Inbox"] {
            assert_eq!(decode_utf7(&encode_utf7(name)), name);
        }
    }

    #[test]
    fn keeps_undecodable_names() {
        assert_eq!(decode_utf7("broken&"), "broken&");
        assert_eq!(decode_utf7("bad&***-x"), "bad&***-x");
    }

    #[test]
    fn quotes_names() {
        assert_eq!(quote(r#"a"b\c"#).unwrap(), r#""a\"b\\c""#);
        assert!(quote("a\r\nb LOGOUT").is_err());
    }

    #[test]
    fn roles_come_from_attributes_then_names() {
        assert_eq!(folder_role("[Gmail]/All Mail", &[NameAttribute::All]), Role::All);
        assert_eq!(folder_role("[Gmail]/Spam", &[NameAttribute::Junk]), Role::Spam);
        assert_eq!(folder_role("INBOX", &[]), Role::Inbox);
        assert_eq!(folder_role("Deleted Messages", &[]), Role::Trash);
        assert_eq!(folder_role("Projects", &[]), Role::None);
    }
}
