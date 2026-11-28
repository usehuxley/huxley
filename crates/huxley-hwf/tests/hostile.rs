//! Hostile workflow documents: the shapes attackers send to parsers. Each
//! must fail quickly and cleanly, never hang, exhaust memory or overflow
//! the stack.

use std::fmt::Write as _;

use std::time::{Duration, Instant};

/// Runs `f` on a thread with a generous stack budget check: it must finish
/// within `limit`.
fn within<T: Send + 'static>(limit: Duration, f: impl FnOnce() -> T + Send + 'static) -> T {
    let started = Instant::now();
    let handle = std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(f)
        .unwrap();
    let out = handle.join().expect("the parser panicked");
    assert!(started.elapsed() < limit, "took {:?}", started.elapsed());
    out
}

/// "Billion laughs": nine levels of ten aliases each, a billion nodes from a
/// few hundred bytes.
fn billion_laughs() -> String {
    let mut doc = String::from("a0: &a0 [\"lol\"]\n");
    for i in 1..=9 {
        let prev = i - 1;
        let refs = vec![format!("*a{prev}"); 10].join(", ");
        let _ = writeln!(doc, "a{i}: &a{i} [{refs}]");
    }
    doc
}

#[test]
fn alias_bombs_are_refused_quickly() {
    let doc = billion_laughs();
    let result = within(Duration::from_secs(5), move || huxley_hwf::parse_yaml(&doc));
    assert!(result.is_err());
}

#[test]
fn deep_nesting_doesnt_overflow_the_stack() {
    for doc in [
        "[".repeat(200_000),
        "{a: ".repeat(100_000),
        format!("x: {}", "- ".repeat(50_000)),
    ] {
        let result = within(Duration::from_secs(5), move || huxley_hwf::parse_yaml(&doc));
        assert!(result.is_err());
    }
    let json = "[".repeat(200_000);
    let result = within(Duration::from_secs(5), move || {
        huxley_hwf::parse_json(&json)
    });
    assert!(result.is_err());
}

mod random {
    use proptest::prelude::*;

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(512))]

        /// Whatever arrives, the parsers answer with a value or an error.
        #[test]
        fn any_text_parses_or_fails_cleanly(text in "\\PC{0,400}") {
            let _ = huxley_hwf::parse_yaml(&text);
            let _ = huxley_hwf::parse_json(&text);
        }

        /// YAML-shaped noise reaches deeper into the deserializer.
        #[test]
        fn yaml_shaped_input_parses_or_fails_cleanly(
            lines in proptest::collection::vec("[ ]{0,6}(- )?[a-z&*!|>{}\\[\\]:,'\"0-9 -]{0,30}", 0..40)
        ) {
            let _ = huxley_hwf::parse_yaml(&lines.join("\n"));
        }

        /// Durations and cron expressions come from users.
        #[test]
        fn durations_and_cron_parse_or_fail_cleanly(text in "\\PC{0,60}") {
            let _ = huxley_hwf::duration::parse(&text);
            let _ = huxley_hwf::cron::Cron::parse(&text);
        }
    }
}
