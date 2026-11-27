//! `/api/v1/ai` (design doc §10.1, ADR-031): an organization's AI provider
//! (bring your own key), a test call, and the log of AI calls. Needs
//! `ai:manage` at the organization.

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use huxley_ai::store::{self, CallView, ProviderInput, ProviderView, StoreError, Use};
use huxley_ai::{AiError, Purpose, Request};
use huxley_authz::permissions;
use huxley_db::{OrgScope, begin_scoped};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::authz::Authed;
use crate::{ApiError, ApiResult, AppState};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/provider",
            get(get_provider).put(set_provider).delete(remove_provider),
        )
        .route("/provider/test", post(test_provider))
        .route("/calls", get(list_calls))
}

fn db_err(e: impl std::fmt::Display) -> ApiError {
    ApiError::internal(&e)
}

fn map_err(e: StoreError) -> ApiError {
    match e {
        StoreError::Invalid(at, message) => ApiError::validation().with_field_error(at, message),
        StoreError::NotFound => ApiError::not_found(),
        StoreError::Db(e) => ApiError::internal(&e),
        StoreError::Crypto(e) => ApiError::internal(&e),
    }
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[serde(rename_all = "camelCase")]
pub struct OrgQuery {
    pub org_id: Uuid,
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[serde(rename_all = "camelCase")]
pub struct ProviderQuery {
    pub org_id: Uuid,
    /// Which provider: `chat` (the default) or `embeddings`.
    #[serde(default, rename = "use")]
    #[param(rename = "use", value_type = Option<Use>)]
    pub purpose: Option<Use>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = AiProviderState)]
pub struct ProviderState {
    /// This organization's own provider.
    pub own: Option<ProviderView>,
    /// The one it uses: its own, or the nearest above it.
    pub effective: Option<ProviderView>,
    /// Tokens the effective provider has used this month, across every
    /// organization using it.
    pub used_this_month: i64,
}

async fn state_for(state: &AppState, org_id: Uuid, purpose: Use) -> ApiResult<ProviderState> {
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    let own = store::own(&mut tx, org_id, purpose)
        .await
        .map_err(map_err)?;
    let effective = store::effective(&mut tx, org_id, purpose)
        .await
        .map_err(map_err)?;
    let used_this_month = match &effective {
        Some(p) => store::used_this_month(&mut tx, p.id)
            .await
            .map_err(map_err)?,
        None => 0,
    };
    tx.commit().await.map_err(db_err)?;
    Ok(ProviderState {
        own,
        effective,
        used_this_month,
    })
}

/// An organization's AI provider, or its embeddings provider (never its
/// key), and the one it uses.
#[utoipa::path(get, path = "/api/v1/ai/provider", operation_id = "get_ai_provider", tag = "ai",
    params(ProviderQuery), responses((status = 200, body = ProviderState)))]
pub async fn get_provider(
    State(state): State<AppState>,
    authed: Authed,
    Query(query): Query<ProviderQuery>,
) -> ApiResult<Json<ProviderState>> {
    authed
        .require_at(&state, permissions::AI_MANAGE, query.org_id)
        .await?;
    Ok(Json(
        state_for(&state, query.org_id, query.purpose.unwrap_or(Use::Chat)).await?,
    ))
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schema(as = SetAiProviderRequest)]
pub struct SetProviderRequest {
    pub org_id: Uuid,
    /// Which provider: `chat` (the default) or `embeddings`.
    #[serde(default, rename = "use")]
    #[schema(rename = "use")]
    pub purpose: Option<Use>,
    pub provider: ProviderInput,
}

/// Sets an organization's AI provider; organizations below without their
/// own use it too. The key is stored encrypted and never shown again.
#[utoipa::path(put, path = "/api/v1/ai/provider", operation_id = "set_ai_provider", tag = "ai",
    request_body = SetProviderRequest,
    responses((status = 200, body = ProviderState), (status = 422, body = ApiError, content_type = "application/problem+json")))]
