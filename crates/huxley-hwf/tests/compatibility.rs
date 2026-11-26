//! HWF `usehuxley.io/v1` is frozen (ADR-042): later changes may only add.
//! This compares the published schema with the baseline taken when v1 was
//! frozen, and fails on anything a v1 document could break on: a removed
//! step type, trigger, field or allowed value, or a field that became
//! required.
//!
//! The baseline only changes in one way: refreshed after an additive change
//! (`UPDATE_BASELINE=1 cargo test -p huxley-hwf --test compatibility`), so
//! the next change is compared with it.

use serde_json::Value;

fn load(name: &str) -> Value {
    let path = format!("{}/../../schemas/hwf/{name}", env!("CARGO_MANIFEST_DIR"));
    serde_json::from_str(&std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}")))
        .unwrap()
}

/// A oneOf variant's name: its `type` constant (step and trigger types).
fn variant_key(v: &Value) -> Option<String> {
    v.pointer("/properties/type/const")
        .and_then(Value::as_str)
        .map(str::to_owned)
}

fn strings(v: Option<&Value>) -> Vec<String> {
    v.and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

/// Everything `old` accepts, `new` must still accept.
fn compare(old: &Value, new: &Value, at: &str, problems: &mut Vec<String>) {
    // Allowed values: none removed.
    for key in ["enum"] {
        let (o, n) = (strings(old.get(key)), strings(new.get(key)));
        for v in o.iter().filter(|v| !n.contains(v)) {
            problems.push(format!("{at}: the value {v:?} was removed"));
        }
    }
    if let (Some(o), Some(n)) = (old.get("const"), new.get("const"))
        && o != n
    {
        problems.push(format!("{at}: the constant changed from {o} to {n}"));
    }
    // Types: none removed.
    let types = |v: &Value| match v.get("type") {
        Some(Value::String(s)) => vec![s.clone()],
        other => strings(other),
    };
    let (ot, nt) = (types(old), types(new));
    if !nt.is_empty() {
        for t in ot.iter().filter(|t| !nt.contains(t)) {
            problems.push(format!("{at}: the type {t:?} is no longer allowed"));
        }
    }
    // Fields: none removed; none newly required.
    if let Some(props) = old.get("properties").and_then(Value::as_object) {
        for (name, schema) in props {
            match new.pointer(&format!(
                "/properties/{}",
                name.replace('~', "~0").replace('/', "~1")
            )) {
                Some(n) => compare(schema, n, &format!("{at}.{name}"), problems),
                None => problems.push(format!("{at}: the field {name:?} was removed")),
            }
        }
    }
    let (or, nr) = (strings(old.get("required")), strings(new.get("required")));
    for r in nr.iter().filter(|r| !or.contains(r)) {
        problems.push(format!("{at}: the field {r:?} became required"));
    }
    if old.get("additionalProperties") != Some(&Value::Bool(false))
        && new.get("additionalProperties") == Some(&Value::Bool(false))
    {
        problems.push(format!("{at}: extra fields are no longer allowed"));
    }
    // Variants (step and trigger types): none removed; each still accepts as much.
    for key in ["oneOf", "anyOf"] {
        let (Some(o), Some(n)) = (
            old.get(key).and_then(Value::as_array),
            new.get(key).and_then(Value::as_array),
        ) else {
            continue;
        };
        for (i, ov) in o.iter().enumerate() {
            let matched = match variant_key(ov) {
                Some(k) => n
                    .iter()
                    .find(|nv| variant_key(nv).as_deref() == Some(k.as_str())),
                None => n.get(i),
            };
            match matched {
                Some(nv) => compare(
                    ov,
                    nv,
                    &format!("{at}[{}]", variant_key(ov).unwrap_or(i.to_string())),
                    problems,
                ),
                None => problems.push(format!(
                    "{at}: the {} {:?} was removed",
                    if at.ends_with("Trigger") {
                        "trigger"
                    } else {
                        "variant"
                    },
                    variant_key(ov).unwrap_or(i.to_string())
                )),
            }
        }
    }
    if let (Some(o), Some(n)) = (old.get("items"), new.get("items")) {
        compare(o, n, &format!("{at}[]"), problems);
    }
}

#[test]
fn v1_only_ever_grows() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../schemas/hwf/v1.baseline.json"
    );
    let new = load("v1.json");
    if std::env::var_os("UPDATE_BASELINE").is_some() {
        std::fs::write(path, serde_json::to_string_pretty(&new).unwrap() + "\n").unwrap();
    }
    let old = load("v1.baseline.json");
    assert_eq!(old["$id"], new["$id"], "the schema's id is fixed for v1");
    let mut problems = Vec::new();
    compare(&old, &new, "workflow", &mut problems);
    let (od, nd) = (
        old["$defs"].as_object().unwrap(),
        new["$defs"].as_object().unwrap(),
    );
    for (name, schema) in od {
        match nd.get(name) {
            Some(n) => compare(schema, n, name, &mut problems),
            None => problems.push(format!("{name} was removed")),
        }
    }
    assert!(
        problems.is_empty(),
        "HWF v1 is frozen: changes may only add (ADR-042). These would break v1 documents:\n  {}",
        problems.join("\n  ")
    );
}

#[test]
fn the_checker_catches_breaking_changes() {
    let old = serde_json::json!({
        "type": "object", "required": ["id"],
        "properties": { "id": { "type": "string" }, "mode": { "enum": ["a", "b"] } },
        "oneOf": [{ "properties": { "type": { "const": "wait" } } }],
    });
    let new = serde_json::json!({
        "type": "object", "required": ["id", "mode"], "additionalProperties": false,
        "properties": { "mode": { "enum": ["a"] } },
        "oneOf": [],
    });
    let mut problems = Vec::new();
    compare(&old, &new, "x", &mut problems);
    let text = problems.join("\n");
    for expected in [
        "\"id\" was removed",
        "\"b\" was removed",
        "\"mode\" became required",
        "no longer allowed",
        "\"wait\" was removed",
    ] {
        assert!(text.contains(expected), "missing {expected:?} in:\n{text}");
    }
}
