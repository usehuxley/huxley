//! Evaluating expressions.

use std::collections::BTreeMap;

use huxley_expr::{Engine, ExprError, REDACTED, Scope, redact};
use serde_json::json;

fn scope() -> Scope {
    Scope::new()
        .with(
            "inputs",
            &json!({ "firstName": "Ada", "lastName": "Lovelace", "count": 3 }),
        )
        .with(
            "steps",
            &json!({ "lookup": { "output": { "id": 42, "tags": ["a", "b"], "none": null } } }),
        )
        .with_secrets(BTreeMap::from([(
            "api_key".to_owned(),
            "sk-live-123456".to_owned(),
        )]))
}

#[test]
fn whole_expressions_are_typed_and_templates_are_strings() {
    let engine = Engine::new();
    let s = scope();
    let eval = |v: serde_json::Value| engine.eval(&v, &s).unwrap().value;

    assert_eq!(eval(json!("{{ steps.lookup.output.id }}")), json!(42));
    assert_eq!(
        eval(json!("  {{ steps.lookup.output.tags }} ")),
        json!(["a", "b"])
    );
    assert_eq!(eval(json!("{{ inputs.count > 2 }}")), json!(true));
    assert_eq!(
        eval(json!("{{ {'a': 1, 'b': [inputs.count]} }}")),
        json!({ "a": 1, "b": [3] })
    );
    assert_eq!(eval(json!("{{ steps.lookup.output.none }}")), json!(null));
    assert_eq!(
        eval(json!("{{ steps.lookup.output.missing }}")),
        json!(null)
    );
    assert_eq!(
        eval(json!("{{ inputs.firstName }} {{ inputs.lastName }}")),
        json!("Ada Lovelace")
    );
    assert_eq!(
        eval(json!("id={{ steps.lookup.output.id }}")),
        json!("id=42")
    );
    assert_eq!(eval(json!("plain text")), json!("plain text"));
    // Nested values are evaluated recursively; non-strings pass through.
    assert_eq!(
        eval(
            json!({ "upn": "{{ inputs.firstName | lower }}@x.com", "n": 5, "list": ["{{ 1 + 1 }}"] })
        ),
        json!({ "upn": "ada@x.com", "n": 5, "list": [2] })
    );
}

#[test]
fn filters_functions_and_python_methods() {
    let engine = Engine::new();
    let s = scope();
    let eval = |t: &str| engine.eval_str(t, &s).unwrap().value;
    assert_eq!(eval("{{ inputs.firstName.upper() }}"), json!("ADA"));
    assert_eq!(
        eval("{{ '{\"a\": [1]}' | from_json }}"),
        json!({ "a": [1] })
    );
    assert_eq!(eval("{{ {'a': 1} | to_json }}"), json!("{\"a\":1}"));
    assert_eq!(eval("{{ 'user:pass' | b64encode }}"), json!("dXNlcjpwYXNz"));
    assert_eq!(eval("{{ 'dXNlcjpwYXNz' | b64decode }}"), json!("user:pass"));
    assert_eq!(eval("{{ 'Hello World' | slugify }}"), json!("hello-world"));
    assert_eq!(
        eval("{{ 'a1b22c333' | regex_findall('[0-9]+') }}"),
        json!(["1", "22", "333"])
    );
    assert_eq!(
        eval("{{ 'ab-12' | regex_replace('[0-9]', 'x') }}"),
        json!("ab-xx")
    );
    assert_eq!(eval("{{ none | default_if_none('d') }}"), json!("d"));
    assert_eq!(eval("{{ uuid() | length }}"), json!(36));
    assert_eq!(
        eval("{{ steps.lookup.output.missing is defined }}"),
        json!(false)
    );
    assert_eq!(
        eval("{{ steps.lookup.output.missing | default('x') }}"),
        json!("x")
    );
}

