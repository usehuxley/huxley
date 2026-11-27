//! Step jobs: executing one step with side effects (ADR-021).
//!
//! The step's input was evaluated and journaled when it was scheduled, so
//! execution doesn't re-evaluate anything. The result is recorded in one
//! transaction with the job's removal (or its rescheduling for a retry).

use std::time::Duration;

use huxley_db::{OrgScope, begin_scoped};
use huxley_hwf::{Backoff, RetryPolicy, Spec, Step, Workflow};
use serde_json::{Value as Json, json};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use uuid::Uuid;

use crate::journal::{Record, StepStatus};
use crate::queue::{self, Job};
use crate::run::RunStatus;
use crate::walk::error;
use crate::{Engine, EngineError, EngineResult};

/// How long a step may run when it doesn't set `timeout`.
const DEFAULT_STEP_TIMEOUT: Duration = Duration::from_secs(300);

#[derive(sqlx::FromRow)]
struct RunState {
    org_id: Uuid,
    status: RunStatus,
    cancel_requested: bool,
    workflow_id: Option<Uuid>,
    workflow_version: Option<i32>,
    document: Option<Json>,
}

/// Finds a step anywhere in the workflow by id.
fn find<'a>(spec: &'a Spec, id: &str) -> Option<&'a Step> {
    fn search<'a>(steps: &'a [Step], id: &str) -> Option<&'a Step> {
        steps.iter().find_map(|step| {
            if step.id() == id {
                return Some(step);
            }
            step.blocks()
                .into_iter()
                .find_map(|(_, block)| search(block, id))
        })
    }
    search(&spec.steps, id).or_else(|| search(&spec.on_error, id))
}

/// What executing a step came to.
pub(crate) enum Executed {
    Done(Json),
    Failed(Json),
    /// Not due yet (a `wait` claimed early): run again at this time.
    NotYet(OffsetDateTime),
    /// Work done in the recording transaction (so it happens exactly once).
    Notify(Json),
    Subworkflow(Json),
    /// Started something that will complete the step later (a subworkflow
    /// that is waited for): keep it running, record this as its output.
    Waiting(Json),
}

/// A step's output, or the failure it ended with.
impl From<Result<Json, Json>> for Executed {
    fn from(result: Result<Json, Json>) -> Self {
        match result {
            Ok(output) => Self::Done(output),
            Err(failure) => Self::Failed(failure),
        }
    }
}

impl Engine {
    pub(crate) async fn execute_step(&self, job: &Job, worker: &str) -> EngineResult<()> {
        let (Some(run_id), Some(key)) = (job.run_id, job.step_key.as_deref()) else {
            return Err(EngineError::Document(
                "step job without a run or step".into(),
            ));
        };

        // Read what to do, without holding locks while the step runs.
        let mut tx = begin_scoped(&self.db, &OrgScope::System).await?;
        let run: Option<RunState> = sqlx::query_as(
            "SELECT org_id, status, cancel_requested, workflow_id, workflow_version, document \
               FROM runs WHERE id = $1",
        )
        .bind(run_id)
        .fetch_optional(&mut *tx)
        .await?;
        let record = latest(&mut tx, run_id, key).await?;
        let document = match &run {
            Some(RunState {
                document: Some(document),
                ..
            }) => Some(document.clone()),
            Some(RunState {
                workflow_id: Some(workflow_id),
                workflow_version: Some(version),
                ..
            }) => sqlx::query_as::<_, (Json,)>(
                "SELECT document FROM workflow_versions WHERE workflow_id = $1 AND version = $2",
            )
            .bind(workflow_id)
            .bind(version)
            .fetch_optional(&mut *tx)
            .await?
            .map(|(d,)| d),
            _ => None,
        };
        tx.commit().await?;

        let runnable = run
            .as_ref()
            .is_some_and(|r| !r.status.is_finished() && !r.cancel_requested);
        let (Some(run), Some(mut record), Some(document), true) = (run, record, document, runnable)
        else {
            return self.settle(job, worker, None).await;
        };
        if record.status.is_settled() {
            return self.settle(job, worker, Some(run.org_id)).await;
        }
        record.hydrate(&self.crypto, &self.payloads, run_id).await?;
        let workflow: Workflow =
            serde_json::from_value(document).map_err(|e| EngineError::Document(e.to_string()))?;
        let Some(step) = find(&workflow.spec, &record.step_id) else {
            return self
                .record(
                    job,
                    worker,
                    run.org_id,
                    run_id,
                    &record,
                    Executed::Failed(error(
                        "definition",
                        format!("step {:?} isn't in the workflow", record.step_id),
                    )),
                    None,
                )
                .await;
        };

        let timeout = step
            .common()
            .timeout
            .and_then(huxley_hwf::duration::parse)
            .unwrap_or(DEFAULT_STEP_TIMEOUT);
        let input = record.input.clone().unwrap_or(Json::Null);
        let executed = match tokio::time::timeout(
            timeout,
            self.execute(step, &input, run_id, run.org_id, &record),
        )
        .await
        {
            Ok(executed) => executed,
            Err(_) => Executed::Failed(error(
                "timeout",
                format!("the step didn't finish within {}s", timeout.as_secs()),
            )),
        };
        let retry = step.common().retry.cloned();
        self.record(
            job,
            worker,
            run.org_id,
            run_id,
            &record,
            executed,
            retry.as_ref(),
        )
        .await
    }

