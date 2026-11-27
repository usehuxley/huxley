//! OAuth 2.1 for MCP clients (ADR-032), per the MCP authorization spec.
//!
//! - Discovery: `/.well-known/oauth-protected-resource[/mcp]` (RFC 9728)
//!   and `/.well-known/oauth-authorization-server` (RFC 8414).
//! - `/oauth/register` (RFC 7591), `/oauth/token`, `/oauth/revoke` (RFC 7009):
//!   outside `/api/v1` (no session, no CSRF header), open to any origin.
//! - The authorization endpoint is the web app's `/oauth/authorize` page. It
//!   asks `/api/v1/oauth/authorization` (signed-in session only) to check
//!   the request and, once the person decides, for the URL to send them to.
//! - `/api/v1/auth/connected-apps`: your connected apps, to disconnect.
//!
//! All of it needs `auth.public_url`: the issuer and the resource are URLs.

use axum::extract::{FromRequestParts, Path, Request, State};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Form, Json, Router};
use huxley_auth::oauth::{self, Approval, CodeExchange, OAuthError};
use huxley_authz::permissions;
use huxley_db::{OrgScope, begin_scoped};
use serde::{Deserialize, Serialize};
use serde_json::json;
use time::OffsetDateTime;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::authz::Authed;
use crate::context::ClientInfo;
use crate::session::CurrentUser;
use crate::{ApiError, ApiResult, AppState};

/// The scope meaning "everything your roles allow" (a token scope of `*`).
pub const FULL_SCOPE: &str = "mcp";

/// The issuer: `auth.public_url` without a trailing slash.
pub fn issuer(state: &AppState) -> Option<String> {
    state
        .auth
        .config()
        .public_url
        .as_deref()
        .map(|u| u.trim_end_matches('/').to_owned())
}

/// The protected resource: the MCP endpoint's URL.
pub fn resource(state: &AppState) -> Option<String> {
    issuer(state).map(|i| format!("{i}/mcp"))
}

/// Where clients find the protected resource metadata (for `/mcp`'s 401).
pub fn resource_metadata_url(state: &AppState) -> Option<String> {
    issuer(state).map(|i| format!("{i}/.well-known/oauth-protected-resource/mcp"))
}

/// The routes outside `/api/v1`.
pub fn public_routes() -> Router<AppState> {
    Router::new()
        .route(
            "/.well-known/oauth-protected-resource",
            get(protected_resource),
        )
        .route(
            "/.well-known/oauth-protected-resource/mcp",
            get(protected_resource),
        )
        .route(
            "/.well-known/oauth-authorization-server",
            get(authorization_server),
        )
        .route("/oauth/register", post(register))
        .route("/oauth/token", post(token))
        .route("/oauth/revoke", post(revoke))
        .layer(axum::middleware::from_fn(open_cors))
}

/// `/api/v1/oauth`: the consent page's API.
pub fn consent_routes() -> Router<AppState> {
    Router::new().route("/authorization", post(authorization))
}

/// `/api/v1/auth/connected-apps`.
pub fn connected_app_routes() -> Router<AppState> {
    Router::new()
        .route("/", get(list_connected))
        .route("/{id}", delete(disconnect))
}

/// Browser-based MCP clients call these from their own origin. Nothing here
/// uses cookies, so any origin may.
async fn open_cors(request: Request, next: Next) -> Response {
    let preflight = request.method() == Method::OPTIONS;
    let mut response = if preflight {
        StatusCode::NO_CONTENT.into_response()
    } else {
        next.run(request).await
    };
    let headers = response.headers_mut();
    headers.insert(
        header::ACCESS_CONTROL_ALLOW_ORIGIN,
        HeaderValue::from_static("*"),
    );
    if preflight {
        headers.insert(
            header::ACCESS_CONTROL_ALLOW_METHODS,
            HeaderValue::from_static("GET, POST, OPTIONS"),
        );
        headers.insert(
            header::ACCESS_CONTROL_ALLOW_HEADERS,
            HeaderValue::from_static("authorization, content-type, mcp-protocol-version"),
        );
        headers.insert(
            header::ACCESS_CONTROL_MAX_AGE,
            HeaderValue::from_static("86400"),
        );
    }
    response
}

