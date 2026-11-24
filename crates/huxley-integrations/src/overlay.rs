//! Overlays (design doc §7.1): your edits to an integration's actions
//! (their names, descriptions and groups, and which are hidden), stored
//! apart from the vendor's spec. They're applied whenever a definition is
//! read, so a newly imported version keeps them; an edit follows its
//! action through renames (the version's aliases). Calls never change:
//! workflows use action keys, and hidden actions still work, they just
//! aren't offered.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value as Json;
use utoipa::ToSchema;

use crate::Definition;

/// Edits to one action.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ActionOverlay {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    /// Not offered when picking actions (still callable).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub hidden: bool,
    /// How the action pages through results, for `all_pages`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pagination: Option<crate::Pagination>,
}

/// An integration's overlay: edits by action key.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Overlay {
    #[serde(default)]
    pub actions: BTreeMap<String, ActionOverlay>,
    /// At most this many calls per connection in a window, across every
    /// worker; calls over it wait for the next window.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rate_limit: Option<RateLimit>,
}

/// A fixed-window rate limit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RateLimit {
    /// Calls allowed per window (1–100000).
    pub requests: u32,
    /// The window's length in seconds (1–86400).
    pub per_seconds: u32,
}

impl Overlay {
    /// Checks lengths; drops empty edits.
    pub fn checked(mut self) -> Result<Self, (String, &'static str)> {
        for (key, edit) in &mut self.actions {
            let trim = |v: &mut Option<String>| {
                *v = v
                    .take()
                    .map(|s| s.trim().to_owned())
                    .filter(|s| !s.is_empty());
            };
            trim(&mut edit.name);
            trim(&mut edit.description);
            trim(&mut edit.group);
            if edit.name.as_ref().is_some_and(|n| n.chars().count() > 200) {
                return Err((format!("actions.{key}.name"), "at most 200 characters"));
            }
            if edit.group.as_ref().is_some_and(|n| n.chars().count() > 100) {
                return Err((format!("actions.{key}.group"), "at most 100 characters"));
            }
            if edit
                .description
                .as_ref()
                .is_some_and(|n| n.chars().count() > 4000)
            {
                return Err((
                    format!("actions.{key}.description"),
                    "at most 4000 characters",
                ));
            }
        }
        self.actions.retain(|_, e| *e != ActionOverlay::default());
        if let Some(limit) = self.rate_limit
            && (!(1..=100_000).contains(&limit.requests)
                || !(1..=86_400).contains(&limit.per_seconds))
        {
            return Err((
                "rateLimit".into(),
                "requests from 1 to 100000, per_seconds from 1 to 86400",
            ));
        }
        Ok(self)
    }

    pub fn from_json(value: &Json) -> Self {
        serde_json::from_value(value.clone()).unwrap_or_default()
    }

    /// Applies the edits to `definition`. `aliases` maps old action keys to
    /// their new ones (from renames), so edits follow their actions.
    pub fn apply(&self, definition: &mut Definition, aliases: &Json) {
        for (key, edit) in &self.actions {
            let target = if definition.actions.iter().any(|a| a.key == *key) {
                key.as_str()
            } else {
                match aliases[key.as_str()].as_str() {
                    Some(renamed) => renamed,
                    None => continue,
                }
            };
            let Some(action) = definition.actions.iter_mut().find(|a| a.key == target) else {
                continue;
            };
            if let Some(name) = &edit.name {
                action.name.clone_from(name);
            }
            if let Some(description) = &edit.description {
                action.description = Some(description.clone());
            }
            if let Some(group) = &edit.group {
                action.group = Some(group.clone());
            }
            action.hidden = edit.hidden;
            if let Some(pagination) = &edit.pagination {
                action.pagination = Some(pagination.clone());
            }
        }
    }
}