    // A flat dispatch table: one arm per step type.
    #[allow(clippy::too_many_lines)]
    async fn execute(
        &self,
        step: &Step,
        input: &Json,
        run_id: Uuid,
        org_id: Uuid,
        record: &Record,
    ) -> Executed {
        match step {
            Step::Approval(_) => match self.park_approval(run_id, org_id, record, input).await {
                Ok(executed) => executed,
                Err(e) => Executed::Failed(error("internal", e.to_string())),
            },
            Step::WaitForCallback(_) => {
                match self.park_callback(run_id, org_id, record, input).await {
                    Ok(executed) => executed,
                    Err(e) => Executed::Failed(error("internal", e.to_string())),
                }
            }
            Step::Script(_) => {
                let timeout = step
                    .common()
                    .timeout
                    .and_then(huxley_hwf::duration::parse)
                    .unwrap_or(crate::script::DEFAULT_TIMEOUT);
                match self
                    .park_script(run_id, org_id, record, input, timeout)
                    .await
                {
                    Ok(executed) => executed,
                    Err(e) => Executed::Failed(error("internal", e.to_string())),
                }
            }
            Step::HttpRequest(_) => crate::http::request(&self.egress, input).await.into(),
            Step::IntegrationAction(_) => match self.call_action(run_id, org_id, input).await {
                Ok(Ok(output)) => Executed::Done(output),
                // Over the rate limit: wait for the next window (not a retry).
                Ok(Err(failure)) if failure["kind"] == "rate_limited" => {
                    let at = failure["retry_at"]
                        .as_str()
                        .and_then(|t| OffsetDateTime::parse(t, &Rfc3339).ok())
                        .unwrap_or_else(OffsetDateTime::now_utc);
                    Executed::NotYet(at)
                }
                Ok(Err(failure)) => Executed::Failed(failure),
                Err(e) => Executed::Failed(error("internal", e.to_string())),
            },
            Step::Notify(_) => Executed::Notify(input.clone()),
            Step::Email(_) => self.send_email(input).await.into(),
            Step::DatasetQuery(_) => self.query_dataset(org_id, input).await.into(),
            Step::ContactFind(_)
            | Step::ContactUpsert(_)
            | Step::CompanyGet(_)
            | Step::CompanyUpdate(_) => self
                .crm_step(step.type_name(), org_id, input, record.tainted)
                .await
                .into(),
            Step::TicketCreate(_)
            | Step::TicketGet(_)
            | Step::TicketFind(_)
            | Step::TicketUpdate(_)
            | Step::TicketNote(_)
            | Step::TicketDispatch(_) => self
                .ticket_step(step.type_name(), org_id, input, record.tainted)
                .await
                .into(),
            Step::KbEmbed(_) => self.embed_articles(org_id, input).await.into(),
            Step::TimesheetCheck(_) => self.check_timesheets(input).await.into(),
            Step::CsatSurvey(_) => self.csat_survey(org_id, input).await.into(),
            Step::NpsSend(_) => self.nps_send(org_id, input).await.into(),
            Step::HealthCompute(_) => self.health_compute(org_id).await.into(),
            Step::SentimentRecord(_) => self.sentiment_record(org_id, input).await.into(),
            Step::TriageOptions(_) => self.triage_options(org_id, input).await.into(),
            Step::TriageApply(_) => self.triage_apply(org_id, input).await.into(),
            Step::DispatchRecommend(_) => self.recommend_assignees(org_id, input).await.into(),
            Step::WorkforceInsights(_) => self.skill_gaps(org_id, input).await.into(),
            Step::TicketInsight(_) => self.ticket_insight(org_id, input).await.into(),
            Step::KbDraft(_) => self.kb_draft(org_id, input).await.into(),
            Step::TimeSuggest(_) => self.time_suggest(org_id, input).await.into(),
            Step::ReportSend(_) => self.report_send(org_id, input).await.into(),
            Step::ChangeDecide(_) => self.change_decide(org_id, input).await.into(),
            Step::ScopeDecide(_) => self.scope_decide(org_id, input).await.into(),
            Step::QuoteReview(_) => self.quote_review(org_id, input).await.into(),
            Step::SalesLogEmail(_) => self.sales_log_email(org_id, input).await.into(),
            Step::CalendarSync(_) => self.calendar_sync(input).await.into(),
            Step::CalendarWrite(_) => self.calendar_write(input).await.into(),
            Step::ProjectStatus(_) => self.project_status(org_id, input).await.into(),
            Step::ProjectReport(_) => self.project_report(org_id, input).await.into(),
            Step::AssetSync(_) => self.sync_assets(org_id, input).await.into(),
            Step::AssetWarranty(_) => self.set_warranty(org_id, input).await.into(),
            Step::MailFetch(_)
            | Step::TicketEmbed(_)
            | Step::TicketFromEmail(_)
            | Step::TicketEmailReply(_) => {
                self.desk_step(step.type_name(), org_id, input).await.into()
            }
            Step::DatasetUpsert(_) => self
                .upsert_dataset(org_id, input, record.tainted)
                .await
                .into(),
            Step::AiPrompt(_) | Step::AiClassify(_) | Step::AiExtract(_) => self
                .ai_step(step.type_name(), org_id, run_id, input, record.tainted)
                .await
                .into(),
            Step::Subworkflow(_) => Executed::Subworkflow(input.clone()),
            Step::Wait(_) => {
                let until = input["until"]
                    .as_str()
                    .and_then(|t| OffsetDateTime::parse(t, &Rfc3339).ok())
                    .unwrap_or_else(OffsetDateTime::now_utc);
                if until > OffsetDateTime::now_utc() {
                    Executed::NotYet(until)
                } else {
                    Executed::Done(json!({ "until": input["until"] }))
                }
            }
            other => Executed::Failed(error(
                "unsupported",
                format!("{} steps aren't available yet", other.type_name()),
            )),
        }
    }

