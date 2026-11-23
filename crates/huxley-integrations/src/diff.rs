//! What changes between two versions of an integration, and which changes
//! can break what uses it: shown before a newer spec is applied.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::Value as Json;
use utoipa::ToSchema;

use crate::{Action, AuthScheme, Definition};

/// One difference, and whether it can break workflows or connections.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct Change {
    pub breaking: bool,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ActionChanges {
    pub key: String,
    pub changes: Vec<Change>,
}

/// An action that seems to have been renamed: gone under one key, back
/// under another with the same method and path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct Rename {
    pub from: String,
    pub to: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct DefinitionDiff {
    /// True if any change can break something already using the integration.
    pub breaking: bool,
    pub added: Vec<String>,
    /// Actions that are gone (renamed ones are listed under `renamed`).
    pub removed: Vec<String>,
    pub renamed: Vec<Rename>,
    pub changed: Vec<ActionChanges>,
    /// Auth and server changes, which matter to connections.
    pub connection: Vec<Change>,
}

impl DefinitionDiff {
    pub fn is_empty(&self) -> bool {
        self.added.is_empty()
            && self.removed.is_empty()
            && self.renamed.is_empty()
            && self.changed.is_empty()
            && self.connection.is_empty()
    }
}

fn breaking(message: impl Into<String>) -> Change {
    Change {
        breaking: true,
        message: message.into(),
    }
}

fn note(message: impl Into<String>) -> Change {
    Change {
        breaking: false,
        message: message.into(),
    }
}

/// Compares an integration's current definition with a newer one.
pub fn diff(old: &Definition, new: &Definition) -> DefinitionDiff {
    let old_actions: BTreeMap<&str, &Action> =
        old.actions.iter().map(|a| (a.key.as_str(), a)).collect();
    let new_actions: BTreeMap<&str, &Action> =
        new.actions.iter().map(|a| (a.key.as_str(), a)).collect();

    let mut added: Vec<String> = new_actions
        .keys()
        .filter(|k| !old_actions.contains_key(*k))
        .map(|k| (*k).to_owned())
        .collect();
    let mut removed: Vec<String> = old_actions
        .keys()
        .filter(|k| !new_actions.contains_key(*k))
        .map(|k| (*k).to_owned())
        .collect();

    // Same method and path under a new key: probably a renamed operation id.
    let mut renamed = Vec::new();
    removed.retain(|gone| {
        let before = old_actions[gone.as_str()];
        let found = added.iter().position(|k| {
            let after = new_actions[k.as_str()];
            after.method == before.method && after.path == before.path
        });
        match found {
            Some(i) => {
                renamed.push(Rename {
                    from: gone.clone(),
                    to: added.remove(i),
                });
                false
            }
            None => true,
        }
    });

    let mut changed = Vec::new();
    for (key, before) in &old_actions {
        let Some(after) = new_actions.get(key) else {
            continue;
        };
        let found = action_changes(before, after);
        if !found.is_empty() {
            changed.push(ActionChanges {
                key: (*key).to_owned(),
                changes: found,
            });
        }
    }
    for rename in &renamed {
        let found = action_changes(
            old_actions[rename.from.as_str()],
            new_actions[rename.to.as_str()],
        );
        if !found.is_empty() {
            changed.push(ActionChanges {
                key: rename.to.clone(),
                changes: found,
            });
        }
    }

    let connection = connection_changes(old, new);
    let breaking = !removed.is_empty()
        || !renamed.is_empty()
        || changed.iter().flat_map(|a| &a.changes).any(|c| c.breaking)
        || connection.iter().any(|c| c.breaking);
    DefinitionDiff {
        breaking,
        added,
        removed,
        renamed,
        changed,
        connection,
    }
}

fn required(schema: &Json) -> BTreeSet<&str> {
    schema["required"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Json::as_str)
        .collect()
}

