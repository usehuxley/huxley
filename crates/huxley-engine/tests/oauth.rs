//! OAuth 2.0 connections against a mock provider on localhost.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use axum::Json as AxumJson;
use axum::extract::{Form, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::post;
use huxley_core::{RequestContext, Via};
use huxley_crypto::Crypto;
use huxley_db::testing::{IsolatedDb, TestPools, ensure_root, isolated};
use huxley_db::{OrgScope, begin_scoped};
use huxley_engine::oauth::TokenError;
use huxley_engine::{EgressConfig, Engine, EngineConfig};
use huxley_integrations::connections::{self, ConnectionInput, ConnectionStatus};
use huxley_integrations::{oauth, store};
use serde_json::json;
use uuid::Uuid;

/// What the mock provider saw and how it should answer.
#[derive(Default)]
struct Provider {
    requests: Vec<HashMap<String, String>>,
    /// The PKCE challenge from the authorize URL, checked at exchange.
    challenge: Option<String>,
    /// Answer refreshes with `invalid_grant`.
    revoked: bool,
    issued: u32,
}

async fn token(
    State(p): State<Arc<Mutex<Provider>>>,
    Form(form): Form<HashMap<String, String>>,
) -> impl IntoResponse {
    let mut p = p.lock().unwrap();
    p.requests.push(form.clone());
    if form.get("client_secret").map(String::as_str) != Some("s3cret") {
        return (
            StatusCode::UNAUTHORIZED,
            AxumJson(json!({ "error": "invalid_client" })),
        );
    }
    match form["grant_type"].as_str() {
        "authorization_code" => {
            let verifier = form.get("code_verifier").cloned().unwrap_or_default();
            if p.challenge.as_deref() != Some(oauth::challenge(&verifier).as_str())
                || form.get("code").map(String::as_str) != Some("the-code")
            {
                return (
                    StatusCode::BAD_REQUEST,
                    AxumJson(
                        json!({ "error": "invalid_grant", "error_description": "bad code or verifier" }),
                    ),
                );
            }
        }
        "refresh_token" if p.revoked => {
            return (
                StatusCode::BAD_REQUEST,
                AxumJson(json!({ "error": "invalid_grant", "error_description": "revoked" })),
            );
        }
        _ => {}
    }
    p.issued += 1;
    let n = p.issued;
    (
        StatusCode::OK,
        AxumJson(json!({
            "access_token": format!("access-{n}"),
            "refresh_token": format!("refresh-{n}"),
            "token_type": "Bearer",
            "expires_in": 3600,
        })),
    )
}

struct Harness {
    _db: IsolatedDb,
    pools: TestPools,
    engine: Engine,
    crypto: Crypto,
    org: Uuid,
    integration: Uuid,
    provider: Arc<Mutex<Provider>>,
}

async fn harness() -> Option<Harness> {
    let db = isolated("oauth").await?;
    let pools = db.pools.clone();
    let org = ensure_root(&pools).await;
    let provider = Arc::new(Mutex::new(Provider::default()));
    let app = axum::Router::new()
        .route("/token", post(token))
        .with_state(provider.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    let spec = format!(
        r#"
openapi: 3.0.3
info: {{ title: Provider API, version: "1" }}
servers: [{{ url: "http://localhost:{port}" }}]
components:
  securitySchemes:
    app:
      type: oauth2
      flows:
        clientCredentials: {{ tokenUrl: "http://localhost:{port}/token", scopes: {{ api: API }} }}
    user:
      type: oauth2
      flows:
        authorizationCode:
          authorizationUrl: https://login.example.com/authorize
          tokenUrl: "http://localhost:{port}/token"
          scopes: {{ read: Read }}
paths:
  /me:
    get: {{ responses: {{ "200": {{ description: ok }} }} }}
"#
    );
    let ctx = RequestContext::system(Via::System);
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    let integration = store::create(&mut tx, &ctx, org, Some("provider"), &spec)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let crypto = Crypto::new(pools.app.clone(), huxley_crypto::testing::key_ring());
    let config = EngineConfig {
        egress: EgressConfig {
            private_hosts: vec!["localhost".into()],
            ..EgressConfig::default()
        },
        ..EngineConfig::default()
    };
    let engine = Engine::with_config(pools.app.clone(), crypto.clone(), &config);
    Some(Harness {
        _db: db,
        pools,
        engine,
        crypto,
        org,
        integration: integration.summary.id,
        provider,
    })
}

impl Harness {
    async fn connection(&self, scheme: &str) -> Uuid {
        let ctx = RequestContext::system(Via::System);
        let input = ConnectionInput {
            name: format!("{scheme} connection"),
            auth_scheme: Some(scheme.into()),
            settings: json!({ "client_id": "huxley" })
                .as_object()
                .unwrap()
                .clone(),
            secrets: [("client_secret".to_owned(), "s3cret".to_owned())].into(),
            ..ConnectionInput::default()
        };
        let mut tx = begin_scoped(&self.pools.app, &OrgScope::System)
            .await
            .unwrap();
        let view = connections::create(
            &mut tx,
            &self.crypto,
            &ctx,
            self.org,
            self.integration,
            &input,
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
        view.id
    }

    async fn expire(&self, id: Uuid) {
        sqlx::query("UPDATE connections SET token_expires_at = now() WHERE id = $1")
            .bind(id)
            .execute(&self.pools.owner)
            .await
            .unwrap();
    }

    async fn status(&self, id: Uuid) -> ConnectionStatus {
        let mut tx = begin_scoped(&self.pools.app, &OrgScope::System)
            .await
            .unwrap();
        connections::get(&mut tx, id).await.unwrap().status
    }

    fn requests(&self) -> usize {
        self.provider.lock().unwrap().requests.len()
    }

    async fn user(&self) -> Uuid {
        let id = Uuid::now_v7();
        sqlx::query("INSERT INTO users (id, email, display_name) VALUES ($1, $2, 'Signer')")
            .bind(id)
            .bind(format!("{}@example.com", id.simple()))
            .execute(&self.pools.owner)
            .await
            .unwrap();
        id
    }
}

#[tokio::test]
async fn client_credentials_tokens_are_reused_and_refreshed_once() {
    let Some(h) = harness().await else { return };
    let id = h.connection("app").await;
    assert_eq!(h.status(id).await, ConnectionStatus::Ready);

    let token = h.engine.connection_token(id).await.unwrap();
    assert_eq!(token.as_deref(), Some("access-1"));
    assert_eq!(
        h.engine.connection_token(id).await.unwrap().as_deref(),
        Some("access-1")
    );
    assert_eq!(h.requests(), 1, "the stored token is reused");
    let first = h.provider.lock().unwrap().requests[0].clone();
    assert_eq!(first["grant_type"], "client_credentials");
    assert_eq!(first["scope"], "api");
    assert_eq!(first["client_id"], "huxley");

    // Expired: five workers at once make one request between them.
    h.expire(id).await;
    let calls: Vec<_> = (0..5)
        .map(|_| {
            let engine = h.engine.clone();
            tokio::spawn(async move { engine.connection_token(id).await.unwrap() })
        })
        .collect();
    for call in calls {
        assert_eq!(call.await.unwrap().as_deref(), Some("access-2"));
    }
    assert_eq!(h.requests(), 2);
}

#[tokio::test]
async fn signing_in_exchanges_the_code_with_pkce_then_refreshes() {
    let Some(h) = harness().await else { return };
    let id = h.connection("user").await;
    assert_eq!(h.status(id).await, ConnectionStatus::NeedsAuth);
    let user = h.user().await;
    let redirect = "https://huxley.example.com/integrations/oauth/callback";

    let mut tx = begin_scoped(&h.pools.app, &OrgScope::System).await.unwrap();
    let url = oauth::begin_sign_in(&mut tx, id, user, redirect)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let url = url::Url::parse(&url).unwrap();
    let query: HashMap<String, String> = url.query_pairs().into_owned().collect();
    assert_eq!(url.host_str(), Some("login.example.com"));
    assert_eq!(query["response_type"], "code");
    assert_eq!(query["client_id"], "huxley");
    assert_eq!(query["redirect_uri"], redirect);
    assert_eq!(query["scope"], "read");
    assert_eq!(query["code_challenge_method"], "S256");
    h.provider.lock().unwrap().challenge = Some(query["code_challenge"].clone());
    let state = &query["state"];

    // Someone else can't finish it.
    let ctx = RequestContext::system(Via::System);
    let other = h.user().await;
    let err = h
        .engine
        .complete_sign_in(&ctx, &OrgScope::System, other, state, "the-code")
        .await
        .unwrap_err();
    assert!(err.to_string().contains("isn't yours"), "{err}");

    // That attempt used the state up; start again and finish properly.
    let mut tx = begin_scoped(&h.pools.app, &OrgScope::System).await.unwrap();
    let url = oauth::begin_sign_in(&mut tx, id, user, redirect)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let query: HashMap<String, String> = url::Url::parse(&url)
        .unwrap()
        .query_pairs()
        .into_owned()
        .collect();
    h.provider.lock().unwrap().challenge = Some(query["code_challenge"].clone());
    let view = h
        .engine
        .complete_sign_in(&ctx, &OrgScope::System, user, &query["state"], "the-code")
        .await
        .unwrap();
    assert_eq!(view.status, ConnectionStatus::Ready);
    assert_eq!(
        h.engine.connection_token(id).await.unwrap().as_deref(),
        Some("access-1")
    );
    // A state works once.
    assert!(
        h.engine
            .complete_sign_in(&ctx, &OrgScope::System, user, &query["state"], "the-code")
            .await
            .is_err()
    );

    // Expired: the refresh token gets a new pair (the provider rotates it).
    h.expire(id).await;
    assert_eq!(
        h.engine.connection_token(id).await.unwrap().as_deref(),
        Some("access-2")
    );
    let refresh = h.provider.lock().unwrap().requests.last().unwrap().clone();
    assert_eq!(refresh["grant_type"], "refresh_token");
    assert_eq!(refresh["refresh_token"], "refresh-1");

    // Revoked at the provider: the connection needs signing in again.
    h.provider.lock().unwrap().revoked = true;
    h.expire(id).await;
    let err = h.engine.connection_token(id).await.unwrap_err();
    assert!(matches!(err, TokenError::NeedsSignIn(_)), "{err}");
    assert_eq!(h.status(id).await, ConnectionStatus::NeedsAuth);
}

#[tokio::test]
async fn other_connections_have_no_token() {
    let Some(h) = harness().await else { return };
    // A connection with no auth isn't OAuth.
    let ctx = RequestContext::system(Via::System);
    let input = ConnectionInput {
        name: "Open".into(),
        ..ConnectionInput::default()
    };
    let mut tx = begin_scoped(&h.pools.app, &OrgScope::System).await.unwrap();
    let view = connections::create(&mut tx, &h.crypto, &ctx, h.org, h.integration, &input)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(h.engine.connection_token(view.id).await.unwrap(), None);
    assert_eq!(h.requests(), 0);
}
