//! The journal: the latest attempt of every step key in a run (ADR-021).

use std::collections::BTreeMap;

use huxley_crypto::{Binding, Crypto};
use huxley_expr::REDACTED;
use serde::Serialize;
use serde_json::{Map, Value as Json, json};
use sqlx::PgConnection;
use uuid::Uuid;
use zeroize::Zeroizing;

use crate::EngineError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, utoipa::ToSchema, sqlx::Type)]
#[sqlx(type_name = "step_status", rename_all = "lowercase")]
#[serde(rename_all = "lowercase")]
pub enum StepStatus {
    Running,
    Waiting,
    Succeeded,
    Failed,
    Skipped,
    Cancelled,
}

impl StepStatus {
    pub const fn is_settled(self) -> bool {
        !matches!(self, Self::Running | Self::Waiting)
    }
}

/// A step's key: its id plus the indices of the enclosing `foreach`
/// iterations, outermost first: `inner[2][0]`.
pub fn step_key(id: &str, indices: &[usize]) -> String {
    use std::fmt::Write as _;
    let mut key = id.to_owned();
    for i in indices {
        let _ = write!(key, "[{i}]");
    }
    key
}

fn parse_indices(key: &str) -> Vec<usize> {
    key.split_once('[').map_or_else(Vec::new, |(_, rest)| {
        rest.trim_end_matches(']')
            .split("][")
            .filter_map(|i| i.parse().ok())
            .collect()
    })
}

/// One attempt of one step.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Record {
    pub step_key: String,
    pub step_id: String,
    pub step_type: String,
    pub attempt: i32,
    pub status: StepStatus,
    pub input: Option<Json>,
    pub output: Option<Json>,
    pub error: Option<Json>,
    pub tainted: bool,
    pub input_sealed: Option<Vec<u8>>,
    pub output_tainted: bool,
    pub output_sealed: Option<Vec<u8>>,
    #[sqlx(skip)]
    pub indices: Vec<usize>,
    /// Changed in memory since it was loaded.
    #[sqlx(skip)]
    pub dirty: bool,
}

impl Record {
    pub fn new(step_id: &str, step_type: &str, indices: &[usize], status: StepStatus) -> Self {
        Self {
            step_key: step_key(step_id, indices),
            step_id: step_id.to_owned(),
            step_type: step_type.to_owned(),
            attempt: 1,
            status,
            input: None,
            output: None,
            error: None,
            tainted: false,
            input_sealed: None,
            output_tainted: false,
            output_sealed: None,
            indices: indices.to_vec(),
            dirty: true,
        }
    }
}

#[derive(Debug, Default)]
pub struct Journal {
    records: BTreeMap<String, Record>,
}

impl Journal {
    pub async fn load(conn: &mut PgConnection, run_id: Uuid) -> sqlx::Result<Self> {
        let rows: Vec<Record> = sqlx::query_as(
            "SELECT DISTINCT ON (step_key) step_key, step_id, step_type, attempt, status, input, \
                    output, error, tainted, input_sealed, output_tainted, output_sealed \
               FROM step_runs WHERE run_id = $1 ORDER BY step_key, attempt DESC",
        )
        .bind(run_id)
        .fetch_all(conn)
        .await?;
        Ok(Self {
            records: rows
                .into_iter()
                .map(|mut r| {
                    r.indices = parse_indices(&r.step_key);
                    (r.step_key.clone(), r)
                })
                .collect(),
        })
    }

    pub fn get(&self, key: &str) -> Option<&Record> {
        self.records.get(key)
    }

    pub fn get_mut(&mut self, key: &str) -> Option<&mut Record> {
        let record = self.records.get_mut(key)?;
        record.dirty = true;
        Some(record)
    }

    pub fn put(&mut self, record: Record) {
        self.records.insert(record.step_key.clone(), record);
    }

    /// Whether any step of the iteration at `indices` has started.
    pub fn iteration_started(&self, indices: &[usize]) -> bool {
        self.records
            .values()
            .any(|r| r.indices.starts_with(indices))
    }

    pub fn records(&self) -> impl Iterator<Item = &Record> {
        self.records.values()
    }

    /// `steps` as expressions see it from a step at `indices`: every step in
    /// the same iteration (or outside all loops), as `{output, status}`.
    pub fn visible(&self, indices: &[usize]) -> Json {
        let mut steps = Map::new();
        for record in self.records.values() {
            if indices.starts_with(&record.indices) {
                let output = match record.status {
                    StepStatus::Failed => record.error.clone(),
                    _ => record.output.clone(),
                };
                steps.insert(
                    record.step_id.clone(),
                    json!({ "output": output.unwrap_or(Json::Null), "status": record.status }),
                );
            }
        }
        Json::Object(steps)
    }

