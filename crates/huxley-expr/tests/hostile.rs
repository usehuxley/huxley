//! Hostile expressions: workflow authors write them, and some inputs come
//! from outside (webhooks, email). Evaluation must stay within its budget
//! of time, memory and depth, and fail cleanly beyond it.

use std::time::{Duration, Instant};

use huxley_expr::{Engine, Scope};
use proptest::prelude::*;

fn eval(text: &str) -> bool {
    let engine = Engine::new();
    engine.eval_str(text, &Scope::new()).is_ok()
}

fn within(limit: Duration, text: &'static str) -> bool {
    let started = Instant::now();
    let handle = std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(move || eval(text))
        .unwrap();
    let ok = handle.join().expect("evaluation panicked");
    assert!(
        started.elapsed() < limit,
        "{text}: took {:?}",
        started.elapsed()
    );
    ok
}

#[test]
fn huge_strings_and_ranges_are_refused() {
    for text in [
        "{{ 'x' * 1000000000 }}",
        "{{ ('x' * 100000) * 100000 }}",
        "{{ range(1000000000) | list | length }}",
        "{{ range(100000000) | join(',') }}",
    ] {
        assert!(
            !within(Duration::from_secs(5), text),
            "{text} should be refused"
        );
    }
}

#[test]
fn runaway_loops_and_recursion_stop() {
    for text in [
        "{% for i in range(100000) %}{% for j in range(100000) %}{% endfor %}{% endfor %}",
        "{% macro f(n) %}{{ f(n + 1) }}{% endmacro %}{{ f(0) }}",
        "{{ [[[[[[[[[[[[[[[[[[[[[[[[[[[[[[1]]]]]]]]]]]]]]]]]]]]]]]]]]]]] }}",
    ] {
        assert!(!within(Duration::from_secs(5), text), "{text} should stop");
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    #[test]
    fn any_template_evaluates_or_fails_cleanly(text in "\\PC{0,200}") {
        let _ = eval(&text);
    }

    /// Template-shaped noise: real delimiters, filters and operators.
    #[test]
    fn template_shaped_input_evaluates_or_fails_cleanly(
        parts in proptest::collection::vec(
            prop::sample::select(vec![
                "{{", "}}", "{%", "%}", " for x in ", " endfor ", " if ", " endif ", "range(", ")",
                " | length", " | join(',')", " | upper", " * ", " ~ ", "'a'", "9", "[", "]", "x", ".", " ",
            ]),
            0..40,
        )
    ) {
        let started = Instant::now();
        let _ = eval(&parts.concat());
        prop_assert!(started.elapsed() < Duration::from_secs(2));
    }
}