fn not_configured() -> Response {
    oauth_error(
        StatusCode::NOT_FOUND,
        "temporarily_unavailable",
        "OAuth isn't available: the server has no auth.public_url.",
    )
}

fn oauth_error(status: StatusCode, code: &str, description: &str) -> Response {
    (
        status,
        [(header::CACHE_CONTROL, "no-store")],
        Json(json!({ "error": code, "error_description": description })),
    )
        .into_response()
}

fn from_oauth(e: &OAuthError) -> Response {
    if e.is_internal() {
        tracing::error!(error = %e, "OAuth request failed");
        return oauth_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "server_error",
            "Something went wrong.",
        );
    }
    let status = if matches!(e, OAuthError::InvalidClient) {
        StatusCode::UNAUTHORIZED
    } else {
        StatusCode::BAD_REQUEST
    };
    oauth_error(status, e.code(), &e.to_string())
}

fn supported_scopes() -> Vec<&'static str> {
    std::iter::once(FULL_SCOPE)
        .chain(permissions::ALL.iter().map(|p| p.key))
        .collect()
}

/// `GET /.well-known/oauth-protected-resource[/mcp]` (RFC 9728).
async fn protected_resource(State(state): State<AppState>) -> Response {
    let (Some(issuer), Some(resource)) = (issuer(&state), resource(&state)) else {
        return not_configured();
    };
    Json(json!({
        "resource": resource,
        "authorization_servers": [issuer],
        "scopes_supported": [FULL_SCOPE],
        "bearer_methods_supported": ["header"],
        "resource_name": "Huxley",
        "resource_documentation": "https://docs.usehuxley.io/mcp",
    }))
    .into_response()
}

/// `GET /.well-known/oauth-authorization-server` (RFC 8414).
async fn authorization_server(State(state): State<AppState>) -> Response {
    let Some(issuer) = issuer(&state) else {
        return not_configured();
    };
    Json(json!({
        "issuer": issuer,
        "authorization_endpoint": format!("{issuer}/oauth/authorize"),
        "token_endpoint": format!("{issuer}/oauth/token"),
        "registration_endpoint": format!("{issuer}/oauth/register"),
        "revocation_endpoint": format!("{issuer}/oauth/revoke"),
        "scopes_supported": supported_scopes(),
        "response_types_supported": ["code"],
        "response_modes_supported": ["query"],
        "grant_types_supported": ["authorization_code", "refresh_token"],
        "token_endpoint_auth_methods_supported": ["none"],
        "revocation_endpoint_auth_methods_supported": ["none"],
        "code_challenge_methods_supported": ["S256"],
        "authorization_response_iss_parameter_supported": true,
        "service_documentation": "https://docs.usehuxley.io/mcp",
    }))
    .into_response()
}

async fn client_info(state: &AppState, headers: HeaderMap) -> huxley_core::RequestContext {
    let (mut parts, ()) = Request::new(()).into_parts();
    parts.headers = headers;
    let ClientInfo(ctx) = ClientInfo::from_request_parts(&mut parts, state)
        .await
        .unwrap_or_else(|never| match never {});
    ctx
}

async fn rate_limited(state: &AppState, ctx: &huxley_core::RequestContext) -> Option<Response> {
    let ip = ctx.ip?;
    (!state.oauth_limiter.check(ip).await).then(|| {
        oauth_error(
            StatusCode::TOO_MANY_REQUESTS,
            "slow_down",
            "Too many requests; try again in a minute.",
        )
    })
}

/// Client metadata (RFC 7591). Only the fields Huxley uses; the rest of
/// what clients send is ignored.
#[derive(Debug, Deserialize)]
struct Registration {
    redirect_uris: Option<Vec<String>>,
    client_name: Option<String>,
    grant_types: Option<Vec<String>>,
    response_types: Option<Vec<String>>,
}