#[test]
fn conditions_use_truthiness() {
    let engine = Engine::new();
    let s = scope();
    assert!(
        engine
            .eval_condition("{{ inputs.count == 3 }}", &s)
            .unwrap()
    );
    assert!(
        !engine
            .eval_condition("{{ steps.lookup.output.none }}", &s)
            .unwrap()
    );
    assert!(
        !engine
            .eval_condition("{{ steps.lookup.output.missing }}", &s)
            .unwrap()
    );
    assert!(
        engine
            .eval_condition("{{ steps.lookup.output.tags }}", &s)
            .unwrap()
    );
}

#[test]
fn errors_are_reported() {
    let engine = Engine::new();
    let s = scope();
    for bad in [
        "{{ inputs.firstName | nope }}",
        "{{ 1 + }}",
        "{{ steps.missing.output }}",
        "{{ steps.lookup.output.missing.deeper }}",
        "{{ 'x' | b64decode }}",
    ] {
        assert!(
            matches!(engine.eval_str(bad, &s), Err(ExprError::Jinja(_))),
            "{bad}"
        );
    }
}

#[test]
fn evaluation_is_bounded() {
    let engine = Engine::new();
    let s = scope();
    // Fuel: an effectively endless loop stops.
    let spin = "{% for i in range(100000) %}{% for j in range(100000) %}{% endfor %}{% endfor %}";
    assert!(engine.eval_str(spin, &s).is_err());
    // Output size.
    let big = "{% for i in range(40000) %}{{ 'xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx' }}{% endfor %}";
    assert!(matches!(
        engine.eval_str(big, &s),
        Err(ExprError::TooLarge | ExprError::Jinja(_))
    ));
    // No templates to include.
    assert!(engine.eval_str("{% include 'x' %}", &s).is_err());
}

#[test]
fn secrets_taint_the_result() {
    let engine = Engine::new();
    let s = scope();
    let plain = engine.eval_str("{{ inputs.firstName }}", &s).unwrap();
    assert!(!plain.tainted);

    let auth = engine.eval_str("Bearer {{ secrets.api_key }}", &s).unwrap();
    assert_eq!(auth.value, json!("Bearer sk-live-123456"));
    assert!(auth.tainted);
    // Transformed secrets are still tainted, though redact() can't spot them.
    let encoded = engine
        .eval_str("{{ secrets.api_key | b64encode }}", &s)
        .unwrap();
    assert!(encoded.tainted);

    let generated = engine.eval_str("{{ secrets.generate(32) }}", &s).unwrap();
    assert!(generated.tainted);
    assert_eq!(generated.value.as_str().unwrap().len(), 32);
    assert!(engine.eval_str("{{ secrets.generate(4) }}", &s).is_err());

    // The object itself reveals nothing, typed or printed, and can't be iterated.
    assert_eq!(
        engine.eval_str("{{ secrets }}", &s).unwrap().value,
        json!(REDACTED)
    );
    assert_eq!(
        engine.eval_str("x{{ secrets }}", &s).unwrap().value,
        json!("x«redacted»")
    );
    assert_eq!(
        engine
            .eval_str("{{ secrets | to_json }}", &s)
            .unwrap()
            .value,
        json!("\"«redacted»\"")
    );
    assert!(engine.eval_str("{{ secrets | list }}", &s).is_err());
    assert!(
        engine
            .eval_str("{{ secrets.unknown }}", &s)
            .unwrap()
            .value
            .is_null()
    );

    let logged = redact(
        &json!({ "headers": { "Authorization": auth.value } }),
        s.secret_values(),
    );
    assert_eq!(
        logged,
        json!({ "headers": { "Authorization": "Bearer «redacted»" } })
    );
}

#[test]
fn references_lists_what_an_expression_reads() {
    let engine = Engine::new();
    assert_eq!(
        engine
            .references("{{ steps.a.output.x ~ inputs.name }} {% set y = 1 %}{{ y }}")
            .unwrap(),
        vec!["inputs.name".to_owned(), "steps.a.output.x".to_owned()]
    );
    assert_eq!(engine.references("no jinja").unwrap(), Vec::<String>::new());
    assert!(engine.references("{{ oops(").is_err());
}
