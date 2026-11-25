//! The UI hints that change what a form asks: `show_if` (a field shown only
//! for certain answers) and `options_from` (choices looked up through an
//! integration action).

use std::collections::BTreeSet;

use huxley_engine::Engine;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value as Json, json};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::{Form, FormError, FormResult, invalid};

/// Most choices a lookup returns.
pub const MAX_CHOICES: usize = 1000;

/// When a field is shown: `{ "field": "x", "equals": v }` or
/// `{ "field": "x", "in": [v, …] }`. A several-choices field matches if
/// any of its answers does.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ShowIf {
    pub field: String,
    #[serde(default)]
    pub equals: Option<Json>,
    #[serde(default, rename = "in")]
    pub any_of: Option<Vec<Json>>,
}

impl ShowIf {
    fn matches(&self, answer: Option<&Json>) -> bool {
        let wanted: Vec<&Json> = match (&self.equals, &self.any_of) {
            (Some(v), _) => vec![v],
            (None, Some(vs)) => vs.iter().collect(),
            (None, None) => return answer.is_some_and(|a| !a.is_null()),
        };
        match answer {
            Some(Json::Array(items)) => items.iter().any(|i| wanted.contains(&i)),
            Some(a) => wanted.contains(&a),
            None => false,
        }
    }
}

/// Choices from an integration action:
/// `{ "action": "<integration>/<action>", "connection"?: key,
///    "with"?: {…}, "items"?: "path.to.list", "value": "id",
///    "label"?: "displayName" }`,
/// or from a dataset's rows (the organization's, and those below it):
/// `{ "dataset": "<key>", "filters"?: { column: value }, "value": "upn",
///    "label"?: "name" }`.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OptionsFrom {
    #[serde(default)]
    pub action: Option<String>,
    #[serde(default)]
    pub dataset: Option<String>,
    /// Column values the dataset's rows must equal.
    #[serde(default)]
    pub filters: Option<Json>,
    #[serde(default)]
    pub connection: Option<String>,
    #[serde(default, rename = "with")]
    pub inputs: Option<Json>,
    /// Where the list is in the response (dot-separated); default: the
    /// response itself.
    #[serde(default)]
    pub items: Option<String>,
    /// Each item's value (dot-separated path); default: the item itself.
    #[serde(default)]
    pub value: Option<String>,
    /// Each item's label; default: its value.
    #[serde(default)]
    pub label: Option<String>,
}

/// One choice.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct Choice {
    pub value: String,
    pub label: String,
}

fn hint<'a>(ui: &'a Json, field: &str) -> Option<&'a Map<String, Json>> {
    ui.get(field).and_then(Json::as_object)
}

pub(crate) fn show_if(ui: &Json, field: &str) -> Option<ShowIf> {
    serde_json::from_value(hint(ui, field)?.get("show_if")?.clone()).ok()
}

pub(crate) fn options_from(ui: &Json, field: &str) -> Option<OptionsFrom> {
    serde_json::from_value(hint(ui, field)?.get("options_from")?.clone()).ok()
}

/// Checks the hints against the schema's fields.
pub(crate) fn check(schema: &Json, ui: &Json) -> FormResult<()> {
    let empty = Map::new();
    let fields = schema["properties"].as_object().unwrap_or(&empty);
    for name in fields.keys() {
        let Some(hints) = hint(ui, name) else {
            continue;
        };
        if let Some(raw) = hints.get("show_if") {
            let at = format!("ui.{name}.show_if");
            let rule: ShowIf = serde_json::from_value(raw.clone()).map_err(|e| {
                invalid(
                    &at,
                    format!("{{ field, equals }} or {{ field, in: [...] }}: {e}"),
                )
            })?;
            if rule.field == *name || !fields.contains_key(&rule.field) {
                return Err(invalid(at, "show_if needs another field of this form"));
            }
        }
        if let Some(raw) = hints.get("options_from") {
            let at = format!("ui.{name}.options_from");
            let lookup: OptionsFrom = serde_json::from_value(raw.clone())
                .map_err(|e| invalid(&at, format!("{{ action, value, label, … }}: {e}")))?;
            match (&lookup.action, &lookup.dataset) {
                (Some(action), None) => {
                    if action
                        .split_once('/')
                        .is_none_or(|(i, a)| i.is_empty() || a.is_empty())
                    {
                        return Err(invalid(at, "action is <integration>/<action>"));
                    }
                }
                (None, Some(dataset)) => {
                    if dataset.trim().is_empty() || lookup.value.is_none() {
                        return Err(invalid(
                            at,
                            "a dataset's choices need its key and a value column",
                        ));
                    }
                }
                _ => return Err(invalid(at, "set action or dataset")),
            }
            let field = &fields[name];
            let string = field["type"] == "string"
                || (field["type"] == "array" && field["items"]["type"] == "string");
            if !string {
                return Err(invalid(
                    at,
                    "choices from an action need a string field (or a list of strings)",
                ));
            }
        }
    }
    check_cycles(fields, ui)
}