/// `POST /oauth/register`: dynamic client registration. Every client is a
/// public client (`token_endpoint_auth_method: none`), whatever it asked for.
async fn register(State(state): State<AppState>, headers: HeaderMap, body: String) -> Response {
    if issuer(&state).is_none() {
        return not_configured();
    }
    let ctx = client_info(&state, headers).await;
    if let Some(limited) = rate_limited(&state, &ctx).await {
        return limited;
    }
    let Ok(metadata) = serde_json::from_str::<Registration>(&body) else {
        return oauth_error(
            StatusCode::BAD_REQUEST,
            "invalid_client_metadata",
            "The body must be a JSON object of client metadata.",
        );
    };
    let grant_types = metadata
        .grant_types
        .unwrap_or_else(|| vec!["authorization_code".into()]);
    if let Some(bad) = grant_types
        .iter()
        .find(|g| !matches!(g.as_str(), "authorization_code" | "refresh_token"))
    {
        return from_oauth(&OAuthError::InvalidClientMetadata(format!(
            "grant type {bad:?} isn't supported"
        )));
    }
    if let Some(bad) = metadata
        .response_types
        .iter()
        .flatten()
        .find(|r| r.as_str() != "code")
    {
        return from_oauth(&OAuthError::InvalidClientMetadata(format!(
            "response type {bad:?} isn't supported"
        )));
    }
    let name = metadata
        .client_name
        .filter(|n| !n.trim().is_empty())
        .unwrap_or_else(|| "Unnamed app".into());
    let redirect_uris = metadata.redirect_uris.unwrap_or_default();
    let result = async {
        let mut tx = begin_scoped(&state.db, &OrgScope::System).await?;
        let client = oauth::register_client(&mut tx, &name, &redirect_uris, ctx.ip).await?;
        tx.commit().await?;
        Ok::<_, OAuthError>(client)
    }
    .await;
    match result {
        Ok(client) => (
            StatusCode::CREATED,
            [(header::CACHE_CONTROL, "no-store")],
            Json(json!({
                "client_id": client.client_id,
                "client_id_issued_at": client.created_at.unix_timestamp(),
                "client_name": client.name,
                "redirect_uris": client.redirect_uris,
                "grant_types": ["authorization_code", "refresh_token"],
                "response_types": ["code"],
                "token_endpoint_auth_method": "none",
            })),
        )
            .into_response(),
        Err(e) => from_oauth(&e),
    }
}

/// The token request (`application/x-www-form-urlencoded`).
#[derive(Debug, Deserialize)]
struct TokenRequest {
    grant_type: Option<String>,
    code: Option<String>,
    redirect_uri: Option<String>,
    code_verifier: Option<String>,
    client_id: Option<String>,
    refresh_token: Option<String>,
    resource: Option<String>,
}

/// The client id from the body, or from HTTP Basic (some clients send it
/// that way, with an empty secret).
fn client_id_of(body_client_id: Option<String>, headers: &HeaderMap) -> Option<String> {
    use base64::Engine as _;
    body_client_id.filter(|c| !c.is_empty()).or_else(|| {
        let basic = headers
            .get(header::AUTHORIZATION)?
            .to_str()
            .ok()?
            .strip_prefix("Basic ")?;
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(basic.trim())
            .ok()?;
        let decoded = String::from_utf8(decoded).ok()?;
        let (id, _) = decoded.split_once(':')?;
        Some(id.to_owned()).filter(|id| !id.is_empty())
    })
}

/// `POST /oauth/token`: the authorization-code and refresh-token grants.
async fn token(
    State(state): State<AppState>,
    headers: HeaderMap,
    form: Result<Form<TokenRequest>, axum::extract::rejection::FormRejection>,
) -> Response {
    if issuer(&state).is_none() {
        return not_configured();
    }
    let ctx = client_info(&state, headers.clone()).await;
    if let Some(limited) = rate_limited(&state, &ctx).await {
        return limited;
    }
    let Ok(Form(request)) = form else {
        return oauth_error(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "Send the request as application/x-www-form-urlencoded.",
        );
    };
    let Some(client_id) = client_id_of(request.client_id.clone(), &headers) else {
        return from_oauth(&OAuthError::InvalidRequest("client_id is required".into()));
    };
    let result = match request.grant_type.as_deref() {
        Some("authorization_code") => exchange(&state, &ctx, &client_id, &request).await,
        Some("refresh_token") => {
            let Some(refresh_token) = request.refresh_token.as_deref() else {
                return from_oauth(&OAuthError::InvalidRequest(
                    "refresh_token is required".into(),
                ));
            };
            in_tx(&state, async |tx| {
                oauth::refresh(tx, &ctx, &client_id, refresh_token).await
            })
            .await
        }
        _ => {
            return oauth_error(
                StatusCode::BAD_REQUEST,
                "unsupported_grant_type",
                "grant_type must be authorization_code or refresh_token.",
            );
        }
    };
    match result {
        Ok(tokens) => (
            [
                (header::CACHE_CONTROL, "no-store"),
                (header::PRAGMA, "no-cache"),
            ],
            Json(json!({
                "access_token": tokens.access_token,
                "token_type": "Bearer",
                "expires_in": tokens.expires_in,
                "refresh_token": tokens.refresh_token,
                "scope": scope_string(&tokens.scopes),
            })),
        )
            .into_response(),
        Err(e) => from_oauth(&e),
    }
}

