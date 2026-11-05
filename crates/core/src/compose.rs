//! What a new message starts from, and how a finished one becomes bytes. Plain functions over raw
//! mail, so all of it can be tested without a store or a server.

use std::collections::HashSet;
use std::str::FromStr;
use std::time::SystemTime;

use lettre::message::header::{ContentType, HeaderName, HeaderValue};
use lettre::message::{Attachment as Part, Mailbox, Mailboxes, MessageBuilder, MultiPart, SinglePart};
use lettre::address::Envelope;
use mail_parser::{MessageParser, MessageRef};

/// A file on a message, whole.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attachment {
    pub name: String,
    pub mime: String,
    pub data: Vec<u8>,
}

/// The fields of a message being written.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Compose {
    pub to: String,
    pub cc: String,
    pub bcc: String,
    pub subject: String,
    pub body: String,
    /// The Message-ID being answered, without brackets, or empty.
    pub in_reply_to: String,
    /// Message-IDs of the thread so far, without brackets, separated by spaces.
    pub references: String,
}

impl Compose {
    /// True if there is nothing here worth keeping.
    pub fn is_blank(&self) -> bool {
        [&self.to, &self.cc, &self.bcc, &self.subject, &self.body].iter().all(|f| f.trim().is_empty())
    }
}

fn addresses(list: Option<mail_parser::AddressList>) -> Vec<(String, String)> {
    let Some(list) = list else { return Vec::new() };
    list.mailboxes()
        .filter_map(|m| Some((m.name().unwrap_or_default().to_string(), m.address()?.to_string())))
        .collect()
}

fn format_address((name, address): &(String, String)) -> String {
    if name.is_empty() {
        address.clone()
    } else if name.contains([',', ';', '"', '<', '>', '@', '(', ')']) {
        format!("\"{}\" <{address}>", name.replace('\\', "\\\\").replace('"', "\\\""))
    } else {
        format!("{name} <{address}>")
    }
}

fn join(list: &[(String, String)]) -> String {
    list.iter().map(format_address).collect::<Vec<_>>().join(", ")
}

fn with_prefix(prefix: &str, subject: &str) -> String {
    let already = subject.get(..prefix.len()).is_some_and(|start| start.eq_ignore_ascii_case(prefix));
    if already { subject.to_string() } else { format!("{prefix} {subject}") }
}

fn body_of(message: MessageRef) -> String {
    message.body_text(0).map_or_else(String::new, |t| t.replace("\r\n", "\n"))
}

fn sender_name(message: MessageRef) -> String {
    addresses(message.from()).first().map_or_else(String::new, |(name, address)| {
        if name.is_empty() { address.clone() } else { name.clone() }
    })
}

/// The date as the sender wrote it, which is the one the quoted mail itself shows.
fn date_line(message: MessageRef) -> String {
    message.date().map_or_else(String::new, |d| d.to_rfc822())
}

/// A reply to a raw message. `own` is the sending address, kept off the recipients. With `all`,
/// everyone who got the original gets the answer. `None` if the bytes are not a message.
pub fn reply(raw: &[u8], own: &str, all: bool) -> Option<Compose> {
    let parsed = MessageParser::default().parse(raw)?;
    let message = parsed.root();
    let is_own = |a: &(String, String)| a.1.eq_ignore_ascii_case(own);
    let from = addresses(message.from());
    let reply_to = addresses(message.reply_to());
    let original_to = addresses(message.to());
    // Answering your own mail continues it, so it goes to the same people rather than to you.
    let mut to = if from.iter().any(is_own) {
        original_to.clone()
    } else if reply_to.is_empty() {
        from
    } else {
        reply_to
    };
    let mut cc = Vec::new();
    if all {
        cc.extend(original_to);
        cc.extend(addresses(message.cc()));
    }
    let mut seen = HashSet::new();
    to.retain(|a| seen.insert(a.1.to_lowercase()));
    cc.retain(|a| !is_own(a) && seen.insert(a.1.to_lowercase()));
    let mid = message.message_id().unwrap_or_default().to_string();
    let mut references: Vec<&str> = message.references().map_or_else(Vec::new, |r| r.iter().collect());
    if references.is_empty()
        && let Some(parent) = message.in_reply_to().and_then(|r| r.first())
    {
        references.push(parent);
    }
    if !mid.is_empty() {
        references.push(&mid);
    }
    let quoted: String = body_of(message).trim_end().lines().map(|l| if l.is_empty() { ">\n".to_string() } else { format!("> {l}\n") }).collect();
    Some(Compose {
        to: join(&to),
        cc: join(&cc),
        bcc: String::new(),
        subject: with_prefix("Re:", message.subject().unwrap_or_default()),
        body: format!("\n\nOn {}, {} wrote:\n{quoted}", date_line(message), sender_name(message)),
        references: references.join(" "),
        in_reply_to: mid,
    })
}

