//! The `dataset.query` and `dataset.upsert` steps (design doc §9). A run
//! reads and writes the rows of its own organization and those below it,
//! with row-level security enforcing that scope.

use std::collections::BTreeMap;

use huxley_datasets::{Dataset, DatasetError, Query};
use huxley_db::{OrgScope, begin_scoped};
use serde_json::{Value as Json, json};
use uuid::Uuid;

use crate::Engine;
use crate::walk::error;

fn dataset_error(e: DatasetError) -> Json {
    match e {
        DatasetError::NotFound => error("input", "no such dataset for this organization"),
        DatasetError::Invalid(at, message) => error("input", format!("{at}: {message}")),
        other => error("internal", other.to_string()),
    }
}

impl Engine {
    /// The run's organization and every one below it.
    async fn subtree(&self, org_id: Uuid) -> Result<Vec<Uuid>, Json> {
        let mut tx = begin_scoped(self.db(), &OrgScope::System)
            .await
            .map_err(|e| error("internal", e.to_string()))?;
        let ids: Vec<Uuid> = sqlx::query_scalar(
            "SELECT o.id FROM organizations o WHERE o.path <@ (SELECT path FROM organizations WHERE id = $1)",
        )
        .bind(org_id)
        .fetch_all(&mut *tx)
        .await
        .map_err(|e| error("internal", e.to_string()))?;
        tx.commit()
            .await
            .map_err(|e| error("internal", e.to_string()))?;
        Ok(ids)
    }

    /// The dataset `key` for `org_id` (its own, or the nearest above).
    async fn dataset_for(&self, org_id: Uuid, key: &str) -> Result<Dataset, Json> {
        let mut tx = begin_scoped(self.db(), &OrgScope::System)
            .await
            .map_err(|e| error("internal", e.to_string()))?;
        let dataset = huxley_datasets::find(&mut tx, org_id, key)
            .await
            .map_err(dataset_error)?;
        tx.commit()
            .await
            .map_err(|e| error("internal", e.to_string()))?;
        Ok(dataset)
    }

    /// An `org_id` input, if given: the run's organization or one below it.
    fn org_in(input: &Json, subtree: &[Uuid]) -> Result<Option<Uuid>, Json> {
        match &input["org_id"] {
            Json::Null => Ok(None),
            Json::String(s) => {
                let id: Uuid = s
                    .parse()
                    .map_err(|_| error("input", format!("org_id {s:?} isn't an id")))?;
                if subtree.contains(&id) {
                    Ok(Some(id))
                } else {
                    Err(error(
                        "input",
                        "org_id must be the run's organization or one below it",
                    ))
                }
            }
            _ => Err(error("input", "org_id is an organization id")),
        }
    }

    pub(crate) async fn query_dataset(&self, org_id: Uuid, input: &Json) -> Result<Json, Json> {
        let key = input["dataset"].as_str().unwrap_or_default();
        let dataset = self.dataset_for(org_id, key).await?;
        let subtree = self.subtree(org_id).await?;
        let filters: BTreeMap<String, Json> = match &input["filters"] {
            Json::Null => BTreeMap::new(),
            Json::Object(map) => map.clone().into_iter().collect(),
            _ => return Err(error("input", "filters is an object of column values")),
        };
        let limit = match &input["limit"] {
            Json::Null => None,
            value => Some(
                value
                    .as_i64()
                    .or_else(|| value.as_str().and_then(|s| s.trim().parse().ok()))
                    .ok_or_else(|| error("input", "limit is a number"))?,
            ),
        };
        let query = Query {
            filters,
            org_id: Self::org_in(input, &subtree)?,
            order_by: input["order_by"].as_str().map(str::to_owned),
            limit,
            offset: None,
        };
        // The run's subtree only, enforced by row-level security.
        let mut tx = begin_scoped(self.db(), &OrgScope::Orgs(subtree))
            .await
            .map_err(|e| error("internal", e.to_string()))?;
        let rows = huxley_datasets::query(&mut tx, &dataset, &query)
            .await
            .map_err(dataset_error)?;
        tx.commit()
            .await
            .map_err(|e| error("internal", e.to_string()))?;
        Ok(json!({ "rows": rows.rows, "total": rows.total }))
    }

    pub(crate) async fn upsert_dataset(
        &self,
        org_id: Uuid,
        input: &Json,
        tainted: bool,
    ) -> Result<Json, Json> {
        // Dataset tables are plain tables: secrets don't go in them.
        if tainted {
            return Err(error(
                "input",
                "rows can't contain secrets or values derived from them",
            ));
        }
        let key = input["dataset"].as_str().unwrap_or_default();
        let dataset = self.dataset_for(org_id, key).await?;
        let subtree = self.subtree(org_id).await?;
        let target = Self::org_in(input, &subtree)?.unwrap_or(org_id);
        let Some(rows) = input["rows"].as_array() else {
            return Err(error("input", "rows is a list of objects"));
        };
        let replace = match &input["replace"] {
            Json::Null => false,
            Json::Bool(b) => *b,
            Json::String(s) => matches!(s.trim(), "true" | "True" | "yes"),
            _ => return Err(error("input", "replace is true or false")),
        };
        let mut tx = begin_scoped(self.db(), &OrgScope::Orgs(subtree))
            .await
            .map_err(|e| error("internal", e.to_string()))?;
        let written = if replace {
            huxley_datasets::replace(&mut tx, &dataset, target, rows).await
        } else {
            huxley_datasets::upsert(&mut tx, &dataset, target, rows).await
        }
        .map_err(dataset_error)?;
        tx.commit()
            .await
            .map_err(|e| error("internal", e.to_string()))?;
        Ok(json!({ "written": written }))
    }
}