    /// Does the work that belongs in the recording transaction (notifying,
    /// starting a subworkflow) and masks secrets a tainted step may echo.
    async fn in_transaction(
        &self,
        tx: &mut sqlx::PgConnection,
        org_id: Uuid,
        run_id: Uuid,
        attempted: &Record,
        executed: Executed,
    ) -> EngineResult<Executed> {
        let executed = match executed {
            Executed::Notify(input) => match crate::notify::send(&mut *tx, org_id, &input).await? {
                Ok(output) => Executed::Done(output),
                Err(failure) => Executed::Failed(failure),
            },
            Executed::Subworkflow(input) => {
                let started = crate::run::start_child(
                    self,
                    &mut *tx,
                    org_id,
                    run_id,
                    &attempted.step_key,
                    &input,
                )
                .await?;
                match started {
                    Ok(child) if input["wait"].as_bool() == Some(false) => {
                        Executed::Done(json!({ "run_id": child }))
                    }
                    Ok(child) => Executed::Waiting(json!({ "run_id": child })),
                    Err(failure) => Executed::Failed(failure),
                }
            }
            other => other,
        };
        // A step given a secret may echo it back (an API reflecting its
        // Authorization header): mask the org's secret values in what's kept.
        let executed = if attempted.tainted {
            let secrets = self.secret_values(&mut *tx, org_id, run_id).await?;
            let mask =
                |value: &Json| huxley_expr::redact(value, secrets.iter().map(String::as_str));
            match executed {
                Executed::Done(output) => Executed::Done(mask(&output)),
                Executed::Failed(failure) => Executed::Failed(mask(&failure)),
                other => other,
            }
        } else {
            executed
        };
        Ok(executed)
    }