/// A message's real attachments: files, rather than images its own HTML shows inline.
pub fn attachments(raw: &[u8]) -> Vec<Attachment> {
    let Some(parsed) = MessageParser::default().parse(raw) else { return Vec::new() };
    let message = parsed.root();
    message
        .attachments()
        .filter(|p| !(p.is_inline() && p.content_id().is_some()))
        .map(|p| Attachment {
            name: p.attachment_name().unwrap_or("Attachment").to_string(),
            mime: p.content_type().map_or_else(
                || "application/octet-stream".to_string(),
                |t| format!("{}/{}", t.ctype(), t.subtype().unwrap_or("octet-stream")),
            ),
            data: p.decoded().into_owned(),
        })
        .collect()
}

/// A forward of a raw message. Its attachments come from [`attachments`].
pub fn forward(raw: &[u8]) -> Option<Compose> {
    let parsed = MessageParser::default().parse(raw)?;
    let message = parsed.root();
    let original = body_of(message);
    let fields = [
        ("From", join(&addresses(message.from()))),
        ("Date", date_line(message)),
        ("Subject", message.subject().unwrap_or_default().to_string()),
        ("To", join(&addresses(message.to()))),
    ];
    let lines: String = fields.iter().filter(|(_, v)| !v.is_empty()).map(|(k, v)| format!("{k}: {v}\n")).collect();
    let header = format!("---------- Forwarded message ----------\n{lines}");
    Some(Compose {
        subject: with_prefix("Fwd:", message.subject().unwrap_or_default()),
        body: format!("\n\n{header}\n{}", original.trim_end()),
        ..Compose::default()
    })
}

/// A saved draft as fields again, for a draft written in another client.
pub fn from_raw(raw: &[u8]) -> Option<(Compose, Option<String>)> {
    let parsed = MessageParser::default().parse(raw)?;
    let message = parsed.root();
    let compose = Compose {
        to: join(&addresses(message.to())),
        cc: join(&addresses(message.cc())),
        bcc: join(&addresses(message.bcc())),
        subject: message.subject().unwrap_or_default().to_string(),
        body: body_of(message),
        in_reply_to: message.in_reply_to().and_then(|r| r.first().map(str::to_string)).unwrap_or_default(),
        references: message.references().map_or_else(String::new, |r| r.iter().collect::<Vec<_>>().join(" ")),
    };
    Some((compose, message.message_id().map(str::to_string)))
}

/// Reads a recipients field. Commas and semicolons both separate, and a stray one is forgiven.
pub fn parse_recipients(field: &str) -> Result<Vec<Mailbox>, String> {
    let field = field.replace(';', ",");
    let trimmed = field.trim().trim_matches(',').trim();
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }
    Mailboxes::from_str(trimmed).map(|m| m.into_iter().collect()).map_err(|_| {
        let bad = trimmed.split(',').find(|part| Mailbox::from_str(part.trim()).is_err());
        bad.unwrap_or(trimmed).trim().to_string()
    })
}

