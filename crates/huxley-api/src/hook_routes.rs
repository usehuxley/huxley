//! `POST /hooks/{publicId}`: webhook triggers (design doc §6.5).
//!
//! Outside `/api/v1`: callers are other systems, authenticated by the
//! trigger's own scheme (HMAC, bearer, basic), not sessions or API tokens.
//! The public id is unguessable, but a webhook without `auth` accepts anyone
//! who has the URL.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use axum::Json;
use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use base64::Engine as _;
use huxley_db::{OrgScope, begin_scoped};
use huxley_engine::RunStatus;
use huxley_hwf::WebhookResponse;
use huxley_triggers::webhook::{Incoming, verify};
use serde_json::{Value as JsonValue, json};
use uuid::Uuid;

use crate::{ApiError, AppState};

/// Largest webhook body.
pub const MAX_BODY: usize = 1024 * 1024;
/// How long a sync webhook waits when the trigger doesn't say.
const DEFAULT_SYNC_WAIT: Duration = Duration::from_secs(30);

/// Headers not passed on to the workflow.
const HIDDEN_HEADERS: &[&str] = &["authorization", "cookie", "proxy-authorization"];

pub async fn receive(
    State(state): State<AppState>,
    Path(public_id): Path<String>,
    Query(query): Query<BTreeMap<String, String>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    match handle(&state, &public_id, query, &headers, &body).await {
        Ok(response) => response,
        Err(error) => error.into_response(),
    }
}

async fn handle(
    state: &AppState,
    public_id: &str,
    query: BTreeMap<String, String>,
    headers: &HeaderMap,
    body: &[u8],
) -> Result<Response, ApiError> {
    let webhook = huxley_triggers::find_webhook(&state.engine, public_id)
        .await
        .map_err(|e| ApiError::internal(&e))?
        .ok_or_else(ApiError::not_found)?;

    let auth = huxley_triggers::resolve_auth(&state.engine, &state.crypto, &webhook)
        .await
        .map_err(|e| ApiError::internal(&e))?;
    if let Some(auth) = auth {
        let lookup = |name: &str| {
            headers
                .get(name)
                .and_then(|v| v.to_str().ok())
                .map(str::to_owned)
        };
        let incoming = Incoming {
            header: &lookup,
            body,
            now: time::OffsetDateTime::now_utc().unix_timestamp(),
        };
        if let Err(refused) = verify(&auth, &incoming) {
            tracing::info!(webhook = %public_id, reason = %refused, "webhook request refused");
            return Err(ApiError::new(
                StatusCode::UNAUTHORIZED,
                "webhook-unauthorized",
                "The request isn't signed or authenticated as this webhook requires",
            ));
        }
    }

    let is_json = headers
        .get(http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|t| t.contains("json"));
    let payload = if body.is_empty() {
        JsonValue::Null
    } else if is_json {
        serde_json::from_slice(body)
            .map_err(|e| ApiError::bad_request(format!("the body isn't valid JSON: {e}")))?
    } else {
        match std::str::from_utf8(body) {
            Ok(text) => JsonValue::String(text.to_owned()),
            Err(_) => json!({ "base64": base64::engine::general_purpose::STANDARD.encode(body) }),
        }
    };
    let passed_headers: BTreeMap<String, String> = headers
        .iter()
        .filter(|(name, _)| !HIDDEN_HEADERS.contains(&name.as_str()))
        .map(|(name, value)| {
            (
                name.as_str().to_owned(),
                String::from_utf8_lossy(value.as_bytes()).into_owned(),
            )
        })
        .collect();
    let inputs = if payload.is_object() {
        payload.clone()
    } else {
        json!({})
    };
    huxley_workflows::check_inputs(&webhook.document, &inputs).map_err(|e| match e {
        huxley_workflows::WorkflowError::InvalidInputs(problems) => problems.into_iter().fold(
            ApiError::new(
                StatusCode::UNPROCESSABLE_ENTITY,
                "inputs-invalid",
                "The body doesn't match the workflow's input schema",
            ),
            |error, p| error.with_field_error("body", p),
        ),
        other => ApiError::internal(&other),
    })?;
    let trigger = json!({
        "id": webhook.trigger_id,
        "headers": passed_headers,
        "query": query,
        "body": payload,
    });
    let run = huxley_triggers::start(&state.engine, &webhook, trigger, inputs)
        .await
        .map_err(|e| ApiError::internal(&e))?;

    if webhook.config.response == WebhookResponse::Async {
        return Ok((StatusCode::ACCEPTED, Json(json!({ "runId": run }))).into_response());
    }
    let wait = webhook
        .config
        .timeout
        .as_deref()
        .and_then(huxley_hwf::duration::parse)
        .unwrap_or(DEFAULT_SYNC_WAIT);
    wait_for(state, run, wait).await
}

/// Waits for a sync webhook's run: its outputs, its error, or "still running".
async fn wait_for(state: &AppState, run: Uuid, wait: Duration) -> Result<Response, ApiError> {
    let deadline = Instant::now() + wait;
    loop {
        let mut tx = begin_scoped(&state.db, &OrgScope::System)
            .await
            .map_err(|e| ApiError::internal(&e))?;
        let (status, outputs, error): (RunStatus, Option<JsonValue>, Option<JsonValue>) =
            sqlx::query_as("SELECT status, outputs, error FROM runs WHERE id = $1")
                .bind(run)
                .fetch_one(&mut *tx)
                .await
                .map_err(|e| ApiError::internal(&e))?;
        tx.commit().await.map_err(|e| ApiError::internal(&e))?;
        match status {
            RunStatus::Succeeded => {
                return Ok(
                    (StatusCode::OK, Json(outputs.unwrap_or_else(|| json!({})))).into_response()
                );
            }
            RunStatus::Failed | RunStatus::Cancelled => {
                let body = json!({ "runId": run, "status": status, "error": error });
                return Ok((StatusCode::INTERNAL_SERVER_ERROR, Json(body)).into_response());
            }
            RunStatus::Queued | RunStatus::Running => {}
        }
        if Instant::now() >= deadline {
            let body = json!({ "runId": run, "status": "running" });
            return Ok((StatusCode::ACCEPTED, Json(body)).into_response());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}