    /// Removes a job that has nothing left to do, nudging the run along.
    async fn settle(&self, job: &Job, worker: &str, org_id: Option<Uuid>) -> EngineResult<()> {
        let mut tx = begin_scoped(&self.db, &OrgScope::System).await?;
        if let (Some(org_id), Some(run_id)) = (org_id, job.run_id) {
            queue::enqueue_advance(&mut tx, org_id, run_id).await?;
        }
        queue::complete(&mut tx, job.id, worker).await?;
        tx.commit().await?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    async fn record(
        &self,
        job: &Job,
        worker: &str,
        org_id: Uuid,
        run_id: Uuid,
        attempted: &Record,
        executed: Executed,
        retry: Option<&RetryPolicy>,
    ) -> EngineResult<()> {
        let mut tx = begin_scoped(&self.db, &OrgScope::System).await?;
        // Serialize with advances, then check nothing moved on meanwhile
        // (a cancellation, or another worker that re-ran this attempt).
        let status: Option<(RunStatus, bool)> =
            sqlx::query_as("SELECT status, cancel_requested FROM runs WHERE id = $1 FOR UPDATE")
                .bind(run_id)
                .fetch_optional(&mut *tx)
                .await?;
        let current = latest(&mut tx, run_id, &attempted.step_key).await?;
        let still_ours = status.is_some_and(|(s, cancel)| !s.is_finished() && !cancel)
            && current
                .as_ref()
                .is_some_and(|c| c.attempt == attempted.attempt && !c.status.is_settled());
        if !still_ours {
            queue::enqueue_advance(&mut tx, org_id, run_id).await?;
            queue::complete(&mut tx, job.id, worker).await?;
            tx.commit().await?;
            return Ok(());
        }

        let executed = self
            .in_transaction(&mut tx, org_id, run_id, attempted, executed)
            .await?;
        let mut settled = true;
        match executed {
            Executed::NotYet(at) => {
                queue::reschedule(&mut tx, job.id, worker, at).await?;
                tx.commit().await?;
                return Ok(());
            }
            Executed::Notify(_) | Executed::Subworkflow(_) => unreachable!("handled above"),
            Executed::Waiting(output) => {
                sqlx::query(
                    "UPDATE step_runs SET output = $4 \
                      WHERE run_id = $1 AND step_key = $2 AND attempt = $3",
                )
                .bind(run_id)
                .bind(&attempted.step_key)
                .bind(attempted.attempt)
                .bind(&output)
                .execute(&mut *tx)
                .await?;
                queue::complete(&mut tx, job.id, worker).await?;
                tx.commit().await?;
                return Ok(());
            }
            Executed::Done(output) => {
                let (output, output_ref) =
                    self.payloads.store(org_id, run_id, Some(&output)).await?;
                set_result(
                    &mut tx,
                    run_id,
                    attempted,
                    StepStatus::Succeeded,
                    (output.as_ref(), output_ref.as_deref()),
                    None,
                )
                .await?;
            }
            Executed::Failed(failure) => {
                set_result(
                    &mut tx,
                    run_id,
                    attempted,
                    StepStatus::Failed,
                    (None, None),
                    Some(&failure),
                )
                .await?;
                if let Some(policy) = retry
                    && should_retry(policy, &failure, attempted.attempt)
                {
                    let delay = retry_delay(policy, attempted.attempt);
                    next_attempt(&mut tx, run_id, attempted).await?;
                    queue::reschedule(&mut tx, job.id, worker, OffsetDateTime::now_utc() + delay)
                        .await?;
                    settled = false;
                }
            }
        }
        if settled {
            queue::enqueue_advance(&mut tx, org_id, run_id).await?;
            queue::complete(&mut tx, job.id, worker).await?;
        }
        tx.commit().await?;
        Ok(())
    }
}

async fn latest(
    conn: &mut sqlx::PgConnection,
    run_id: Uuid,
    key: &str,
) -> sqlx::Result<Option<Record>> {
    sqlx::query_as(
        "SELECT step_key, step_id, step_type, attempt, status, input, output, error, tainted, \
                input_sealed, output_tainted, output_sealed, input_ref, output_ref \
           FROM step_runs WHERE run_id = $1 AND step_key = $2 AND NOT superseded \
          ORDER BY attempt DESC LIMIT 1",
    )
    .bind(run_id)
    .bind(key)
    .fetch_optional(conn)
    .await
}

async fn set_result(
    conn: &mut sqlx::PgConnection,
    run_id: Uuid,
    record: &Record,
    status: StepStatus,
    (output, output_ref): (Option<&Json>, Option<&str>),
    error: Option<&Json>,
) -> sqlx::Result<()> {
    sqlx::query(
        "UPDATE step_runs SET status = $4, output = $5, error = $6, output_ref = $7, \
                finished_at = now() \
          WHERE run_id = $1 AND step_key = $2 AND attempt = $3",
    )
    .bind(run_id)
    .bind(&record.step_key)
    .bind(record.attempt)
    .bind(status)
    .bind(output)
    .bind(error)
    .bind(output_ref)
    .execute(conn)
    .await
    .map(|_| ())
}

/// The next attempt, with the same (already evaluated, possibly sealed)
/// input.
async fn next_attempt(
    conn: &mut sqlx::PgConnection,
    run_id: Uuid,
    record: &Record,
) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO step_runs (run_id, step_key, attempt, org_id, step_id, step_type, status, \
                input, tainted, input_sealed) \
         SELECT run_id, step_key, attempt + 1, org_id, step_id, step_type, 'running', \
                input, tainted, input_sealed \
           FROM step_runs WHERE run_id = $1 AND step_key = $2 AND attempt = $3",
    )
    .bind(run_id)
    .bind(&record.step_key)
    .bind(record.attempt)
    .execute(conn)
    .await
    .map(|_| ())
}

