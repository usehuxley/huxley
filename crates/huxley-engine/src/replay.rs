//! Running from a step with pinned data: a test run that starts at a chosen
//! top-level step, with every step before it taking its recorded result
//! from an earlier run instead of running again.

use std::collections::BTreeSet;

use huxley_db::{OrgScope, begin_scoped};
use serde_json::Value as Json;
use sqlx::PgConnection;
use uuid::Uuid;

use crate::journal::{open, seal};
use crate::{Engine, EngineResult};

/// The ids of every step inside `steps` (nested ones included).
fn step_ids(steps: &[Json], out: &mut BTreeSet<String>) {
    fn walk(value: &Json, out: &mut BTreeSet<String>) {
        match value {
            Json::Object(map) => {
                if let (Some(Json::String(id)), Some(Json::String(_))) =
                    (map.get("id"), map.get("type"))
                {
                    out.insert(id.clone());
                }
                for v in map.values() {
                    walk(v, out);
                }
            }
            Json::Array(items) => {
                for v in items {
                    walk(v, out);
                }
            }
            _ => {}
        }
    }
    for s in steps {
        walk(s, out);
    }
}

/// The steps before top-level step `from` in `document`, or why not.
pub fn steps_before(document: &Json, from: &str) -> Result<BTreeSet<String>, String> {
    let steps = document["spec"]["steps"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let index = steps
        .iter()
        .position(|s| s["id"] == from)
        .ok_or_else(|| format!("{from:?} isn't a top-level step of this workflow"))?;
    let mut ids = BTreeSet::new();
    step_ids(&steps[..index], &mut ids);
    Ok(ids)
}

/// The type of each step in `document`, by id (nested ones included).
fn step_types(document: &Json) -> std::collections::BTreeMap<String, String> {
    fn walk(value: &Json, out: &mut std::collections::BTreeMap<String, String>) {
        match value {
            Json::Object(map) => {
                if let (Some(Json::String(id)), Some(Json::String(kind))) =
                    (map.get("id"), map.get("type"))
                {
                    out.insert(id.clone(), kind.clone());
                }
                map.values().for_each(|v| walk(v, out));
            }
            Json::Array(items) => items.iter().for_each(|v| walk(v, out)),
            _ => {}
        }
    }
    let mut out = std::collections::BTreeMap::new();
    walk(&document["spec"]["steps"], &mut out);
    out
}

/// The most mocked output, in bytes, a test may give.
pub const MAX_MOCK_BYTES: usize = 256 * 1024;

/// Checks test mocks against `document`: each key is a step id, or a step
/// id inside loops with its iteration indices (`inner[2][0]`).
pub fn check_mocks(
    document: &Json,
    mocks: &std::collections::BTreeMap<String, Json>,
) -> Result<(), String> {
    let types = step_types(document);
    for (key, output) in mocks {
        let id = key.split('[').next().unwrap_or_default();
        if !types.contains_key(id) {
            return Err(format!("mocks.{key}: there is no step {id:?}"));
        }
        if crate::journal::step_key(id, &crate::journal::parse_indices(key)) != *key {
            return Err(format!(
                "mocks.{key}: a step id, or one with loop indices like {id}[0]"
            ));
        }
        if serde_json::to_vec(output).map_or(0, |b| b.len()) > MAX_MOCK_BYTES {
            return Err(format!("mocks.{key}: at most {MAX_MOCK_BYTES} bytes"));
        }
    }
    Ok(())
}

#[derive(sqlx::FromRow)]
struct Pinned {
    step_key: String,
    attempt: i32,
    step_id: String,
    step_type: String,
    status: crate::journal::StepStatus,
    input: Option<Json>,
    output: Option<Json>,
    tainted: bool,
    input_sealed: Option<Vec<u8>>,
    output_tainted: bool,
    output_sealed: Option<Vec<u8>>,
    input_ref: Option<String>,
    output_ref: Option<String>,
}

impl Engine {
    /// Records `mocks` (checked with [`check_mocks`]) as succeeded steps of
    /// test run `run_id`, so it uses them instead of running those steps.
    pub async fn mock_steps(
        &self,
        conn: &mut PgConnection,
        org_id: Uuid,
        run_id: Uuid,
        document: &Json,
        mocks: &std::collections::BTreeMap<String, Json>,
    ) -> EngineResult<usize> {
        check_mocks(document, mocks).map_err(crate::EngineError::Document)?;
        let types = step_types(document);
        for (key, output) in mocks {
            let id = key.split('[').next().unwrap_or_default();
            sqlx::query(
                "INSERT INTO step_runs (run_id, step_key, attempt, org_id, step_id, step_type, status, \
                        output, finished_at) \
                 VALUES ($1, $2, 1, $3, $4, $5, 'succeeded', $6, now())",
            )
            .bind(run_id)
            .bind(key)
            .bind(org_id)
            .bind(id)
            .bind(&types[id])
            .bind(output)
            .execute(&mut *conn)
            .await?;
        }
        Ok(mocks.len())
    }

    /// Copies `source`'s results for `steps` into run `run_id` (same
    /// organization), so the run treats them as done. Sealed values are
    /// re-encrypted for the new run. Returns how many records were pinned.
    pub async fn pin_steps(
        &self,
        conn: &mut PgConnection,
        org_id: Uuid,
        run_id: Uuid,
        source: Uuid,
        steps: &BTreeSet<String>,
    ) -> EngineResult<usize> {
        let ids: Vec<&String> = steps.iter().collect();
        let rows: Vec<Pinned> = {
            let mut tx = begin_scoped(self.db(), &OrgScope::System).await?;
            let rows = sqlx::query_as(
                "SELECT DISTINCT ON (step_key) step_key, attempt, step_id, step_type, status, input, \
                        output, tainted, input_sealed, output_tainted, output_sealed, input_ref, output_ref \
                   FROM step_runs \
                  WHERE run_id = $1 AND org_id = $2 AND NOT superseded AND step_id = ANY($3) \
                  ORDER BY step_key, attempt DESC",
            )
            .bind(source)
            .bind(org_id)
            .bind(&ids)
            .fetch_all(&mut *tx)
            .await?;
            tx.commit().await?;
            rows
        };
        let mut pinned = 0;
        for row in rows {
            if !matches!(
                row.status,
                crate::journal::StepStatus::Succeeded | crate::journal::StepStatus::Skipped
            ) {
                continue;
            }
            let input_purpose = format!("step_input:{}", row.step_key);
            let output_purpose = format!("step_output:{}:{}", row.step_key, row.attempt);
            let reseal =
                async |sealed: Option<Vec<u8>>, purpose: &str| -> EngineResult<Option<Vec<u8>>> {
                    let Some(sealed) = sealed else {
                        return Ok(None);
                    };
                    let value = open(&self.crypto, source, purpose, &sealed).await?;
                    Ok(Some(
                        seal(&self.crypto, org_id, run_id, purpose, &value).await?,
                    ))
                };
            let input_sealed = reseal(row.input_sealed, &input_purpose).await?;
            let output_sealed = reseal(row.output_sealed, &output_purpose).await?;
            sqlx::query(
                "INSERT INTO step_runs (run_id, step_key, attempt, org_id, step_id, step_type, status, \
                        input, output, tainted, input_sealed, output_tainted, output_sealed, \
                        input_ref, output_ref, finished_at) \
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, now())",
            )
            .bind(run_id)
            .bind(&row.step_key)
            .bind(row.attempt)
            .bind(org_id)
            .bind(&row.step_id)
            .bind(&row.step_type)
            .bind(row.status)
            .bind(&row.input)
            .bind(&row.output)
            .bind(row.tainted)
            .bind(input_sealed)
            .bind(row.output_tainted)
            .bind(output_sealed)
            .bind(&row.input_ref)
            .bind(&row.output_ref)
            .execute(&mut *conn)
            .await?;
            pinned += 1;
        }
        Ok(pinned)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn steps_before_a_top_level_step_include_nested_ones() {
        let doc = json!({ "spec": { "steps": [
            { "id": "a", "type": "transform", "output": 1 },
            { "id": "loop", "type": "foreach", "items": [], "steps": [
                { "id": "inner", "type": "transform", "output": 2 } ] },
            { "id": "b", "type": "transform", "output": 3 },
            { "id": "c", "type": "transform", "output": 4 } ] } });
        let ids = steps_before(&doc, "b").unwrap();
        assert_eq!(ids, ["a", "inner", "loop"].map(String::from).into());
        assert!(steps_before(&doc, "a").unwrap().is_empty());
        assert!(steps_before(&doc, "inner").is_err(), "only top-level steps");
    }
}