/// Says what stops a message going, in the user's terms, or `None` if nothing does.
pub fn problem(compose: &Compose) -> Option<String> {
    let mut any = false;
    for field in [&compose.to, &compose.cc, &compose.bcc] {
        match parse_recipients(field) {
            Ok(list) => any |= !list.is_empty(),
            Err(bad) => return Some(format!("“{bad}” isn't an email address")),
        }
    }
    (!any).then(|| "Add someone to send this to".to_string())
}

fn angle(mid: &str) -> String {
    format!("<{mid}>")
}

fn header(name: &'static str, value: &str) -> HeaderValue {
    HeaderValue::new(HeaderName::new_from_ascii_str(name), value.to_string())
}

fn base(compose: &Compose, from: &Mailbox, mid: &str) -> MessageBuilder {
    let mut builder = lettre::Message::builder()
        .from(from.clone())
        .subject(compose.subject.clone())
        .date(SystemTime::now())
        .message_id(Some(angle(mid)));
    if !compose.in_reply_to.is_empty() {
        builder = builder.in_reply_to(angle(&compose.in_reply_to));
    }
    if !compose.references.is_empty() {
        let references: Vec<String> = compose.references.split_whitespace().map(angle).collect();
        builder = builder.references(references.join(" "));
    }
    builder
}

fn finish(builder: MessageBuilder, compose: &Compose, files: &[Attachment]) -> Result<lettre::Message, String> {
    let text = SinglePart::plain(compose.body.clone());
    let built = if files.is_empty() {
        builder.singlepart(text)
    } else {
        let mut parts = MultiPart::mixed().singlepart(text);
        for file in files {
            let kind = ContentType::parse(&file.mime).unwrap_or(ContentType::parse("application/octet-stream").expect("valid"));
            parts = parts.singlepart(Part::new(file.name.clone()).body(file.data.clone(), kind));
        }
        builder.multipart(parts)
    };
    built.map_err(|e| e.to_string())
}

/// The bytes of a message to send, and who gets it. Bcc is in the envelope and nowhere else.
pub fn outgoing(compose: &Compose, from: &Mailbox, mid: &str, files: &[Attachment]) -> Result<(Envelope, Vec<u8>), String> {
    let mut builder = base(compose, from, mid);
    for mailbox in parse_recipients(&compose.to)? {
        builder = builder.to(mailbox);
    }
    for mailbox in parse_recipients(&compose.cc)? {
        builder = builder.cc(mailbox);
    }
    for mailbox in parse_recipients(&compose.bcc)? {
        builder = builder.bcc(mailbox);
    }
    // With no To at all, lettre would refuse a message that has only Cc or Bcc.
    let everyone: Vec<_> = [&compose.to, &compose.cc, &compose.bcc]
        .into_iter()
        .flat_map(|f| parse_recipients(f).unwrap_or_default())
        .map(|m| m.email)
        .collect();
    let envelope = Envelope::new(Some(from.email.clone()), everyone).map_err(|e| e.to_string())?;
    let message = finish(builder.envelope(envelope.clone()), compose, files)?;
    Ok((envelope, message.formatted()))
}

/// The bytes of a draft. A draft may be half written, so its address fields go in as they stand,
/// and Bcc stays in because this copy is only for its author.
pub fn draft_bytes(compose: &Compose, from: &Mailbox, mid: &str, files: &[Attachment]) -> Result<Vec<u8>, String> {
    let mut builder = base(compose, from, mid);
    for (name, value) in [("To", &compose.to), ("Cc", &compose.cc), ("Bcc", &compose.bcc)] {
        if !value.trim().is_empty() {
            builder = builder.raw_header(header(name, value));
        }
    }
    // Nothing will ever use this envelope, but lettre wants one before it will build.
    let envelope = Envelope::new(Some(from.email.clone()), vec![from.email.clone()]).map_err(|e| e.to_string())?;
    Ok(finish(builder.envelope(envelope).keep_bcc(), compose, files)?.formatted())
}

#[cfg(test)]
mod tests {
    use super::*;

