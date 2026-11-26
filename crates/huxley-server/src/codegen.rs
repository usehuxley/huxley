//! `huxley workflows codegen`: TypeScript for an organization's integration
//! actions, for `@usehuxley/sdk`: each action's input and output types, and
//! a builder per action that makes its `integration.action` step.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use serde_json::Value as Json;

/// Deeper than this, a schema is `unknown`.
const MAX_DEPTH: usize = 6;

fn words(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut prev_lower = false;
    for c in s.chars() {
        if !c.is_ascii_alphanumeric() {
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
            prev_lower = false;
            continue;
        }
        if c.is_ascii_uppercase() && prev_lower && !cur.is_empty() {
            out.push(std::mem::take(&mut cur));
        }
        prev_lower = c.is_ascii_lowercase() || c.is_ascii_digit();
        cur.push(c.to_ascii_lowercase());
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

fn capitalized(w: &str) -> String {
    let mut c = w.chars();
    c.next()
        .map(|f| f.to_ascii_uppercase().to_string() + c.as_str())
        .unwrap_or_default()
}

/// `list_users` → `ListUsers`.
pub fn pascal(s: &str) -> String {
    let p: String = words(s).iter().map(|w| capitalized(w)).collect();
    if p.starts_with(|c: char| c.is_ascii_digit()) || p.is_empty() {
        format!("X{p}")
    } else {
        p
    }
}

/// `list_users` → `listUsers`.
pub fn camel(s: &str) -> String {
    let p = pascal(s);
    let mut c = p.chars();
    c.next()
        .map(|f| f.to_ascii_lowercase().to_string() + c.as_str())
        .unwrap_or_default()
}

fn is_ident(s: &str) -> bool {
    s.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_' || c == '$')
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$')
}

fn prop_name(s: &str) -> String {
    if is_ident(s) {
        s.to_owned()
    } else {
        serde_json::to_string(s).unwrap_or_default()
    }
}

fn doc(text: Option<&str>, indent: &str) -> String {
    let Some(text) = text.map(str::trim).filter(|t| !t.is_empty()) else {
        return String::new();
    };
    let text = text.replace("*/", "*\\/");
    let lines: Vec<&str> = text.lines().collect();
    if lines.len() == 1 {
        format!("{indent}/** {} */\n", lines[0])
    } else {
        let body: String = lines
            .iter()
            .map(|l| {
                if l.trim().is_empty() {
                    format!("{indent} *\n")
                } else {
                    format!("{indent} * {l}\n")
                }
            })
            .collect();
        format!("{indent}/**\n{body}{indent} */\n")
    }
}

/// A JSON Schema as a TypeScript type. With `inputs`, values other than
/// text may also be an `Expression` (`{{ … }}`), as workflow inputs can be.
pub fn ts_type(schema: &Json, inputs: bool, depth: usize) -> String {
    let wrap = |t: String| {
        if inputs {
            format!("{t} | Expression")
        } else {
            t
        }
    };
    if depth > MAX_DEPTH {
        return "unknown".into();
    }
    if let Some(values) = schema.get("enum").and_then(Json::as_array) {
        let literals: Vec<String> = values.iter().map(ToString::to_string).collect();
        return if literals.is_empty() {
            "never".into()
        } else {
            wrap(literals.join(" | "))
        };
    }
    if let Some(c) = schema.get("const") {
        return wrap(c.to_string());
    }
    for key in ["oneOf", "anyOf"] {
        if let Some(variants) = schema.get(key).and_then(Json::as_array) {
            let mut parts: Vec<String> = variants
                .iter()
                .map(|v| ts_type(v, inputs, depth + 1))
                .collect();
            parts.dedup();
            return if parts.is_empty() {
                "unknown".into()
            } else {
                parts.join(" | ")
            };
        }
    }
    let types: Vec<&str> = match schema.get("type") {
        Some(Json::String(t)) => vec![t.as_str()],
        Some(Json::Array(ts)) => ts.iter().filter_map(Json::as_str).collect(),
        _ if schema.get("properties").is_some() => vec!["object"],
        _ => return "unknown".into(),
    };
    let mut parts: Vec<String> = Vec::new();
    for t in types {
        let part = match t {
            "string" => "string".to_owned(),
            "number" | "integer" => wrap("number".into()),
            "boolean" => wrap("boolean".into()),
            "null" => "null".into(),
            "array" => {
                let items = schema
                    .get("items")
                    .map_or("unknown".into(), |i| ts_type(i, inputs, depth + 1));
                wrap(format!("Array<{items}>"))
            }
            "object" => wrap(object(schema, inputs, depth)),
            _ => "unknown".into(),
        };
        if !parts.contains(&part) {
            parts.push(part);
        }
    }
    parts.join(" | ")
}

fn object(schema: &Json, inputs: bool, depth: usize) -> String {
    let Some(props) = schema
        .get("properties")
        .and_then(Json::as_object)
        .filter(|p| !p.is_empty())
    else {
        return "Record<string, unknown>".into();
    };
    let required: Vec<&str> = schema
        .get("required")
        .and_then(Json::as_array)
        .map(|r| r.iter().filter_map(Json::as_str).collect())
        .unwrap_or_default();
    let indent = "\t".repeat(depth + 1);
    let mut out = String::from("{\n");
    // Required first (as listed), then the rest by name: the same whatever
    // order the map keeps.
    let mut names: Vec<&String> = props.keys().collect();
    names.sort_by_key(|n| {
        (
            required.iter().position(|r| r == n).unwrap_or(usize::MAX),
            (*n).clone(),
        )
    });
    for name in names {
        let prop = &props[name];
        out.push_str(&doc(
            prop.get("description").and_then(Json::as_str),
            &indent,
        ));
        let optional = if required.contains(&name.as_str()) {
            ""
        } else {
            "?"
        };
        let _ = writeln!(
            out,
            "{indent}{}{optional}: {};",
            prop_name(name),
            ts_type(prop, inputs, depth + 1)
        );
    }
    let _ = write!(out, "{}}}", "\t".repeat(depth));
    out
}

/// The generated module for `actions` (`/api/v1/integrations/actions?schemas=true`).
pub fn typescript(actions: &[Json], source: &str) -> String {
    let mut by_integration: BTreeMap<String, Vec<&Json>> = BTreeMap::new();
    for a in actions {
        if let Some(i) = a["integration"].as_str() {
            by_integration.entry(i.to_owned()).or_default().push(a);
        }
    }
    let mut out = format!(
        "// Generated by `huxley workflows codegen` from {source}. Do not edit: run it again\n\
         // when integrations change.\n\
         import {{ step, type Expression, type StepFields }} from '@usehuxley/sdk';\n\n\
         /** A step's other fields: `retry`, `timeout`, `if`, `connection`, `all_pages`… */\n\
         export type ActionOptions = Omit<StepFields<'integration.action'>, 'action' | 'with'>;\n"
    );
    let mut used: Vec<String> = Vec::new();
    for (integration, mut list) in by_integration {
        list.sort_by_key(|a| a["action"].as_str().unwrap_or_default().to_owned());
        let name = list[0]["integrationName"].as_str().unwrap_or(&integration);
        let mut builders = String::new();
        let mut methods: Vec<String> = Vec::new();
        for a in list {
            let action = a["action"].as_str().unwrap_or_default();
            let key = action.split_once('/').map_or(action, |(_, k)| k);
            let base = format!("{}{}", pascal(&integration), pascal(key));
            let mut type_name = base.clone();
            let mut n = 2;
            while used.contains(&type_name) {
                type_name = format!("{base}{n}");
                n += 1;
            }
            used.push(type_name.clone());
            // The step's `with` is an object; what's in it may be expressions.
            let input = a
                .get("inputSchema")
                .map_or("Record<string, unknown>".into(), |s| object(s, true, 0));
            let output = a
                .get("outputSchema")
                .map_or("unknown".into(), |s| ts_type(s, false, 0));
            let summary = format!(
                "{}{}. `{} {}`",
                a["name"].as_str().unwrap_or(key),
                if a["deprecated"] == Json::Bool(true) {
                    " (deprecated)"
                } else {
                    ""
                },
                a["method"].as_str().unwrap_or_default(),
                a["path"].as_str().unwrap_or_default()
            );
            let _ = write!(
                out,
                "\n{}export type {type_name}Input = {input};\n{}export type {type_name}Output = {output};\n",
                doc(Some(&format!("{summary}: its inputs.")), ""),
                doc(Some(&format!("{summary}: what it returns.")), ""),
            );
            let mut method = camel(key);
            let mut m = 2;
            while methods.contains(&method) {
                method = format!("{}{m}", camel(key));
                m += 1;
            }
            methods.push(method.clone());
            let description = a["description"]
                .as_str()
                .map(|d| format!("{summary}\n\n{d}"));
            builders.push_str(&doc(Some(description.as_deref().unwrap_or(&summary)), "\t"));
            let _ = writeln!(
                builders,
                "\t{method}: (id: string, input: {type_name}Input, options: ActionOptions = {{}}) =>\n\
                 \t\tstep(id, 'integration.action', {{ ...options, action: {}, with: input }}),",
                serde_json::to_string(action).unwrap_or_default()
            );
        }
        let _ = write!(
            out,
            "\n{}export const {} = {{\n{builders}}} as const;\n",
            doc(Some(&format!("{name} (`{integration}`).")), ""),
            camel(&integration)
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn names() {
        assert_eq!(
            (pascal("list_users"), camel("list_users")),
            ("ListUsers".into(), "listUsers".into())
        );
        assert_eq!(camel("getUserByID"), "getUserById");
        assert_eq!(camel("cw-automate"), "cwAutomate");
        assert_eq!(pascal("2fa"), "X2fa");
    }

    #[test]
    fn schemas_become_types() {
        let s = json!({ "type": "object", "required": ["id"], "properties": {
            "id": { "type": "string" },
            "top": { "type": "integer", "description": "How many" },
            "state": { "enum": ["open", "closed"] },
            "tags": { "type": "array", "items": { "type": "string" } },
            "odd-name": { "type": ["string", "null"] },
        } });
        let t = ts_type(&s, true, 0);
        assert!(t.contains("\tid: string;"), "{t}");
        assert!(
            t.contains("/** How many */\n\ttop?: number | Expression;"),
            "{t}"
        );
        assert!(
            t.contains("state?: \"open\" | \"closed\" | Expression;"),
            "{t}"
        );
        assert!(t.contains("tags?: Array<string> | Expression;"), "{t}");
        assert!(t.contains("\"odd-name\"?: string | null;"), "{t}");
        assert_eq!(
            ts_type(&json!({ "type": "object" }), false, 0),
            "Record<string, unknown>"
        );
        assert_eq!(ts_type(&json!({}), false, 0), "unknown");
    }

    /// The SDK type-checks and uses this fixture
    /// (`packages/sdk/test/fixtures`). Refresh with `UPDATE_CODEGEN=1`.
    #[test]
    fn fixture_is_up_to_date() {
        let dir = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../packages/sdk/test/fixtures"
        );
        let actions: Vec<Json> =
            serde_json::from_str(&std::fs::read_to_string(format!("{dir}/actions.json")).unwrap())
                .unwrap();
        let generated = typescript(&actions, "test fixtures");
        let path = format!("{dir}/integrations.ts");
        if std::env::var_os("UPDATE_CODEGEN").is_some() {
            std::fs::write(&path, &generated).unwrap();
        }
        assert_eq!(
            std::fs::read_to_string(&path).unwrap_or_default(),
            generated,
            "packages/sdk/test/fixtures/integrations.ts is stale: run UPDATE_CODEGEN=1 cargo test -p huxley-server codegen"
        );
    }
}
