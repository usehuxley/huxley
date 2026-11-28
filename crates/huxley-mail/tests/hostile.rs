//! Hostile email: mailboxes receive whatever anyone sends. Parsing must
//! never panic or hang.

use std::fmt::Write as _;

use proptest::prelude::*;

#[test]
fn deeply_nested_multiparts_parse_quickly() {
    let mut msg = String::from("From: a@example.com\r\nSubject: nest\r\n");
    let depth = 2_000;
    for i in 0..depth {
        let _ = write!(
            msg,
            "Content-Type: multipart/mixed; boundary=\"b{i}\"\r\n\r\n--b{i}\r\n"
        );
    }
    msg.push_str("Content-Type: text/plain\r\n\r\nhello\r\n");
    let started = std::time::Instant::now();
    let handle = std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(move || {
            let _ = huxley_mail::parse(msg.as_bytes());
        })
        .unwrap();
    handle.join().expect("parsing panicked");
    assert!(started.elapsed() < std::time::Duration::from_secs(5));
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    #[test]
    fn any_bytes_parse_or_fail_cleanly(bytes in proptest::collection::vec(any::<u8>(), 0..2048)) {
        let _ = huxley_mail::parse(&bytes);
    }

    /// Header-shaped noise: real header names, random values and encodings.
    #[test]
    fn header_shaped_messages_parse_or_fail_cleanly(
        headers in proptest::collection::vec(
            ("(From|To|Cc|Subject|Message-ID|In-Reply-To|References|Content-Type|Content-Transfer-Encoding)",
             "[ -~]{0,80}"),
            0..12,
        ),
        body in "[ -~\r\n=]{0,300}",
    ) {
        let mut msg = String::new();
        for (name, value) in headers {
            let _ = writeln!(msg, "{name}: {value}\r");
        }
        msg.push_str("\r\n");
        msg.push_str(&body);
        let _ = huxley_mail::parse(msg.as_bytes());
    }
}