async fn exchange(
    state: &AppState,
    ctx: &huxley_core::RequestContext,
    client_id: &str,
    request: &TokenRequest,
) -> Result<oauth::Tokens, OAuthError> {
    let (Some(code), Some(redirect_uri), Some(code_verifier)) = (
        request.code.as_deref(),
        request.redirect_uri.as_deref(),
        request.code_verifier.as_deref(),
    ) else {
        return Err(OAuthError::InvalidRequest(
            "code, redirect_uri and code_verifier are required".into(),
        ));
    };
    let exchange = CodeExchange {
        client_id,
        code,
        redirect_uri,
        code_verifier,
        resource: request.resource.as_deref().map(|r| r.trim_end_matches('/')),
    };
    in_tx(state, async |tx| {
        oauth::exchange_code(tx, ctx, &exchange).await
    })
    .await
}

/// Runs `f` in a transaction. Commits on success and on `invalid_grant`
/// (which writes only the revocations a reused secret triggers).
async fn in_tx<T>(
    state: &AppState,
    f: impl AsyncFnOnce(&mut sqlx::PgConnection) -> Result<T, OAuthError>,
) -> Result<T, OAuthError> {
    let mut tx = begin_scoped(&state.db, &OrgScope::System).await?;
    let result = f(&mut tx).await;
    if matches!(result, Ok(_) | Err(OAuthError::InvalidGrant(_))) {
        tx.commit().await?;
    }
    result
}

/// The token revocation request.
#[derive(Debug, Deserialize)]
struct RevokeRequest {
    token: Option<String>,
    client_id: Option<String>,
}

/// `POST /oauth/revoke` (RFC 7009). Always 200, so it reveals nothing.
async fn revoke(
    State(state): State<AppState>,
    headers: HeaderMap,
    form: Result<Form<RevokeRequest>, axum::extract::rejection::FormRejection>,
) -> Response {
    let ctx = client_info(&state, headers.clone()).await;
    if let Some(limited) = rate_limited(&state, &ctx).await {
        return limited;
    }
    let Ok(Form(request)) = form else {
        return from_oauth(&OAuthError::InvalidRequest("token is required".into()));
    };
    let (Some(token), Some(client_id)) = (request.token, client_id_of(request.client_id, &headers))
    else {
        return from_oauth(&OAuthError::InvalidRequest(
            "token and client_id are required".into(),
        ));
    };
    match in_tx(&state, async |tx| {
        oauth::revoke_token(tx, &ctx, &client_id, &token).await
    })
    .await
    {
        Ok(()) => StatusCode::OK.into_response(),
        Err(e) => from_oauth(&e),
    }
}

fn scope_string(scopes: &[String]) -> String {
    if scopes.iter().any(|s| s == permissions::WILDCARD) {
        FULL_SCOPE.to_owned()
    } else {
        scopes.join(" ")
    }
}

/// The authorization request's parameters, as the consent page received them.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AuthorizationParams {
    pub response_type: Option<String>,
    pub client_id: Option<String>,
    pub redirect_uri: Option<String>,
    pub code_challenge: Option<String>,
    pub code_challenge_method: Option<String>,
    pub scope: Option<String>,
    pub state: Option<String>,
    pub resource: Option<String>,
}

/// `POST /api/v1/oauth/authorization`.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AuthorizationRequest {
    pub params: AuthorizationParams,
    /// Omit to only check the request (to show the consent page); `true` to
    /// approve it, `false` to deny it.
    pub approve: Option<bool>,
}

