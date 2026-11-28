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
use huxley_triggers::webhook::{Incoming, delivery, verify};
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
    let mut recorded: Option<Vec<u8>> = None;
    if let Some(auth) = auth {
        // Checked without awaiting: the header lookup isn't `Send`.
        let (verified, signed) = {
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
            let verified = verify(&auth, &incoming);
            let signed = verified
                .is_ok()
                .then(|| delivery(&auth, &incoming))
                .flatten();
            (verified, signed)
        };
        if let Err(refused) = verified {
            tracing::info!(webhook = %public_id, reason = %refused, "webhook request refused");
            return Err(ApiError::new(
                StatusCode::UNAUTHORIZED,
                "webhook-unauthorized",
                "The request isn't signed or authenticated as this webhook requires",
            ));
        }
        if let Some((signed_at, digest)) = signed {
            let fresh =
                huxley_triggers::record_delivery(&state.engine, public_id, signed_at, &digest)
                    .await
                    .map_err(|e| ApiError::internal(&e))?;
            if !fresh {
                // Already received: a replay, or the sender retrying. Either
                // way it doesn't start another run.
                tracing::info!(webhook = %public_id, "repeated signed webhook delivery ignored");
                return Ok((StatusCode::OK, Json(json!({ "duplicate": true }))).into_response());
            }
            recorded = Some(digest);
        }
    }
    let response = accept(state, &webhook, query, headers, body).await;
    if response.is_err()
        && let Some(digest) = recorded
    {
        // Not accepted, so the sender's retry should be.
        if let Err(e) = huxley_triggers::forget_delivery(&state.engine, public_id, &digest).await {
            tracing::warn!(webhook = %public_id, error = %e, "couldn't forget a webhook delivery");
        }
    }
    response
}

/// Parses an authenticated request and starts the run.
async fn accept(
    state: &AppState,
    webhook: &huxley_triggers::Webhook,
    query: BTreeMap<String, String>,
    headers: &HeaderMap,
    body: &[u8],
) -> Result<Response, ApiError> {
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
    let run = huxley_triggers::start(&state.engine, webhook, trigger, inputs)
        .await
        .map_err(|e| match e {
            huxley_triggers::TriggerError::RunAsUnavailable(reason) => ApiError::new(
                StatusCode::CONFLICT,
                "run-as-unavailable",
                "The workflow can't run right now",
            )
            .with_detail(reason),
            other => ApiError::internal(&other),
        })?;

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

/// `/callbacks/{token}`: an external system calling back a parked
/// `wait_for_callback` step. The token is the credential; the URL works
/// once. Unknown, used and expired tokens all answer 404 alike.
pub async fn callback(
    State(state): State<AppState>,
    Path(token): Path<String>,
    method: axum::http::Method,
    Query(query): Query<BTreeMap<String, String>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let content_type = headers
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let body = if body.is_empty() {
        JsonValue::Null
    } else {
        serde_json::from_slice(&body)
            .unwrap_or_else(|_| JsonValue::String(String::from_utf8_lossy(&body).into_owned()))
    };
    let request = huxley_engine::CallbackRequest {
        method: method.to_string(),
        query: serde_json::to_value(query).unwrap_or(JsonValue::Null),
        content_type,
        body,
    };
    match state.engine.complete_callback(&token, request).await {
        Ok(true) => (StatusCode::OK, Json(json!({ "ok": true }))).into_response(),
        Ok(false) => ApiError::not_found().into_response(),
        Err(e) => ApiError::internal(&e).into_response(),
    }
}
