//! Datasets (design doc §9) as sources (ADR-041): `dataset:<key>`, with a
//! field per column (JSON columns aside), the row's organization, and when
//! it was synced. Read with `dataset:read`; rows are those of the
//! organizations the caller can read datasets at.

use std::borrow::Cow;

use serde_json::Value as Json;
use sqlx::PgConnection;
use uuid::Uuid;

use crate::AnalyticsResult;
use crate::catalog::{DATASET_PREFIX, Field, Kind, Source};
use crate::query::Scope;

/// The permission that reads datasets.
pub const PERMISSION: &str = "dataset:read";

fn column_name_ok(name: &str) -> bool {
    name.len() <= 63
        && name.starts_with(|c: char| c.is_ascii_lowercase())
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

/// A dataset's source, from its definition.
pub fn source_of(
    id: Uuid,
    key: &str,
    name: &str,
    description: Option<&str>,
    columns: &Json,
) -> Source {
    let mut fields = vec![Field {
        key: Cow::Borrowed("_organization"),
        label: Cow::Borrowed("Organization"),
        kind: Kind::Text,
        sql: Cow::Borrowed("o.name"),
    }];
    let mut time_field: Option<String> = None;
    for column in columns.as_array().into_iter().flatten() {
        let (Some(col), Some(kind)) = (column["name"].as_str(), column["type"].as_str()) else {
            continue;
        };
        if !column_name_ok(col) {
            continue;
        }
        let (kind, sql) = match kind {
            "text" => (Kind::Text, format!("d.\"{col}\"")),
            "uuid" => (Kind::Text, format!("d.\"{col}\"::text")),
            "integer" | "number" => (Kind::Number, format!("d.\"{col}\"")),
            "boolean" => (Kind::Bool, format!("d.\"{col}\"")),
            "timestamp" => (Kind::Time, format!("d.\"{col}\"")),
            "date" => (Kind::Date, format!("d.\"{col}\"")),
            _ => continue,
        };
        if time_field.is_none() && matches!(kind, Kind::Time | Kind::Date) {
            time_field = Some(col.to_owned());
        }
        fields.push(Field {
            key: Cow::Owned(col.to_owned()),
            label: Cow::Owned(col.replace('_', " ")),
            kind,
            sql: Cow::Owned(sql),
        });
    }
    fields.push(Field {
        key: Cow::Borrowed("_synced_at"),
        label: Cow::Borrowed("Synced"),
        kind: Kind::Time,
        sql: Cow::Borrowed("d._synced_at"),
    });
    Source {
        key: Cow::Owned(format!("{DATASET_PREFIX}{key}")),
        label: Cow::Owned(name.to_owned()),
        description: Cow::Owned(description.unwrap_or("A dataset.").to_owned()),
        module: None,
        permission: PERMISSION,
        time_field: Cow::Owned(time_field.unwrap_or_else(|| "_synced_at".into())),
        fields: Cow::Owned(fields),
        from: Cow::Owned(format!(
            "datasets.\"ds_{}\" d JOIN organizations o ON o.id = d._org_id",
            id.simple()
        )),
    }
}

/// Dataset sources for a caller who can read datasets in `scope`: those
/// defined there or above. The same key at several organizations is the
/// one nearest the top (shared the widest).
pub async fn sources(conn: &mut PgConnection, scope: &Scope) -> AnalyticsResult<Vec<Source>> {
    let orgs: Option<Vec<Uuid>> = match scope {
        Scope::All => None,
        Scope::Orgs(orgs) => Some(orgs.clone()),
    };
    let rows: Vec<(Uuid, String, String, Option<String>, Json)> = sqlx::query_as(
        "SELECT DISTINCT ON (d.key) d.id, d.key, d.name, d.description, d.columns \
           FROM datasets d JOIN organizations o ON o.id = d.org_id \
          WHERE $1::uuid[] IS NULL \
             OR EXISTS (SELECT 1 FROM organizations s WHERE s.id = ANY ($1) AND o.path @> s.path) \
          ORDER BY d.key, nlevel(o.path), d.id",
    )
    .bind(orgs)
    .fetch_all(conn)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(id, key, name, description, columns)| {
            source_of(id, &key, &name, description.as_deref(), &columns)
        })
        .collect())
}

/// One dataset source by its key (`dataset:<key>`), if the caller can read it.
pub async fn source(
    conn: &mut PgConnection,
    scope: &Scope,
    key: &str,
) -> AnalyticsResult<Option<Source>> {
    Ok(sources(conn, scope)
        .await?
        .into_iter()
        .find(|s| s.key == key))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn columns_become_fields() {
        let s = source_of(
            Uuid::nil(),
            "computers",
            "Computers",
            None,
            &json!([
                { "name": "hostname", "type": "text" },
                { "name": "ram_gb", "type": "number" },
                { "name": "last_seen", "type": "timestamp" },
                { "name": "raw", "type": "json" },
                { "name": "Bad Name", "type": "text" },
            ]),
        );
        let keys: Vec<&str> = s.fields.iter().map(|f| &*f.key).collect();
        assert_eq!(
            keys,
            [
                "_organization",
                "hostname",
                "ram_gb",
                "last_seen",
                "_synced_at"
            ]
        );
        assert_eq!(
            (&*s.key, &*s.time_field),
            ("dataset:computers", "last_seen")
        );
        assert!(
            s.from
                .starts_with("datasets.\"ds_00000000000000000000000000000000\" d")
        );
    }
}