/// A permission the client would get.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ScopeView {
    pub key: String,
    pub description: String,
}

/// What the consent page shows, or where it sends the person.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AuthorizationView {
    /// Set when the page should send the browser on (back to the client,
    /// with a code or an error). Otherwise, ask the person.
    pub redirect_to: Option<String>,
    /// The name the client registered with (it chose it itself).
    pub client_name: String,
    /// Where the browser goes afterwards, e.g. `localhost` or `claude.ai`.
    pub redirect_host: String,
    /// `true` when the client gets everything the person's roles allow.
    pub full_access: bool,
    pub scopes: Vec<ScopeView>,
}

/// A request the consent page can't send back to the client.
fn unredirectable(detail: &str) -> ApiError {
    ApiError::new(
        StatusCode::BAD_REQUEST,
        "invalid-authorization-request",
        "This sign-in link isn't valid",
    )
    .with_detail(detail)
}

fn redirect_with(redirect_uri: &str, pairs: &[(&str, &str)], state: Option<&str>) -> String {
    let mut url = url::Url::parse(redirect_uri).expect("registered redirect URIs parse");
    {
        let mut query = url.query_pairs_mut();
        for (k, v) in pairs {
            query.append_pair(k, v);
        }
        if let Some(state) = state {
            query.append_pair("state", state);
        }
    }
    url.into()
}

/// The checked parts of an authorization request.
struct Checked {
    client: oauth::Client,
    redirect_uri: String,
    code_challenge: String,
    scopes: Vec<String>,
    resource: String,
}

/// Checks the request. `Err(Ok(url))`: send the browser back to the client
/// with this error. `Err(Err(problem))`: show the problem (the client or
/// its redirect URI can't be trusted, so nothing is sent back).
async fn check(
    state: &AppState,
    params: &AuthorizationParams,
) -> Result<Checked, Result<String, ApiError>> {
    let (Some(issuer), Some(own_resource)) = (issuer(state), resource(state)) else {
        return Err(Err(unredirectable("OAuth isn't set up on this server.")));
    };
    let client_id = params.client_id.as_deref().unwrap_or_default();
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(|e| Err(ApiError::internal(&e)))?;
    let client = oauth::find_client(&mut tx, client_id)
        .await
        .map_err(|e| Err(ApiError::internal(&e)))?;
    tx.commit().await.map_err(|e| Err(ApiError::internal(&e)))?;
    let Some(client) = client else {
        return Err(Err(unredirectable(
            "The app isn't registered with this Huxley. Start signing in from the app again.",
        )));
    };
    let Some(redirect_uri) = params
        .redirect_uri
        .as_deref()
        .filter(|uri| oauth::redirect_uri_registered(&client, uri))
    else {
        return Err(Err(unredirectable(
            "The app asked to return to an address it didn't register.",
        )));
    };
    let fail = |code: &str, description: &str| {
        Err(Ok(redirect_with(
            redirect_uri,
            &[
                ("error", code),
                ("error_description", description),
                ("iss", &issuer),
            ],
            params.state.as_deref(),
        )))
    };
    if params.response_type.as_deref() != Some("code") {
        return fail("unsupported_response_type", "response_type must be code");
    }
    let Some(code_challenge) = params
        .code_challenge
        .as_deref()
        .filter(|c| oauth::challenge_shaped(c))
    else {
        return fail("invalid_request", "a code_challenge (PKCE) is required");
    };
    if params.code_challenge_method.as_deref() != Some("S256") {
        return fail("invalid_request", "code_challenge_method must be S256");
    }
    let resource = match params.resource.as_deref().map(|r| r.trim_end_matches('/')) {
        None => own_resource,
        Some(r) if r == own_resource => own_resource,
        Some(_) => {
            return fail(
                "invalid_target",
                "this server only issues tokens for its MCP endpoint",
            );
        }
    };
    let Some(scopes) = parse_scopes(params.scope.as_deref()) else {
        return fail("invalid_scope", "an unknown scope was requested");
    };
    Ok(Checked {
        redirect_uri: redirect_uri.to_owned(),
        code_challenge: code_challenge.to_owned(),
        client,
        scopes,
        resource,
    })
}