/// `show_if` rules mustn't go round in a circle.
fn check_cycles(fields: &Map<String, Json>, ui: &Json) -> FormResult<()> {
    for start in fields.keys() {
        let mut seen = BTreeSet::new();
        let mut at = start.clone();
        while let Some(rule) = show_if(ui, &at) {
            if !seen.insert(at.clone()) || rule.field == *start {
                return Err(invalid(
                    format!("ui.{start}.show_if"),
                    "show_if rules go round in a circle",
                ));
            }
            at = rule.field;
        }
    }
    Ok(())
}

/// The answers to fields that are shown: a hidden field's answer is
/// dropped (also when what hides it is itself hidden). Returns them and
/// the hidden fields.
pub fn shown(form: &Form, answers: &Json) -> (Json, BTreeSet<String>) {
    let mut kept = answers.as_object().cloned().unwrap_or_default();
    let names: Vec<String> = form.schema["properties"]
        .as_object()
        .map(|p| p.keys().cloned().collect())
        .unwrap_or_default();
    let mut hidden = BTreeSet::new();
    // Hiding one field can hide another: repeat until nothing changes.
    loop {
        let mut changed = false;
        for name in &names {
            if hidden.contains(name) {
                continue;
            }
            if let Some(rule) = show_if(&form.ui, name)
                && (hidden.contains(&rule.field) || !rule.matches(kept.get(&rule.field)))
            {
                hidden.insert(name.clone());
                kept.remove(name);
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    (Json::Object(kept), hidden)
}

/// The schema to check answers against: hidden fields aren't required.
pub(crate) fn schema_for(form: &Form, hidden: &BTreeSet<String>) -> Json {
    let mut schema = form.schema.clone();
    // `get_mut`, not indexing: indexing a missing key inserts `null`, and
    // `"required": null` isn't a valid schema.
    if let Some(required) = schema.get_mut("required").and_then(Json::as_array_mut) {
        required.retain(|r| r.as_str().is_none_or(|r| !hidden.contains(r)));
    }
    schema
}

fn at_path<'a>(value: &'a Json, path: Option<&str>) -> Option<&'a Json> {
    let Some(path) = path.filter(|p| !p.is_empty()) else {
        return Some(value);
    };
    path.split('.').try_fold(value, |v, key| match v {
        Json::Array(items) => items.get(key.parse::<usize>().ok()?),
        _ => v.get(key),
    })
}

fn text(value: &Json) -> Option<String> {
    match value {
        Json::String(s) => Some(s.clone()),
        Json::Number(n) => Some(n.to_string()),
        Json::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

/// Turns an action's response into choices.
pub fn choices_of(lookup: &OptionsFrom, response: &Json) -> Result<Vec<Choice>, String> {
    let list = at_path(response, lookup.items.as_deref())
        .and_then(Json::as_array)
        .ok_or_else(|| match &lookup.items {
            Some(path) => format!("the response has no list at {path:?}"),
            None => "the response isn't a list (set items to where the list is)".into(),
        })?;
    let mut out = Vec::new();
    for item in list.iter().take(MAX_CHOICES) {
        let Some(value) = at_path(item, lookup.value.as_deref()).and_then(text) else {
            continue;
        };
        let label = lookup
            .label
            .as_deref()
            .and_then(|p| at_path(item, Some(p)))
            .and_then(text)
            .unwrap_or_else(|| value.clone());
        out.push(Choice { value, label });
    }
    Ok(out)
}

/// The choices of field `name`, looked up for `org_id`.
pub async fn options(
    engine: &Engine,
    form: &Form,
    name: &str,
    org_id: Uuid,
) -> FormResult<Vec<Choice>> {
    let Some(lookup) = options_from(&form.ui, name) else {
        return Err(FormError::NotFound);
    };
    if let Some(dataset) = &lookup.dataset {
        let input = json!({ "dataset": dataset, "filters": lookup.filters, "limit": MAX_CHOICES });
        let rows = match engine.lookup_dataset(org_id, &input).await {
            Ok(found) => found,
            Err(failure) => {
                let message = failure["message"]
                    .as_str()
                    .unwrap_or("the dataset couldn't be read");
                return Err(FormError::Lookup(message.to_owned()));
            }
        };
        let lookup = OptionsFrom {
            items: Some("rows".into()),
            ..lookup
        };
        return choices_of(&lookup, &rows).map_err(FormError::Lookup);
    }
    let input = json!({
        "action": lookup.action,
        "connection": lookup.connection,
        "inputs": lookup.inputs.clone().unwrap_or_else(|| json!({})),
    });
    let response = match engine.lookup_action(org_id, &input).await? {
        Ok(body) => body,
        Err(failure) => {
            let message = failure["message"].as_str().unwrap_or("the lookup failed");
            return Err(FormError::Lookup(message.to_owned()));
        }
    };
    choices_of(&lookup, &response).map_err(FormError::Lookup)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn form(schema: Json, ui: Json) -> Form {
        Form {
            id: Uuid::nil(),
            org_id: Uuid::nil(),
            key: "f".into(),
            name: "F".into(),
            description: None,
            schema,
            ui,
            action_for: None,
            archived: false,
            created_at: time::OffsetDateTime::UNIX_EPOCH,
            updated_at: time::OffsetDateTime::UNIX_EPOCH,
        }
    }

    #[test]
    fn hidden_fields_and_their_dependents_are_dropped() {
        let f = form(
            json!({ "type": "object", "required": ["kind", "laptop", "model"], "properties": {
                "kind": { "type": "string" }, "laptop": { "type": "boolean" },
                "model": { "type": "string" }, "extras": { "type": "array", "items": { "type": "string" } },
                "dock": { "type": "string" } } }),
            json!({
                "laptop": { "show_if": { "field": "kind", "equals": "starter" } },
                "model": { "show_if": { "field": "laptop", "equals": true } },
                "dock": { "show_if": { "field": "extras", "in": ["dock", "stand"] } }
            }),
        );
        let answers = json!({ "kind": "leaver", "laptop": true, "model": "X1", "extras": ["dock"], "dock": "TB4" });
        let (kept, hidden) = shown(&f, &answers);
        assert_eq!(
            kept,
            json!({ "kind": "leaver", "extras": ["dock"], "dock": "TB4" })
        );
        assert_eq!(hidden, ["laptop", "model"].map(String::from).into());
        assert_eq!(schema_for(&f, &hidden)["required"], json!(["kind"]));
        let (kept, _) = shown(
            &f,
            &json!({ "kind": "starter", "laptop": true, "model": "X1" }),
        );
        assert_eq!(kept["model"], "X1");
    }

    #[test]
    fn hints_are_checked() {
        let schema = json!({ "type": "object", "properties": {
            "a": { "type": "string" }, "b": { "type": "string" }, "n": { "type": "integer" } } });
        let ok = json!({ "b": { "show_if": { "field": "a", "in": ["x"] },
                                "options_from": { "action": "graph/users", "value": "id" } } });
        assert!(check(&schema, &ok).is_ok());
        for bad in [
            json!({ "b": { "show_if": { "field": "nope", "equals": 1 } } }),
            json!({ "b": { "show_if": { "field": "b", "equals": 1 } } }),
            json!({ "a": { "show_if": { "field": "b" } }, "b": { "show_if": { "field": "a" } } }),
            json!({ "b": { "show_if": { "fld": "a" } } }),
            json!({ "b": { "options_from": { "action": "noslash" } } }),
            json!({ "n": { "options_from": { "action": "g/a" } } }),
        ] {
            assert!(check(&schema, &bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn responses_become_choices() {
        let lookup: OptionsFrom = serde_json::from_value(json!({
            "action": "g/u", "items": "value", "value": "id", "label": "name.display" }))
        .unwrap();
        let response = json!({ "value": [
            { "id": "1", "name": { "display": "Ada" } },
            { "id": 2 },
            { "name": "no id" } ] });
        assert_eq!(
            choices_of(&lookup, &response).unwrap(),
            vec![
                Choice {
                    value: "1".into(),
                    label: "Ada".into()
                },
                Choice {
                    value: "2".into(),
                    label: "2".into()
                }
            ]
        );
        let plain: OptionsFrom = serde_json::from_value(json!({ "action": "g/u" })).unwrap();
        assert_eq!(choices_of(&plain, &json!(["a"])).unwrap()[0].value, "a");
        assert!(choices_of(&plain, &json!({ "value": [] })).is_err());
    }
}