/// Whether a failure matches the policy and attempts remain.
fn should_retry(policy: &RetryPolicy, failure: &Json, attempt: i32) -> bool {
    if i64::from(attempt) >= i64::from(policy.max_attempts) {
        return false;
    }
    let kind = failure["kind"].as_str().unwrap_or_default();
    // Mistakes in the workflow fail the same way every time.
    if matches!(
        kind,
        "expression" | "definition" | "secret" | "unsupported" | "egress" | "limit"
    ) {
        return false;
    }
    if policy.on.is_empty() {
        return true;
    }
    let status = failure["status"].as_u64();
    policy.on.iter().any(|on| match on.as_str() {
        "timeout" => kind == "timeout",
        "network" => kind == "network",
        "4xx" => status.is_some_and(|s| (400..500).contains(&s)),
        "5xx" => status.is_some_and(|s| (500..600).contains(&s)),
        code => status.is_some_and(|s| code.parse::<u64>() == Ok(s)),
    })
}

/// The delay before the attempt after `attempt`.
fn retry_delay(policy: &RetryPolicy, attempt: i32) -> Duration {
    let base = policy
        .delay
        .as_deref()
        .and_then(huxley_hwf::duration::parse)
        .unwrap_or(Duration::from_secs(1));
    let cap = policy
        .max_delay
        .as_deref()
        .and_then(huxley_hwf::duration::parse)
        .unwrap_or(Duration::from_secs(300));
    let exponent = u32::try_from(attempt.saturating_sub(1).clamp(0, 30)).unwrap_or(0);
    let exponential = base.saturating_mul(2u32.saturating_pow(exponent)).min(cap);
    match policy.backoff {
        Backoff::Fixed => base.min(cap),
        Backoff::Exponential => exponential,
        Backoff::Jitter => {
            let mut byte = [0u8; 2];
            getrandom::fill(&mut byte).expect("the operating system RNG is available");
            exponential.mul_f64(f64::from(u16::from_le_bytes(byte)) / f64::from(u16::MAX))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy(on: &[&str], backoff: Backoff) -> RetryPolicy {
        RetryPolicy {
            max_attempts: 4,
            backoff,
            delay: Some("2s".into()),
            max_delay: Some("10s".into()),
            on: on.iter().map(|s| (*s).to_owned()).collect(),
        }
    }

    #[test]
    fn retries_match_the_policy() {
        let http = |status: u64| json!({ "kind": "http", "status": status });
        let p = policy(&["429", "5xx"], Backoff::Exponential);
        assert!(should_retry(&p, &http(503), 1));
        assert!(should_retry(&p, &http(429), 3));
        assert!(!should_retry(&p, &http(429), 4), "out of attempts");
        assert!(!should_retry(&p, &http(404), 1));
        assert!(!should_retry(&p, &json!({ "kind": "timeout" }), 1));
        let any = policy(&[], Backoff::Fixed);
        assert!(should_retry(&any, &json!({ "kind": "timeout" }), 1));
        assert!(!should_retry(&any, &json!({ "kind": "expression" }), 1));
    }

    #[test]
    fn backoff_grows_and_is_capped() {
        let p = policy(&[], Backoff::Exponential);
        let secs: Vec<u64> = (1..=4).map(|a| retry_delay(&p, a).as_secs()).collect();
        assert_eq!(secs, [2, 4, 8, 10]);
        assert_eq!(retry_delay(&policy(&[], Backoff::Fixed), 3).as_secs(), 2);
        assert!(retry_delay(&policy(&[], Backoff::Jitter), 3) <= Duration::from_secs(8));
    }
}