/// Requested scopes as token scopes: `mcp` (or none) is everything (`*`);
/// otherwise permission keys. `None` if any is unknown.
fn parse_scopes(scope: Option<&str>) -> Option<Vec<String>> {
    let requested: Vec<&str> = scope.unwrap_or_default().split_whitespace().collect();
    if requested.is_empty() || requested.contains(&FULL_SCOPE) {
        return Some(vec![permissions::WILDCARD.to_owned()]);
    }
    let mut scopes = Vec::new();
    for s in requested {
        if s == permissions::WILDCARD || !permissions::is_known(s) {
            return None;
        }
        scopes.push(s.to_owned());
    }
    scopes.sort();
    scopes.dedup();
    Some(scopes)
}

/// Checks an authorization request for the consent page and, once the
/// person decides, approves or denies it. Needs a signed-in session: an API
/// token can't approve anything.
#[utoipa::path(post, path = "/api/v1/oauth/authorization", tag = "auth",
    request_body = AuthorizationRequest,
    responses((status = 200, body = AuthorizationView), (status = 400, body = ApiError, content_type = "application/problem+json")))]
pub async fn authorization(
    State(state): State<AppState>,
    user: CurrentUser,
    Json(body): Json<AuthorizationRequest>,
) -> ApiResult<Json<AuthorizationView>> {
    let ctx = user.ctx.clone();
    let user_id = user.auth.user.id;
    let authed = Authed::load(user, &state).await?;
    if authed.mfa_setup_required() {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "mfa-setup-required",
            "Two-factor authentication is required",
        ));
    }
    let checked = match check(&state, &body.params).await {
        Ok(checked) => checked,
        Err(Ok(redirect_to)) => {
            return Ok(Json(AuthorizationView {
                redirect_to: Some(redirect_to),
                client_name: String::new(),
                redirect_host: String::new(),
                full_access: false,
                scopes: Vec::new(),
            }));
        }
        Err(Err(problem)) => return Err(problem),
    };
    let issuer = issuer(&state).unwrap_or_default();
    let mut view = consent_view(&checked);
    match body.approve {
        None => {}
        Some(false) => {
            view.redirect_to = Some(redirect_with(
                &checked.redirect_uri,
                &[
                    ("error", "access_denied"),
                    ("error_description", "the person declined"),
                    ("iss", &issuer),
                ],
                body.params.state.as_deref(),
            ));
        }
        Some(true) => {
            let mut tx = begin_scoped(&state.db, &OrgScope::System)
                .await
                .map_err(|e| ApiError::internal(&e))?;
            let code = oauth::issue_code(
                &mut tx,
                &Approval {
                    client: &checked.client,
                    user_id,
                    redirect_uri: &checked.redirect_uri,
                    code_challenge: &checked.code_challenge,
                    scopes: &checked.scopes,
                    resource: &checked.resource,
                },
            )
            .await
            .map_err(|e| ApiError::internal(&e))?;
            tx.commit().await.map_err(|e| ApiError::internal(&e))?;
            tracing::info!(actor = ?ctx.actor, client = %checked.client.client_id, "OAuth authorization approved");
            view.redirect_to = Some(redirect_with(
                &checked.redirect_uri,
                &[("code", &code), ("iss", &issuer)],
                body.params.state.as_deref(),
            ));
        }
    }
    Ok(Json(view))
}

fn consent_view(checked: &Checked) -> AuthorizationView {
    let full_access = checked.scopes.iter().any(|s| s == permissions::WILDCARD);
    let redirect_host = url::Url::parse(&checked.redirect_uri)
        .ok()
        .and_then(|u| u.host_str().map(str::to_owned))
        .unwrap_or_else(|| {
            checked
                .redirect_uri
                .split(':')
                .next()
                .unwrap_or_default()
                .to_owned()
        });
    AuthorizationView {
        redirect_to: None,
        client_name: checked.client.name.clone(),
        redirect_host,
        full_access,
        scopes: if full_access {
            Vec::new()
        } else {
            checked
                .scopes
                .iter()
                .map(|key| ScopeView {
                    description: permissions::get(key)
                        .map_or_else(|| key.clone(), |p| p.description.to_owned()),
                    key: key.clone(),
                })
                .collect()
        },
    }
}

