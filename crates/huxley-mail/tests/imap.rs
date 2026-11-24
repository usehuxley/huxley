//! Fetching from a real IMAP server. Set `HUXLEY_TEST_IMAP` to a test
//! server's `host:imap_port:smtp_port`, one that makes an account on
//! delivery with the address as its password (as `GreenMail` does);
//! skipped otherwise, unless `HUXLEY_REQUIRE_IMAP_TESTS` is set.

use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::time::Duration;

use huxley_mail::imap::{self, Config, Cursor, Security};

fn server() -> Option<(String, u16, u16)> {
    let Ok(spec) = std::env::var("HUXLEY_TEST_IMAP") else {
        assert!(
            std::env::var("HUXLEY_REQUIRE_IMAP_TESTS").is_err(),
            "HUXLEY_REQUIRE_IMAP_TESTS is set but HUXLEY_TEST_IMAP isn't"
        );
        return None;
    };
    let mut parts = spec.split(':');
    let host = parts.next()?.to_owned();
    Some((
        host,
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
    ))
}

/// Delivers a message over SMTP.
fn send(host: &str, port: u16, to: &str, id: &str, subject: &str) {
    let stream = TcpStream::connect((host, port)).unwrap();
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut writer = stream;
    let mut line = String::new();
    let mut expect = |reader: &mut BufReader<TcpStream>| {
        line.clear();
        reader.read_line(&mut line).unwrap();
        while line.len() > 3 && &line[3..4] == "-" {
            line.clear();
            reader.read_line(&mut line).unwrap();
        }
        assert!(line.starts_with('2') || line.starts_with('3'), "{line}");
    };
    expect(&mut reader);
    for command in [
        "EHLO test",
        "MAIL FROM:<ada@acme.example>",
        &format!("RCPT TO:<{to}>"),
        "DATA",
    ] {
        writer
            .write_all(format!("{command}\r\n").as_bytes())
            .unwrap();
        expect(&mut reader);
    }
    let body = format!(
        "From: Ada <ada@acme.example>\r\nTo: {to}\r\nSubject: {subject}\r\nMessage-ID: <{id}>\r\n\r\nHello\r\n.\r\n"
    );
    writer.write_all(body.as_bytes()).unwrap();
    expect(&mut reader);
    writer.write_all(b"QUIT\r\n").unwrap();
}

/// A fresh account (made by delivering to it).
fn account(host: &str, smtp_port: u16) -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let address = format!("m{}.{nanos}@desk.example", std::process::id());
    send(
        host,
        smtp_port,
        &address,
        &format!("setup.{address}"),
        "Setup",
    );
    address
}

fn config(host: &str, port: u16, user: &str) -> Config {
    Config {
        host: host.to_owned(),
        port,
        security: Security::None,
        username: user.into(),
        password: user.into(),
        folder: "INBOX".into(),
    }
}

#[tokio::test]
async fn fetches_new_messages_once() {
    let Some((host, imap_port, smtp_port)) = server() else {
        return;
    };
    let user = account(&host, smtp_port);
    let config = config(&host, imap_port, &user);
    let timeout = Duration::from_secs(20);
    // The first fetch takes what's unread: the setup message.
    let (setup, cursor) = imap::fetch(&config, Cursor::default(), 500, timeout)
        .await
        .unwrap();
    assert_eq!(setup.len(), 1);
    assert!(cursor.uid_validity.is_some());

    let tag = std::process::id();
    for n in 0..3 {
        send(
            &host,
            smtp_port,
            &user,
            &format!("{tag}.{n}@acme.example"),
            &format!("Hello {n}"),
        );
    }
    // Two at a time, oldest first, then the rest; then nothing.
    let (first, cursor) = imap::fetch(&config, cursor, 2, timeout).await.unwrap();
    assert_eq!(first.len(), 2);
    assert!(String::from_utf8_lossy(&first[0].raw).contains("Subject: Hello 0"));
    let (rest, cursor) = imap::fetch(&config, cursor, 2, timeout).await.unwrap();
    assert_eq!(rest.len(), 1);
    assert!(String::from_utf8_lossy(&rest[0].raw).contains("Subject: Hello 2"));
    assert!(rest[0].uid > first[1].uid);
    let (none, again) = imap::fetch(&config, cursor, 2, timeout).await.unwrap();
    assert!(none.is_empty());
    assert_eq!(again, cursor);

    // A cursor from another folder generation starts over with what's unread: nothing.
    let stale = Cursor {
        uid_validity: Some(1),
        last_uid: 0,
    };
    let (unread, _) = imap::fetch(&config, stale, 50, timeout).await.unwrap();
    assert!(unread.is_empty(), "fetched messages are marked seen");
}

#[tokio::test]
async fn reports_bad_logins_and_missing_servers() {
    let Some((host, imap_port, smtp_port)) = server() else {
        return;
    };
    let user = account(&host, smtp_port);
    let mut wrong = config(&host, imap_port, &user);
    wrong.password = "nope".into();
    let error = imap::fetch(&wrong, Cursor::default(), 10, Duration::from_secs(10))
        .await
        .unwrap_err();
    assert!(matches!(error, imap::ImapError::Login), "{error}");
    let missing = config(&host, 1, &user);
    let error = imap::fetch(&missing, Cursor::default(), 10, Duration::from_secs(10))
        .await
        .unwrap_err();
    assert!(matches!(error, imap::ImapError::Connect(..)), "{error}");
}
