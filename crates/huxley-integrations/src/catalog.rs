//! Built-in integrations (design doc §7.3): curated OpenAPI specs shipped
//! with Huxley, added to an organization in one click. Once added they're
//! ordinary integrations: updated, pinned and overlaid like any other.

use huxley_core::RequestContext;
use serde::Serialize;
use sqlx::PgConnection;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::store::{self, IntegrationDetail, StoreResult};

struct Entry {
    key: &'static str,
    name: &'static str,
    summary: &'static str,
    category: &'static str,
    spec: &'static str,
    /// The overlay it starts with (e.g. how its lists page), as JSON.
    overlay: &'static str,
}

const ENTRIES: &[Entry] = &[
    Entry {
        key: "msgraph",
        name: "Microsoft Graph",
        summary: "Microsoft 365 and Entra ID: users, licenses, groups, mail.",
        category: "Microsoft",
        spec: include_str!("../catalog/microsoft-graph.yaml"),
        overlay: r#"{ "actions": {
            "listUsers": { "pagination": { "style": "next_url", "next": "@odata.nextLink", "items": "value" } },
            "listGroups": { "pagination": { "style": "next_url", "next": "@odata.nextLink", "items": "value" } },
            "listGroupMembers": { "pagination": { "style": "next_url", "next": "@odata.nextLink", "items": "value" } },
            "listUserGroups": { "pagination": { "style": "next_url", "next": "@odata.nextLink", "items": "value" } }
        } }"#,
    },
    Entry {
        key: "slack",
        name: "Slack",
        summary: "Post messages to channels with a bot token.",
        category: "Messaging",
        spec: include_str!("../catalog/slack.yaml"),
        overlay: r#"{ "actions": {
            "listChannels": { "pagination": { "style": "cursor", "cursor": "response_metadata.next_cursor",
                                               "param": "cursor", "items": "channels" } }
        } }"#,
    },
];

/// A built-in integration as the catalog shows it.
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CatalogEntry {
    /// The key it's added under.
    pub key: String,
    pub name: String,
    pub summary: String,
    pub category: String,
    /// How many actions it has.
    pub actions: usize,
}

/// Every built-in integration.
pub fn list() -> Vec<CatalogEntry> {
    ENTRIES
        .iter()
        .map(|e| CatalogEntry {
            key: e.key.to_owned(),
            name: e.name.to_owned(),
            summary: e.summary.to_owned(),
            category: e.category.to_owned(),
            actions: crate::parse(e.spec)
                .ok()
                .and_then(|spec| crate::import_value(spec).ok())
                .map_or(0, |i| i.definition.actions.len()),
        })
        .collect()
}

/// The spec of built-in integration `key`.
pub fn spec(key: &str) -> Option<&'static str> {
    ENTRIES.iter().find(|e| e.key == key).map(|e| e.spec)
}

/// Adds built-in integration `key` to `org_id`, under its own key.
pub async fn add(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    org_id: Uuid,
    key: &str,
) -> StoreResult<Option<IntegrationDetail>> {
    let Some(entry) = ENTRIES.iter().find(|e| e.key == key) else {
        return Ok(None);
    };
    let created = store::create(&mut *conn, ctx, org_id, Some(key), entry.spec).await?;
    let overlay: crate::overlay::Overlay =
        serde_json::from_str(entry.overlay).expect("built-in overlays parse");
    store::set_overlay(conn, ctx, created.summary.id, overlay)
        .await
        .map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_built_in_imports() {
        for entry in ENTRIES {
            let spec = crate::parse(entry.spec).unwrap_or_else(|e| panic!("{}: {e}", entry.key));
            let imported =
                crate::import_value(spec).unwrap_or_else(|e| panic!("{}: {e}", entry.key));
            assert!(
                imported.warnings.is_empty(),
                "{}: {:?}",
                entry.key,
                imported.warnings
            );
            assert_ne!(imported.definition.actions.len(), 0, "{}", entry.key);
        }
        for entry in ENTRIES {
            let overlay: crate::overlay::Overlay = serde_json::from_str(entry.overlay).unwrap();
            let checked = overlay.clone().checked().unwrap();
            assert_eq!(checked, overlay, "{}", entry.key);
            let spec = crate::parse(entry.spec).unwrap();
            let definition = crate::import_value(spec).unwrap().definition;
            for key in overlay.actions.keys() {
                assert!(
                    definition.actions.iter().any(|a| a.key == *key),
                    "{}: {key}",
                    entry.key
                );
            }
        }
        let graph = list().into_iter().find(|e| e.key == "msgraph").unwrap();
        assert_eq!(graph.actions, 14);
    }
}