    /// Decrypts sealed inputs and outputs into memory, after [`Journal::load`].
    pub async fn unseal(&mut self, crypto: &Crypto, run_id: Uuid) -> Result<(), EngineError> {
        for record in self.records.values_mut() {
            if let Some(sealed) = &record.input_sealed {
                record.input = Some(open(crypto, run_id, &purpose("input", record), sealed).await?);
            }
            if let Some(sealed) = &record.output_sealed {
                record.output =
                    Some(open(crypto, run_id, &purpose("output", record), sealed).await?);
            }
        }
        Ok(())
    }

    /// Writes changed records. Tainted inputs and outputs are stored
    /// redacted, with the real value encrypted alongside.
    pub async fn persist(
        &mut self,
        conn: &mut PgConnection,
        run_id: Uuid,
        org_id: Uuid,
        crypto: &Crypto,
    ) -> Result<(), EngineError> {
        for record in self.records.values_mut().filter(|r| r.dirty) {
            let (input, input_sealed) = match (&record.input, record.tainted) {
                (Some(value), true) => (
                    Some(Json::String(REDACTED.into())),
                    Some(seal(crypto, org_id, run_id, &purpose("input", record), value).await?),
                ),
                (value, _) => (value.clone(), None),
            };
            let (output, output_sealed) = match (&record.output, record.output_tainted) {
                (Some(value), true) => (
                    Some(Json::String(REDACTED.into())),
                    Some(seal(crypto, org_id, run_id, &purpose("output", record), value).await?),
                ),
                (value, _) => (value.clone(), None),
            };
            sqlx::query(
                "INSERT INTO step_runs (run_id, step_key, attempt, org_id, step_id, step_type, \
                        status, input, output, error, tainted, input_sealed, output_tainted, \
                        output_sealed, finished_at) \
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, \
                         CASE WHEN $15 THEN now() END) \
                 ON CONFLICT (run_id, step_key, attempt) DO UPDATE SET \
                        status = EXCLUDED.status, input = EXCLUDED.input, \
                        output = EXCLUDED.output, error = EXCLUDED.error, \
                        tainted = EXCLUDED.tainted, input_sealed = EXCLUDED.input_sealed, \
                        output_tainted = EXCLUDED.output_tainted, \
                        output_sealed = EXCLUDED.output_sealed, \
                        finished_at = EXCLUDED.finished_at",
            )
            .bind(run_id)
            .bind(&record.step_key)
            .bind(record.attempt)
            .bind(org_id)
            .bind(&record.step_id)
            .bind(&record.step_type)
            .bind(record.status)
            .bind(&input)
            .bind(&output)
            .bind(&record.error)
            .bind(record.tainted)
            .bind(&input_sealed)
            .bind(record.output_tainted)
            .bind(&output_sealed)
            .bind(record.status.is_settled())
            .execute(&mut *conn)
            .await?;
            record.dirty = false;
        }
        Ok(())
    }
}

/// Binds a sealed value to its place: run (the binding's record), step key,
/// and field. Inputs aren't bound to the attempt: every attempt of a step
/// shares the input evaluated when it was first scheduled.
pub(crate) fn purpose(field: &str, record: &Record) -> String {
    if field == "input" {
        format!("step_input:{}", record.step_key)
    } else {
        format!("step_{field}:{}:{}", record.step_key, record.attempt)
    }
}

pub(crate) async fn seal(
    crypto: &Crypto,
    org_id: Uuid,
    run_id: Uuid,
    purpose: &str,
    value: &Json,
) -> Result<Vec<u8>, EngineError> {
    let bytes = Zeroizing::new(serde_json::to_vec(value).unwrap_or_default());
    Ok(crypto
        .encrypt(
            org_id,
            Binding {
                purpose,
                record: run_id,
            },
            &bytes,
        )
        .await?)
}

pub(crate) async fn open(
    crypto: &Crypto,
    run_id: Uuid,
    purpose: &str,
    sealed: &[u8],
) -> Result<Json, EngineError> {
    let bytes = crypto
        .decrypt(
            Binding {
                purpose,
                record: run_id,
            },
            sealed,
        )
        .await?;
    serde_json::from_slice(&bytes).map_err(|e| EngineError::Document(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_round_trip() {
        assert_eq!(step_key("a", &[]), "a");
        assert_eq!(step_key("inner", &[2, 0]), "inner[2][0]");
        assert_eq!(parse_indices("inner[2][0]"), vec![2, 0]);
        assert_eq!(parse_indices("a"), Vec::<usize>::new());
    }

    #[test]
    fn visibility_follows_iterations() {
        let mut journal = Journal::default();
        for (id, indices) in [("top", vec![]), ("body", vec![0]), ("body", vec![1])] {
            let mut r = Record::new(id, "transform", &indices, StepStatus::Succeeded);
            r.output = Some(json!(format!("{id}{indices:?}")));
            journal.put(r);
        }
        let from_iteration_1 = journal.visible(&[1]);
        assert_eq!(from_iteration_1["body"]["output"], json!("body[1]"));
        assert_eq!(from_iteration_1["top"]["output"], json!("top[]"));
        let from_outside = journal.visible(&[]);
        assert!(from_outside.get("body").is_none());
    }
}
