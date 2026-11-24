//! Inbound email (design doc §8.4, ADR-038): parsing a raw RFC 5322
//! message into what Huxley keeps, spotting automatic mail, stripping
//! quoted replies and signatures, and the `[HX#1042]` ticket token in
//! subjects. Pure functions: storage and tickets live elsewhere.

use mail_parser::{MessageParser, MessageRef};
use serde::Serialize;

/// The most text kept from a body.
pub const MAX_TEXT: usize = 100_000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Addr {
    pub address: String,
    pub name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attachment {
    pub filename: String,
    pub content_type: String,
    /// Shown in the body (e.g. a pasted image) rather than attached.
    pub inline: bool,
    pub data: Vec<u8>,
}

/// What Huxley keeps of a message.
#[derive(Debug, Clone)]
pub struct Parsed {
    /// Without angle brackets; made up from the content if missing.
    pub message_id: String,
    pub in_reply_to: Vec<String>,
    pub references: Vec<String>,
    pub from: Option<Addr>,
    pub to: Vec<Addr>,
    pub cc: Vec<Addr>,
    pub subject: String,
    /// The plain-text body (from HTML if that's all there is).
    pub text: String,
    /// Why it looks automatic (an auto-reply, a bounce, a list), if it does.
    pub automatic: Option<&'static str>,
    pub attachments: Vec<Attachment>,
}

fn addresses(list: Option<mail_parser::AddressList<'_>>) -> Vec<Addr> {
    list.map(|l| {
        l.mailboxes()
            .filter_map(|m| {
                Some(Addr {
                    address: m.address()?.trim().to_lowercase(),
                    name: m
                        .name()
                        .map(str::trim)
                        .filter(|n| !n.is_empty())
                        .map(str::to_owned),
                })
            })
            .collect()
    })
    .unwrap_or_default()
}

fn ids(list: Option<mail_parser::TextList<'_>>) -> Vec<String> {
    list.map(|l| l.iter().map(clean_id).filter(|s| !s.is_empty()).collect())
        .unwrap_or_default()
}

fn clean_id(id: &str) -> String {
    id.trim()
        .trim_start_matches('<')
        .trim_end_matches('>')
        .to_owned()
}

fn header(message: &MessageRef<'_>, name: &str) -> Option<String> {
    let h = message.headers().get(name)?;
    Some(String::from_utf8_lossy(h.raw_value()).trim().to_lowercase())
}

/// Why a message looks automatic, if it does (RFC 3834 and common
/// practice): auto-replies, bounces, lists and bulk mail.
fn automatic(message: &MessageRef<'_>, from: Option<&Addr>) -> Option<&'static str> {
    if header(message, "Auto-Submitted").is_some_and(|v| !v.starts_with("no")) {
        return Some("auto-submitted");
    }
    if header(message, "Precedence")
        .is_some_and(|v| ["bulk", "junk", "list", "auto_reply"].contains(&v.as_str()))
    {
        return Some("bulk or list mail");
    }
    for name in ["X-Autoreply", "X-Autorespond", "X-Auto-Response-Suppress"] {
        if let Some(v) = header(message, name) {
            // Outlook sets X-Auto-Response-Suppress on ordinary mail too;
            // only its "all"/"oof" values mark automatic replies.
            if name != "X-Auto-Response-Suppress" || v.contains("all") || v.contains("oof") {
                return Some("auto-reply");
            }
        }
    }
    if header(message, "List-Id").is_some() || header(message, "List-Unsubscribe").is_some() {
        return Some("bulk or list mail");
    }
    if header(message, "Return-Path").is_some_and(|v| v == "<>") {
        return Some("bounce");
    }
    if let Some(from) = from {
        let local = from.address.split('@').next().unwrap_or_default();
        if ["mailer-daemon", "postmaster"].contains(&local) {
            return Some("bounce");
        }
    }
    None
}

fn truncate(mut text: String) -> String {
    if text.len() > MAX_TEXT {
        let mut end = MAX_TEXT;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
    }
    text
}

/// Parses a raw message. `None` if it has no headers at all.
pub fn parse(raw: &[u8]) -> Option<Parsed> {
    let message = MessageParser::new().parse(raw)?;
    let root = message.root();
    let headers = root.headers();
    let from = addresses(headers.from()).into_iter().next();
    let text = match root.body_text(0) {
        Some(t) => t.replace("\r\n", "\n"),
        None => root
            .body_html(0)
            .map(|h| mail_parser::html_to_text(&h))
            .unwrap_or_default(),
    };
    let message_id = headers
        .message_id()
        .map(clean_id)
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| {
            // No Message-ID: one from the content, so the same message twice
            // is still one message.
            use std::hash::{Hash, Hasher};
            let mut h = std::collections::hash_map::DefaultHasher::new();
            raw.hash(&mut h);
            format!("{:016x}@huxley.invalid", h.finish())
        });
    let attachments = root
        .attachments()
        .map(|part| Attachment {
            filename: part
                .attachment_name()
                .map_or_else(|| "attachment".into(), str::to_owned),
            content_type: part.content_type().map_or_else(
                || "application/octet-stream".into(),
                |c| match c.subtype() {
                    Some(sub) => format!("{}/{sub}", c.ctype()),
                    None => c.ctype().to_owned(),
                },
            ),
            inline: part.is_inline(),
            data: part.decoded().into_owned(),
        })
        .collect();
    Some(Parsed {
        automatic: automatic(&root, from.as_ref()),
        message_id,
        in_reply_to: ids(headers.in_reply_to()),
        references: ids(headers.references()),
        to: addresses(headers.to()),
        cc: addresses(headers.cc()),
        from,
        subject: headers.subject().unwrap_or_default().trim().to_owned(),
        text: truncate(text),
        attachments,
    })
}

/// The ticket number in a subject's `[HX#1042]` token, if any.
pub fn ticket_token(subject: &str) -> Option<i64> {
    let start = subject.find("[HX#")? + 4;
    let rest = &subject[start..];
    let end = rest.find(']')?;
    rest[..end].trim().parse().ok()
}

/// A subject carrying the ticket's token (once).
pub fn with_token(subject: &str, number: i64) -> String {
    if ticket_token(subject) == Some(number) {
        subject.to_owned()
    } else {
        format!("{subject} [HX#{number}]")
    }
}

/// The subject without reply and forward prefixes (`Re:`, `Fwd:`, `AW:` …)
/// or a ticket token.
pub fn clean_subject(subject: &str) -> String {
    let mut s = subject.trim();
    if let (Some(start), Some(n)) = (s.find("[HX#"), ticket_token(s)) {
        let token = format!("[HX#{n}]");
        let owned = format!("{}{}", &s[..start], &s[start + token.len()..]);
        return clean_subject(&owned);
    }
    loop {
        let lower = s.to_lowercase();
        let Some(prefix) = ["re:", "fw:", "fwd:", "aw:", "wg:", "sv:", "vs:", "tr:"]
            .iter()
            .find(|p| lower.starts_with(**p))
        else {
            break;
        };
        s = s[prefix.len()..].trim_start();
    }
    s.trim().to_owned()
}

/// The new part of a reply: everything before the quoted message or the
/// signature.
pub fn strip_reply(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let mut end = lines.len();
    for (i, line) in lines.iter().enumerate() {
        let t = line.trim();
        let quoted_header = (t.starts_with("On ") && t.ends_with("wrote:"))
            || (t.starts_with("Am ") && t.ends_with("schrieb:"))
            || (t.starts_with("Le ") && t.ends_with("a écrit :"));
        let outlook = t.starts_with("-----Original Message-----")
            || (t.starts_with("From:")
                && lines.get(i + 1).is_some_and(|n| {
                    let n = n.trim();
                    n.starts_with("Sent:") || n.starts_with("Date:")
                }))
            || t.starts_with("________________________________");
        let signature = *line == "-- " || t == "--";
        let quote = t.starts_with('>');
        if quoted_header
            || outlook
            || signature
            || (quote
                && lines[i..].iter().all(|l| {
                    let l = l.trim();
                    l.is_empty() || l.starts_with('>')
                }))
        {
            end = i;
            break;
        }
    }
    let kept = lines[..end].join("\n");
    let kept = kept.trim();
    if kept.is_empty() {
        text.trim().to_owned()
    } else {
        kept.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PLAIN: &[u8] = b"From: Ada Lovelace <Ada@Acme.Example>\r\n\
To: help@msp.example, \"Desk\" <desk@msp.example>\r\n\
Subject: Re: Printer on fire [HX#1042]\r\n\
Message-ID: <abc@acme.example>\r\n\
In-Reply-To: <out-1@msp.example>\r\n\
References: <first@acme.example> <out-1@msp.example>\r\n\
\r\n\
It's still burning.\r\n\
\r\n\
On Tue, 24 Nov 2026 at 10:00, Desk <desk@msp.example> wrote:\r\n\
> Have you tried turning it off?\r\n";

    #[test]
    fn parses_what_we_keep() {
        let p = parse(PLAIN).unwrap();
        assert_eq!(p.message_id, "abc@acme.example");
        assert_eq!(p.in_reply_to, ["out-1@msp.example"]);
        assert_eq!(p.references, ["first@acme.example", "out-1@msp.example"]);
        assert_eq!(
            p.from,
            Some(Addr {
                address: "ada@acme.example".into(),
                name: Some("Ada Lovelace".into())
            })
        );
        assert_eq!(p.to.len(), 2);
        assert_eq!(p.to[1].name.as_deref(), Some("Desk"));
        assert_eq!(p.subject, "Re: Printer on fire [HX#1042]");
        assert!(p.automatic.is_none());
        assert_eq!(strip_reply(&p.text), "It's still burning.");
    }

    #[test]
    fn spots_automatic_mail() {
        let with = |header: &str| {
            let raw = format!("From: a@b.example\r\n{header}\r\nSubject: x\r\n\r\nbody\r\n");
            parse(raw.as_bytes()).unwrap().automatic
        };
        assert_eq!(with("Auto-Submitted: auto-replied"), Some("auto-submitted"));
        assert_eq!(with("Auto-Submitted: no"), None);
        assert_eq!(with("Precedence: bulk"), Some("bulk or list mail"));
        assert_eq!(with("X-Autoreply: yes"), Some("auto-reply"));
        assert_eq!(with("X-Auto-Response-Suppress: All"), Some("auto-reply"));
        assert_eq!(with("X-Auto-Response-Suppress: RN, NRN"), None);
        assert_eq!(with("List-Id: <news.example>"), Some("bulk or list mail"));
        assert_eq!(with("Return-Path: <>"), Some("bounce"));
        let daemon =
            parse(b"From: MAILER-DAEMON@mx.example\r\nSubject: x\r\n\r\nfailed\r\n").unwrap();
        assert_eq!(daemon.automatic, Some("bounce"));
    }

    #[test]
    fn html_only_and_attachments() {
        let raw = b"From: b@b.example\r\nSubject: Logs\r\nMIME-Version: 1.0\r\n\
Content-Type: multipart/mixed; boundary=XX\r\n\r\n\
--XX\r\nContent-Type: text/html\r\n\r\n<p>See <b>attached</b></p>\r\n\
--XX\r\nContent-Type: text/plain; name=log.txt\r\nContent-Disposition: attachment; filename=log.txt\r\n\
Content-Transfer-Encoding: base64\r\n\r\naGVsbG8=\r\n--XX--\r\n";
        let p = parse(raw).unwrap();
        assert!(p.text.contains("See attached"), "{:?}", p.text);
        assert_eq!(p.attachments.len(), 1);
        assert_eq!(p.attachments[0].filename, "log.txt");
        assert_eq!(p.attachments[0].content_type, "text/plain");
        assert_eq!(p.attachments[0].data, b"hello");
        assert!(!p.attachments[0].inline);
        // No Message-ID: a stable one from the content.
        assert_eq!(parse(raw).unwrap().message_id, p.message_id);
        assert!(p.message_id.ends_with("@huxley.invalid"));
    }

    #[test]
    fn tokens_and_subjects() {
        assert_eq!(ticket_token("Re: Printer [HX#1042]"), Some(1042));
        assert_eq!(ticket_token("[HX# 7 ] hi"), Some(7));
        assert_eq!(ticket_token("[HX#abc]"), None);
        assert_eq!(ticket_token("no token"), None);
        assert_eq!(with_token("Printer", 1042), "Printer [HX#1042]");
        assert_eq!(with_token("Printer [HX#1042]", 1042), "Printer [HX#1042]");
        assert_eq!(clean_subject("RE: Fwd: AW: Printer [HX#1042]"), "Printer");
        assert_eq!(clean_subject("  Re:Re: Hello "), "Hello");
    }

    #[test]
    fn strips_replies_and_signatures() {
        assert_eq!(strip_reply("Thanks!\n-- \nAda\nAcme Ltd"), "Thanks!");
        assert_eq!(
            strip_reply("Done.\n\nFrom: Desk <desk@msp.example>\nSent: Tuesday\nSubject: x\n\nold"),
            "Done."
        );
        assert_eq!(strip_reply("See below\n> quoted\n> more\n"), "See below");
        // Interleaved answers keep their quotes.
        assert_eq!(
            strip_reply("> q1\nanswer 1\n> q2\nanswer 2"),
            "> q1\nanswer 1\n> q2\nanswer 2"
        );
        // Nothing but a quote: keep it all rather than nothing.
        assert_eq!(strip_reply("> only quote"), "> only quote");
    }
}