/// An app connected to your account through OAuth.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ConnectedAppView {
    pub id: Uuid,
    /// The name the app registered with.
    pub name: String,
    /// Permission keys, or `["*"]` for everything your roles allow.
    pub scopes: Vec<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub connected_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339::option")]
    pub last_used_at: Option<OffsetDateTime>,
    pub last_used_ip: Option<String>,
}

/// Apps (such as AI clients) connected to your account. Needs a session.
#[utoipa::path(get, path = "/api/v1/auth/connected-apps", tag = "auth",
    responses((status = 200, body = Vec<ConnectedAppView>)))]
pub async fn list_connected(
    State(state): State<AppState>,
    user: CurrentUser,
) -> ApiResult<Json<Vec<ConnectedAppView>>> {
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(|e| ApiError::internal(&e))?;
    let apps = oauth::connected_apps(&mut tx, user.auth.user.id)
        .await
        .map_err(|e| ApiError::internal(&e))?;
    tx.commit().await.map_err(|e| ApiError::internal(&e))?;
    Ok(Json(
        apps.into_iter()
            .map(|a| ConnectedAppView {
                id: a.id,
                name: a.client_name,
                scopes: a.scopes,
                connected_at: a.created_at,
                last_used_at: a.last_used_at,
                last_used_ip: a.last_used_ip,
            })
            .collect(),
    ))
}

/// Disconnects an app: its tokens stop working at once.
#[utoipa::path(delete, path = "/api/v1/auth/connected-apps/{id}", tag = "auth",
    params(("id" = Uuid, Path, description = "Connected app id")),
    responses((status = 204), (status = 404, body = ApiError, content_type = "application/problem+json")))]
pub async fn disconnect(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(|e| ApiError::internal(&e))?;
    let found = oauth::disconnect(&mut tx, &user.ctx, user.auth.user.id, id)
        .await
        .map_err(|e| ApiError::internal(&e))?;
    tx.commit().await.map_err(|e| ApiError::internal(&e))?;
    if found {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::not_found())
    }
}

/// The `WWW-Authenticate` value of `/mcp`'s 401.
pub fn www_authenticate(state: &AppState, error: Option<&str>) -> HeaderValue {
    use std::fmt::Write as _;
    let mut value = String::from("Bearer realm=\"huxley\"");
    if let Some(url) = resource_metadata_url(state) {
        let _ = write!(value, ", resource_metadata=\"{url}\"");
    }
    if let Some(error) = error {
        let _ = write!(value, ", error=\"{error}\"");
    }
    HeaderValue::from_str(&value).unwrap_or_else(|_| HeaderValue::from_static("Bearer"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scopes() {
        assert_eq!(parse_scopes(None), Some(vec!["*".to_owned()]));
        assert_eq!(parse_scopes(Some("  ")), Some(vec!["*".to_owned()]));
        assert_eq!(parse_scopes(Some("mcp")), Some(vec!["*".to_owned()]));
        assert_eq!(
            parse_scopes(Some("workflow:read workflow:read")),
            Some(vec!["workflow:read".to_owned()])
        );
        assert_eq!(parse_scopes(Some("workflow:read nope")), None);
        assert_eq!(parse_scopes(Some("*")), None);
        assert_eq!(scope_string(&["*".to_owned()]), "mcp");
    }

    #[test]
    fn redirects_keep_existing_query_and_add_state() {
        let url = redirect_with("http://127.0.0.1:9/cb?x=1", &[("code", "c")], Some("s t"));
        assert_eq!(url, "http://127.0.0.1:9/cb?x=1&code=c&state=s+t");
    }

    #[test]
    fn client_id_from_basic_auth() {
        use base64::Engine as _;
        let mut headers = HeaderMap::new();
        let basic = base64::engine::general_purpose::STANDARD.encode("hxc_abc:");
        headers.insert(
            header::AUTHORIZATION,
            format!("Basic {basic}").parse().unwrap(),
        );
        assert_eq!(client_id_of(None, &headers).as_deref(), Some("hxc_abc"));
        assert_eq!(
            client_id_of(Some("hxc_b".into()), &headers).as_deref(),
            Some("hxc_b")
        );
        assert_eq!(client_id_of(None, &HeaderMap::new()), None);
    }
}
