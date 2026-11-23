//! Inlining local `$ref`s (`#/components/schemas/User`), so each action's
//! schemas stand alone. Recursive schemas are cut where they loop, and a
//! node budget stops a few shared schemas from multiplying without bound.

use serde_json::{Map, Value as Json, json};

use crate::Warning;

/// Most JSON nodes one inlined value may grow to.
const NODE_BUDGET: usize = 50_000;
/// Deepest `$ref` chain followed.
const MAX_DEPTH: usize = 64;

pub(crate) struct Resolver<'a> {
    root: &'a Json,
    pub(crate) warnings: Vec<Warning>,
}

/// Looks up a JSON pointer such as `#/components/schemas/User`.
pub(crate) fn lookup<'a>(root: &'a Json, reference: &str) -> Option<&'a Json> {
    let pointer = reference.strip_prefix('#')?;
    if pointer.is_empty() {
        return Some(root);
    }
    root.pointer(pointer)
}

impl<'a> Resolver<'a> {
    pub(crate) const fn new(root: &'a Json) -> Self {
        Self {
            root,
            warnings: Vec::new(),
        }
    }

    pub(crate) fn warn(&mut self, pointer: &str, message: impl Into<String>) {
        self.warnings.push(Warning {
            pointer: pointer.to_owned(),
            message: message.into(),
        });
    }

    /// Follows a `$ref` (if `value` is one) to what it names, without
    /// inlining anything inside it.
    pub(crate) fn deref<'b>(&mut self, value: &'b Json, at: &str) -> Option<&'b Json>
    where
        'a: 'b,
    {
        let mut current = value;
        for _ in 0..MAX_DEPTH {
            let Some(reference) = current.get("$ref").and_then(Json::as_str) else {
                return Some(current);
            };
            let Some(next) = lookup(self.root, reference) else {
                self.warn(at, format!("can't resolve {reference}"));
                return None;
            };
            current = next;
        }
        self.warn(at, "$ref chain too long");
        None
    }

    /// `value` with every local `$ref` inside it replaced by its target.
    pub(crate) fn inline(&mut self, value: &Json, at: &str) -> Json {
        let mut budget = NODE_BUDGET;
        let mut stack = Vec::new();
        let out = self.walk(value, at, &mut stack, &mut budget);
        if budget == 0 {
            self.warn(
                at,
                "schema too large to inline fully; the rest is left open",
            );
        }
        out
    }

    fn walk(
        &mut self,
        value: &Json,
        at: &str,
        stack: &mut Vec<String>,
        budget: &mut usize,
    ) -> Json {
        if *budget == 0 {
            return json!({});
        }
        *budget -= 1;
        match value {
            Json::Object(map) => {
                if let Some(reference) = map.get("$ref").and_then(Json::as_str) {
                    return self.follow(map, reference, at, stack, budget);
                }
                let mut out = Map::with_capacity(map.len());
                for (key, inner) in map {
                    out.insert(key.clone(), self.walk(inner, at, stack, budget));
                }
                Json::Object(out)
            }
            Json::Array(items) => Json::Array(
                items
                    .iter()
                    .map(|inner| self.walk(inner, at, stack, budget))
                    .collect(),
            ),
            other => other.clone(),
        }
    }

    fn follow(
        &mut self,
        map: &Map<String, Json>,
        reference: &str,
        at: &str,
        stack: &mut Vec<String>,
        budget: &mut usize,
    ) -> Json {
        let name = reference.rsplit('/').next().unwrap_or(reference).to_owned();
        if stack.iter().any(|r| r == reference) || stack.len() >= MAX_DEPTH {
            // A schema that contains itself: stop here.
            return json!({ "description": format!("{name} (recursive)") });
        }
        let Some(target) = lookup(self.root, reference) else {
            if reference.starts_with('#') {
                self.warn(at, format!("can't resolve {reference}"));
            } else {
                self.warn(at, format!("external reference {reference} isn't followed"));
            }
            return json!({});
        };
        stack.push(reference.to_owned());
        let mut resolved = self.walk(target, at, stack, budget);
        stack.pop();
        // OpenAPI 3.1 allows keywords next to $ref (a description, say).
        if map.len() > 1
            && let Json::Object(out) = &mut resolved
        {
            for (key, inner) in map {
                if key != "$ref" {
                    out.insert(key.clone(), self.walk(inner, at, stack, budget));
                }
            }
        }
        resolved
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inlines_and_cuts_cycles() {
        let root = json!({
            "components": { "schemas": {
                "Node": { "type": "object", "properties": {
                    "name": { "type": "string" },
                    "child": { "$ref": "#/components/schemas/Node" }
                } },
                "Wrapper": { "$ref": "#/components/schemas/Node", "description": "outer" }
            } }
        });
        let mut r = Resolver::new(&root);
        let out = r.inline(&json!({ "$ref": "#/components/schemas/Wrapper" }), "/x");
        assert_eq!(out["description"], "outer");
        assert_eq!(out["properties"]["name"]["type"], "string");
        assert_eq!(
            out["properties"]["child"]["description"],
            "Node (recursive)"
        );
        assert_eq!(r.warnings, vec![]);
    }

    #[test]
    fn dangling_and_external_refs_warn() {
        let root = json!({});
        let mut r = Resolver::new(&root);
        assert_eq!(r.inline(&json!({ "$ref": "#/nope" }), "/a"), json!({}));
        assert_eq!(
            r.inline(&json!({ "$ref": "other.yaml#/X" }), "/b"),
            json!({})
        );
        assert_eq!(r.warnings.len(), 2);
        assert_eq!(r.warnings[1].pointer, "/b");
    }
}
