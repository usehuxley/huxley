//! `lookup()` answers for runs (see `huxley_expr::Lookups`): recorded the
//! first time a run asks, so replays take the same path.

use std::collections::BTreeMap;

use huxley_expr::{LookupAnswer, LookupRequest};
use serde_json::{Value as Json, json};
use sqlx::PgConnection;
use uuid::Uuid;

use crate::{Engine, EngineResult};

/// The run's recorded answers.
pub(crate) async fn recorded(
    conn: &mut PgConnection,
    run_id: Uuid,
) -> EngineResult<BTreeMap<String, LookupAnswer>> {
    let rows: Vec<(String, Option<Json>, Option<String>)> =
        sqlx::query_as("SELECT key, row, error FROM run_lookups WHERE run_id = $1")
            .bind(run_id)
            .fetch_all(conn)
            .await?;
    Ok(rows
        .into_iter()
        .map(|(key, row, error)| {
            let answer = match error {
                Some(why) => Err(why),
                None => Ok(row.filter(|r| !r.is_null())),
            };
            (key, answer)
        })
        .collect())
}

/// Forgets failed lookups, so a resumed run asks again (the dataset may
/// exist now).
pub(crate) async fn forget_failures(conn: &mut PgConnection, run_id: Uuid) -> EngineResult<()> {
    sqlx::query("DELETE FROM run_lookups WHERE run_id = $1 AND error IS NOT NULL")
        .bind(run_id)
        .execute(conn)
        .await?;
    Ok(())
}

impl Engine {
    /// Answers lookups for a run of `org_id` (its dataset, or the nearest
    /// above; rows of its subtree only, as `dataset.query` reads them) and
    /// records the answers.
    pub(crate) async fn answer_lookups(
        &self,
        conn: &mut PgConnection,
        run_id: Uuid,
        org_id: Uuid,
        requests: Vec<LookupRequest>,
    ) -> EngineResult<BTreeMap<String, LookupAnswer>> {
        let mut answers = BTreeMap::new();
        for request in requests {
            let input =
                json!({ "dataset": request.dataset, "filters": request.filters, "limit": 1 });
            let answer: LookupAnswer = match self.query_dataset(org_id, &input).await {
                Ok(found) => Ok(found["rows"]
                    .as_array()
                    .and_then(|rows| rows.first())
                    .cloned()),
                Err(e) => Err(e["message"]
                    .as_str()
                    .map_or_else(|| e.to_string(), str::to_owned)),
            };
            let key = request.key();
            let (row, error) = match &answer {
                Ok(row) => (row.clone(), None),
                Err(why) => (None, Some(why.clone())),
            };
            sqlx::query(
                "INSERT INTO run_lookups (run_id, org_id, key, row, error) VALUES ($1, $2, $3, $4, $5) \
                 ON CONFLICT (run_id, key) DO NOTHING",
            )
            .bind(run_id)
            .bind(org_id)
            .bind(&key)
            .bind(row)
            .bind(error)
            .execute(&mut *conn)
            .await?;
            answers.insert(key, answer);
        }
        Ok(answers)
    }
}