    const ORIGINAL: &[u8] = "From: =?UTF-8?Q?Nguy=E1=BB=85n_An?= <an@example.com>\r\n\
To: me@example.com, Bình <binh@example.com>\r\nCc: chi@example.com, ME@example.com\r\n\
Subject: Lịch họp\r\nMessage-ID: <m2@x>\r\nReferences: <m0@x> <m1@x>\r\nIn-Reply-To: <m1@x>\r\n\
Date: Wed, 04 Nov 2026 10:00:00 +0700\r\nContent-Type: text/plain; charset=utf-8\r\n\r\nDòng một\r\n\r\nDòng hai\r\n"
        .as_bytes();

    fn me() -> Mailbox {
        "Me <me@example.com>".parse().unwrap()
    }

    #[test]
    fn a_reply_goes_to_the_sender_and_quotes_the_message() {
        let r = reply(ORIGINAL, "me@example.com", false).unwrap();
        assert_eq!(r.to, "Nguyễn An <an@example.com>");
        assert_eq!(r.cc, "");
        assert_eq!(r.subject, "Re: Lịch họp");
        assert_eq!(r.in_reply_to, "m2@x");
        assert_eq!(r.references, "m0@x m1@x m2@x");
        assert!(r.body.starts_with("\n\nOn "), "{}", r.body);
        assert!(r.body.contains("Nguyễn An wrote:\n> Dòng một\n>\n> Dòng hai\n"), "{}", r.body);
    }

    #[test]
    fn reply_all_keeps_everyone_but_you_once_each() {
        let r = reply(ORIGINAL, "me@example.com", true).unwrap();
        assert_eq!(r.to, "Nguyễn An <an@example.com>");
        assert_eq!(r.cc, "Bình <binh@example.com>, chi@example.com");
    }

    #[test]
    fn reply_to_wins_and_no_re_is_doubled() {
        let raw = b"From: a@x.example\r\nReply-To: list@x.example\r\nSubject: RE: hello\r\n\r\nhi\r\n";
        let r = reply(raw, "me@example.com", false).unwrap();
        assert_eq!(r.to, "list@x.example");
        assert_eq!(r.subject, "RE: hello");
    }

    #[test]
    fn answering_your_own_mail_goes_to_its_recipients() {
        let raw = b"From: me@example.com\r\nTo: a@x.example\r\nSubject: s\r\n\r\nhi\r\n";
        assert_eq!(reply(raw, "ME@example.com", false).unwrap().to, "a@x.example");
    }

    #[test]
    fn a_forward_carries_the_header_block_and_no_recipients() {
        let f = forward(ORIGINAL).unwrap();
        assert_eq!(f.subject, "Fwd: Lịch họp");
        assert_eq!(f.to, "");
        assert!(f.body.contains("From: Nguyễn An <an@example.com>\nDate: "), "{}", f.body);
        assert!(f.body.contains("Dòng hai"));
    }

    #[test]
    fn a_forward_leaves_out_what_the_original_did_not_have() {
        let f = forward(b"From: a@x.example\r\nSubject: s\r\n\r\nhi\r\n").unwrap();
        assert!(!f.body.contains("Date:") && !f.body.contains("To:"), "{}", f.body);
    }

    #[test]
    fn names_with_commas_survive_a_round_trip() {
        let raw = b"From: \"Doe, Jane\" <jane@x.example>\r\nSubject: s\r\n\r\nhi\r\n";
        let to = reply(raw, "me@example.com", false).unwrap().to;
        assert_eq!(to, "\"Doe, Jane\" <jane@x.example>");
        assert_eq!(parse_recipients(&to).unwrap().len(), 1);
    }

    #[test]
    fn recipients_forgive_stray_separators_and_name_the_bad_one() {
        assert_eq!(parse_recipients(" a@x.example; b@x.example, ").unwrap().len(), 2);
        assert!(parse_recipients("  ").unwrap().is_empty());
        assert_eq!(parse_recipients("a@x.example, nobody").unwrap_err(), "nobody");
    }