pub async fn set_provider(
    State(state): State<AppState>,
    authed: Authed,
    Json(body): Json<SetProviderRequest>,
) -> ApiResult<Json<ProviderState>> {
    authed
        .require_at(&state, permissions::AI_MANAGE, body.org_id)
        .await?;
    // The org's first data key audits in its own transaction: make it first.
    state
        .crypto
        .encrypt(
            body.org_id,
            huxley_crypto::Binding {
                purpose: "ai_provider_probe",
                record: body.org_id,
            },
            b"",
        )
        .await
        .map_err(db_err)?;
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    store::set(
        &mut tx,
        &authed.user.ctx,
        &state.crypto,
        body.org_id,
        body.purpose.unwrap_or(Use::Chat),
        &body.provider,
    )
    .await
    .map_err(map_err)?;
    tx.commit().await.map_err(db_err)?;
    Ok(Json(
        state_for(&state, body.org_id, body.purpose.unwrap_or(Use::Chat)).await?,
    ))
}

/// Removes an organization's own AI provider (or embeddings provider).
#[utoipa::path(delete, path = "/api/v1/ai/provider", operation_id = "remove_ai_provider", tag = "ai",
    params(ProviderQuery), responses((status = 204), (status = 404, body = ApiError, content_type = "application/problem+json")))]
pub async fn remove_provider(
    State(state): State<AppState>,
    authed: Authed,
    Query(query): Query<ProviderQuery>,
) -> ApiResult<StatusCode> {
    authed
        .require_at(&state, permissions::AI_MANAGE, query.org_id)
        .await?;
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    store::remove(
        &mut tx,
        &authed.user.ctx,
        query.org_id,
        query.purpose.unwrap_or(Use::Chat),
    )
    .await
    .map_err(map_err)?;
    tx.commit().await.map_err(db_err)?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = AiProviderTest)]
pub struct TestResult {
    pub ok: bool,
    /// The model's reply, or what went wrong.
    pub message: String,
    pub model: Option<String>,
}

/// Sends a tiny request through the provider the organization uses (it's
/// logged and counts toward the budget). For embeddings, one word is
/// embedded and its dimensions reported.
#[utoipa::path(post, path = "/api/v1/ai/provider/test", operation_id = "test_ai_provider", tag = "ai",
    params(ProviderQuery), responses((status = 200, body = TestResult)))]
pub async fn test_provider(
    State(state): State<AppState>,
    authed: Authed,
    Query(query): Query<ProviderQuery>,
) -> ApiResult<Json<TestResult>> {
    authed
        .require_at(&state, permissions::AI_MANAGE, query.org_id)
        .await?;
    if query.purpose == Some(Use::Embeddings) {
        let purpose = Purpose {
            feature: "provider_test",
            run_id: None,
        };
        let result = state
            .engine
            .ai()
            .embed(
                query.org_id,
                purpose,
                &["ready".to_owned()],
                huxley_ai::embed::InputType::Query,
            )
            .await;
        return Ok(Json(match result {
            Ok(done) => TestResult {
                ok: true,
                message: format!("{} dimensions", done.vectors.first().map_or(0, Vec::len)),
                model: Some(done.model),
            },
            Err(e @ AiError::Internal(_)) => return Err(db_err(e)),
            Err(e) => TestResult {
                ok: false,
                message: e.to_string(),
                model: None,
            },
        }));
    }
    let request = Request {
        system: None,
        prompt: "Reply with the single word: ready".into(),
        schema: None,
        max_tokens: 1000,
        attachment: None,
    };
    let purpose = Purpose {
        feature: "provider_test",
        run_id: None,
    };
    Ok(Json(
        match state
            .engine
            .ai()
            .complete(query.org_id, purpose, None, &request)
            .await
        {
            Ok(done) => TestResult {
                ok: true,
                message: done.text,
                model: Some(done.model),
            },
            Err(e @ AiError::Internal(_)) => return Err(db_err(e)),
            Err(e) => TestResult {
                ok: false,
                message: e.to_string(),
                model: None,
            },
        },
    ))
}

/// Recent AI calls made for an organization.
#[utoipa::path(get, path = "/api/v1/ai/calls", operation_id = "list_ai_calls", tag = "ai",
    params(OrgQuery), responses((status = 200, body = Vec<CallView>)))]
pub async fn list_calls(
    State(state): State<AppState>,
    authed: Authed,
    Query(query): Query<OrgQuery>,
) -> ApiResult<Json<Vec<CallView>>> {
    authed
        .require_at(&state, permissions::AI_MANAGE, query.org_id)
        .await?;
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    let calls = store::calls(&mut tx, query.org_id, 100)
        .await
        .map_err(map_err)?;
    tx.commit().await.map_err(db_err)?;
    Ok(Json(calls))
}