fn action_changes(before: &Action, after: &Action) -> Vec<Change> {
    let mut changes = Vec::new();
    if before.method != after.method || before.path != after.path {
        changes.push(breaking(format!(
            "now {} {} (was {} {})",
            after.method, after.path, before.method, before.path
        )));
    }
    let old_props = before.input_schema["properties"].as_object();
    let new_props = after.input_schema["properties"].as_object();
    let (old_required, new_required) = (
        required(&before.input_schema),
        required(&after.input_schema),
    );
    // Sorted, so the report doesn't depend on how maps keep their order.
    let mut old_sorted: Vec<(&String, &Json)> = old_props.into_iter().flatten().collect();
    old_sorted.sort_by_key(|(name, _)| *name);
    for (name, old_schema) in old_sorted {
        match new_props.and_then(|p| p.get(name)) {
            None => changes.push(breaking(format!("input {name} was removed"))),
            Some(new_schema) => {
                if old_schema.get("type") != new_schema.get("type") {
                    changes.push(breaking(format!(
                        "input {name} is now {} (was {})",
                        type_name(new_schema),
                        type_name(old_schema)
                    )));
                } else if old_schema != new_schema {
                    changes.push(note(format!("input {name}'s schema changed")));
                }
            }
        }
    }
    let mut new_names: Vec<&str> = new_props
        .into_iter()
        .flatten()
        .map(|(n, _)| n.as_str())
        .collect();
    new_names.sort_unstable();
    for name in new_names {
        let is_new = old_props.is_none_or(|p| !p.contains_key(name));
        if new_required.contains(name) && (is_new || !old_required.contains(name)) {
            changes.push(breaking(format!("input {name} is now required")));
        } else if is_new {
            changes.push(note(format!("new optional input {name}")));
        }
    }
    let content_type = |a: &Action| a.body.as_ref().map(|b| b.content_type.clone());
    if content_type(before) != content_type(after) && before.body.is_some() {
        changes.push(breaking(format!(
            "the request body is now {}",
            content_type(after).unwrap_or_else(|| "gone".into())
        )));
    }
    if before.output_schema != after.output_schema {
        changes.push(note(
            "the response's shape changed: check expressions that read it",
        ));
    }
    if before.auth != after.auth {
        changes.push(note(format!(
            "accepted auth is now {}",
            if after.auth.is_empty() {
                "none".to_owned()
            } else {
                after.auth.join(", ")
            }
        )));
    }
    if after.deprecated && !before.deprecated {
        changes.push(note("now deprecated"));
    }
    changes
}

fn type_name(schema: &Json) -> String {
    match schema.get("type") {
        Some(Json::String(t)) => t.clone(),
        Some(other) => other.to_string(),
        None => "untyped".into(),
    }
}

fn connection_changes(old: &Definition, new: &Definition) -> Vec<Change> {
    let mut changes = Vec::new();
    let schemes = |d: &Definition| -> BTreeMap<String, AuthScheme> {
        d.auth_schemes
            .iter()
            .map(|s| (s.key.clone(), s.clone()))
            .collect()
    };
    let (before, after) = (schemes(old), schemes(new));
    for (key, scheme) in &before {
        match after.get(key) {
            None => changes.push(breaking(format!(
                "auth scheme {key} was removed: connections using it stop working"
            ))),
            Some(now) if now.kind != scheme.kind => changes.push(breaking(format!(
                "auth scheme {key} changed: connections using it may need setting up again"
            ))),
            Some(_) => {}
        }
    }
    for key in after.keys().filter(|k| !before.contains_key(*k)) {
        changes.push(note(format!("new auth scheme {key}")));
    }
    let urls = |d: &Definition| d.servers.iter().map(|s| s.url.clone()).collect::<Vec<_>>();
    if urls(old) != urls(new) {
        changes.push(note(format!(
            "servers are now {}",
            if new.servers.is_empty() {
                "unspecified".to_owned()
            } else {
                urls(new).join(", ")
            }
        )));
    }
    changes
}