    #[test]
    fn a_message_needs_someone_to_go_to() {
        let mut c = Compose { subject: "s".into(), ..Compose::default() };
        assert!(problem(&c).unwrap().contains("Add someone"));
        c.bcc = "a@x.example".into();
        assert_eq!(problem(&c), None);
        c.to = "not an address".into();
        assert!(problem(&c).unwrap().contains("not an address"));
    }

    #[test]
    fn bcc_goes_in_the_envelope_and_never_in_the_bytes() {
        let c = Compose { to: "a@x.example".into(), bcc: "secret@x.example".into(), subject: "s".into(), body: "b".into(), ..Compose::default() };
        let (envelope, raw) = outgoing(&c, &me(), "id@x", &[]).unwrap();
        assert_eq!(envelope.to().len(), 2);
        assert!(!String::from_utf8_lossy(&raw).contains("secret"));
    }

    #[test]
    fn a_message_with_only_bcc_can_go() {
        let c = Compose { bcc: "a@x.example".into(), subject: "s".into(), ..Compose::default() };
        let (envelope, _) = outgoing(&c, &me(), "id@x", &[]).unwrap();
        assert_eq!(envelope.to().len(), 1);
    }

    #[test]
    fn what_is_sent_parses_back_with_unicode_threading_and_files() {
        let c = Compose {
            to: "Bình <binh@x.example>".into(),
            subject: "Hóa đơn tháng 10".into(),
            body: "Chào anh\nDòng hai".into(),
            in_reply_to: "m2@x".into(),
            references: "m1@x m2@x".into(),
            ..Compose::default()
        };
        let file = Attachment { name: "hóa-đơn.pdf".into(), mime: "application/pdf".into(), data: vec![0, 159, 146, 150] };
        let (_, raw) = outgoing(&c, &me(), "new@x", std::slice::from_ref(&file)).unwrap();
        let parsed = MessageParser::default().parse(&raw).unwrap();
        let m = parsed.root();
        assert_eq!(m.subject(), Some("Hóa đơn tháng 10"));
        assert_eq!(m.message_id(), Some("new@x"));
        assert_eq!(m.in_reply_to().unwrap().first(), Some("m2@x"));
        assert_eq!(m.references().unwrap().iter().collect::<Vec<_>>(), ["m1@x", "m2@x"]);
        assert_eq!(m.to().unwrap().mailboxes().next().unwrap().name(), Some("Bình"));
        assert!(m.body_text(0).unwrap().contains("Dòng hai"));
        assert_eq!(attachments(&raw), [file]);
    }

    #[test]
    fn a_half_written_draft_still_saves_and_reads_back() {
        let c = Compose { to: "bình, not yet".into(), bcc: "b@x.example".into(), subject: "s".into(), body: "text".into(), ..Compose::default() };
        let raw = draft_bytes(&c, &me(), "d1@x", &[]).unwrap();
        let text = String::from_utf8_lossy(&raw);
        assert!(text.contains("Bcc: b@x.example"), "{text}");
        let parsed = MessageParser::default().parse(&raw).unwrap();
        assert_eq!(parsed.root().message_id(), Some("d1@x"));
        assert!(parsed.root().body_text(0).unwrap().contains("text"));
    }

    #[test]
    fn inline_images_are_not_attachments() {
        let raw = b"From: a@x.example\r\nContent-Type: multipart/mixed; boundary=b\r\n\r\n\
--b\r\nContent-Type: text/html\r\n\r\n<img src=cid:i>\r\n\
--b\r\nContent-Type: image/png\r\nContent-ID: <i>\r\nContent-Disposition: inline\r\n\r\nAA\r\n\
--b\r\nContent-Type: application/pdf\r\nContent-Disposition: attachment; filename=r.pdf\r\n\r\nPDF\r\n--b--\r\n";
        let files = attachments(raw);
        assert_eq!(files.len(), 1);
        assert_eq!((files[0].name.as_str(), files[0].mime.as_str()), ("r.pdf", "application/pdf"));
    }
}
