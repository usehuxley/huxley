//! `wait_for_callback` (design doc §6.4): the step records its URL (a hash
//! of the token), tells the other system through `send`, and parks until
//! the URL is called or it expires.

use huxley_db::{OrgScope, begin_scoped};
use serde_json::{Value as Json, json};
use sha2::{Digest, Sha256};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use uuid::Uuid;

use crate::journal::Record;
use crate::step::Executed;
use crate::walk::error;
use crate::{Engine, EngineResult, queue};

fn hash(token: &str) -> Vec<u8> {
    Sha256::digest(token.as_bytes()).to_vec()
}

/// What a callback hands the step: the request that came in.
#[derive(Debug, Clone)]
pub struct CallbackRequest {
    pub method: String,
    pub query: Json,
    pub content_type: Option<String>,
    pub body: Json,
}

impl Engine {
    /// The step job of a `wait_for_callback` step.
    pub(crate) async fn park_callback(
        &self,
        run_id: Uuid,
        org_id: Uuid,
        record: &Record,
        input: &Json,
    ) -> EngineResult<Executed> {
        let token = input["token"].as_str().unwrap_or_default();
        let Some(expires_at) = input["expires_at"]
            .as_str()
            .and_then(|t| OffsetDateTime::parse(t, &Rfc3339).ok())
        else {
            return Ok(Executed::Failed(error(
                "definition",
                "the callback has no expiry",
            )));
        };
        let mut tx = begin_scoped(self.db(), &OrgScope::System).await?;
        // A retried attempt keeps its input, and so the same token: move the
        // callback onto this attempt.
        sqlx::query(
            "INSERT INTO callbacks (token_hash, run_id, org_id, step_key, attempt, expires_at) \
             VALUES ($1, $2, $3, $4, $5, $6) \
             ON CONFLICT (token_hash) DO UPDATE SET attempt = EXCLUDED.attempt \
             WHERE callbacks.used_at IS NULL",
        )
        .bind(hash(token))
        .bind(run_id)
        .bind(org_id)
        .bind(&record.step_key)
        .bind(record.attempt)
        .bind(expires_at)
        .execute(&mut *tx)
        .await?;
        let (sent_at,): (Option<OffsetDateTime>,) =
            sqlx::query_as("SELECT sent_at FROM callbacks WHERE token_hash = $1")
                .bind(hash(token))
                .fetch_one(&mut *tx)
                .await?;
        tx.commit().await?;

        if OffsetDateTime::now_utc() >= expires_at {
            return Ok(Executed::Failed(error(
                "timeout",
                format!(
                    "nobody called back before {}",
                    expires_at.format(&Rfc3339).unwrap_or_default()
                ),
            )));
        }
        // Tell the other system, once (again only if a crash stopped us
        // recording that it was sent).
        if sent_at.is_none() {
            let send = &input["send"];
            let outcome = if send["action"].is_string() {
                let call = json!({
                    "action": send["action"],
                    "connection": send["connection"],
                    "inputs": send["with"],
                });
                self.call_action(run_id, org_id, &call).await?
            } else if send["request"].is_object() {
                crate::http::request(&self.egress, &send["request"]).await
            } else {
                Ok(Json::Null)
            };
            if let Err(failure) = outcome {
                return Ok(Executed::Failed(failure));
            }
            let mut tx = begin_scoped(self.db(), &OrgScope::System).await?;
            sqlx::query("UPDATE callbacks SET sent_at = now() WHERE token_hash = $1")
                .bind(hash(token))
                .execute(&mut *tx)
                .await?;
            // Shown as waiting while it's parked.
            sqlx::query(
                "UPDATE step_runs SET status = 'waiting' \
                  WHERE run_id = $1 AND step_key = $2 AND attempt = $3 AND status = 'running'",
            )
            .bind(run_id)
            .bind(&record.step_key)
            .bind(record.attempt)
            .execute(&mut *tx)
            .await?;
            tx.commit().await?;
        }
        // Wake at the expiry to fail the step if nobody called.
        Ok(Executed::NotYet(expires_at))
    }

    /// A call to a callback URL: completes the parked step with the request
    /// and moves its run on. False if the token is unknown, used, expired,
    /// or its step is no longer waiting: the caller answers the same way
    /// whichever it was.
    pub async fn complete_callback(
        &self,
        token: &str,
        request: CallbackRequest,
    ) -> EngineResult<bool> {
        let mut tx = begin_scoped(self.db(), &OrgScope::System).await?;
        let found: Option<(Uuid, Uuid, String, i32)> = sqlx::query_as(
            "SELECT run_id, org_id, step_key, attempt FROM callbacks \
              WHERE token_hash = $1 AND used_at IS NULL AND expires_at > now() FOR UPDATE",
        )
        .bind(hash(token))
        .fetch_optional(&mut *tx)
        .await?;
        let Some((run_id, org_id, step_key, attempt)) = found else {
            return Ok(false);
        };
        let output = json!({
            "method": request.method,
            "query": request.query,
            "content_type": request.content_type,
            "body": request.body,
            "received_at": OffsetDateTime::now_utc().format(&Rfc3339).unwrap_or_default(),
        });
        let (output, output_ref) = self.payloads.store(org_id, run_id, Some(&output)).await?;
        let updated = sqlx::query(
            "UPDATE step_runs SET status = 'succeeded', output = $4, output_ref = $5, \
                    finished_at = now() \
              WHERE run_id = $1 AND step_key = $2 AND attempt = $3 \
                AND status IN ('running', 'waiting')",
        )
        .bind(run_id)
        .bind(&step_key)
        .bind(attempt)
        .bind(output)
        .bind(output_ref)
        .execute(&mut *tx)
        .await?;
        if updated.rows_affected() == 0 {
            return Ok(false);
        }
        sqlx::query("UPDATE callbacks SET used_at = now() WHERE token_hash = $1")
            .bind(hash(token))
            .execute(&mut *tx)
            .await?;
        queue::enqueue_advance(&mut tx, org_id, run_id).await?;
        tx.commit().await?;
        Ok(true)
    }
}
