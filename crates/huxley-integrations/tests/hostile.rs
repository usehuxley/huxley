//! Hostile OpenAPI documents: spec imports take whatever an admin pastes or
//! uploads. They must fail cleanly, quickly and without exhausting memory.

use std::fmt::Write as _;

use proptest::prelude::*;

#[test]
fn alias_bombs_and_deep_nesting_are_refused_quickly() {
    let mut bomb = String::from("openapi: 3.0.3\na0: &a0 [\"lol\"]\n");
    for i in 1..=9 {
        let refs = vec![format!("*a{}", i - 1); 10].join(", ");
        let _ = writeln!(bomb, "a{i}: &a{i} [{refs}]");
    }
    for doc in [
        bomb,
        "[".repeat(200_000),
        format!("{{\"openapi\": {}", "[".repeat(200_000)),
    ] {
        let started = std::time::Instant::now();
        let handle = std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(move || huxley_integrations::import(&doc).is_err())
            .unwrap();
        assert!(handle.join().expect("import panicked"));
        assert!(started.elapsed() < std::time::Duration::from_secs(5));
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    #[test]
    fn any_text_imports_or_fails_cleanly(text in "\\PC{0,400}") {
        let _ = huxley_integrations::import(&text);
    }

    /// Spec-shaped JSON: real keys, random structure and values.
    #[test]
    fn spec_shaped_documents_import_or_fail_cleanly(
        paths in proptest::collection::vec("/[a-z{}]{0,12}", 0..6),
        methods in proptest::collection::vec(prop::sample::select(vec!["get", "post", "put", "delete", "x-y", "parameters"]), 0..4),
        refs in proptest::collection::vec("#/[a-zA-Z/~01]{0,20}", 0..4),
    ) {
        let mut spec_paths = serde_json::Map::new();
        for (i, p) in paths.iter().enumerate() {
            let mut ops = serde_json::Map::new();
            for m in &methods {
                ops.insert((*m).to_owned(), serde_json::json!({
                    "operationId": format!("op{i}"),
                    "parameters": refs.iter().map(|r| serde_json::json!({ "$ref": r })).collect::<Vec<_>>(),
                    "requestBody": { "content": { "application/json": { "schema": { "$ref": refs.first().cloned().unwrap_or_default() } } } },
                    "responses": { "200": { "description": "ok" } },
                }));
            }
            spec_paths.insert(p.clone(), serde_json::Value::Object(ops));
        }
        let spec = serde_json::json!({
            "openapi": "3.0.3",
            "info": { "title": "t", "version": "1" },
            "paths": spec_paths,
            "components": { "schemas": { "A": { "$ref": "#/components/schemas/A" } } },
        });
        let _ = huxley_integrations::import_value(spec);
    }
}
