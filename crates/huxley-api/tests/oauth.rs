//! OAuth 2.1 for MCP clients, end to end against a real PostgreSQL:
//! discovery, registration, consent, PKCE, the token endpoint, refresh-token
//! rotation and reuse detection, revocation, and that access tokens work
//! only through MCP.

use axum::body::Body;
use base64::Engine as _;
use http::{Request, StatusCode};
use http_body_util::BodyExt as _;
use huxley_api::{ApiConfig, AppState, router};
use huxley_auth::{AuthConfig, AuthService};
use huxley_core::{RequestContext, Via};
use huxley_crypto::Crypto;
use huxley_db::testing::{TestPools, ensure_root, pools, unique};
use huxley_db::{OrgScope, begin_scoped};
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};
use tower::ServiceExt as _;

const PASSWORD: &str = "oauth api passphrase";
const ISSUER: &str = "https://huxley.test";
const REDIRECT: &str = "http://127.0.0.1:47123/callback";
const VERIFIER: &str = "a-long-enough-pkce-code-verifier-0123456789-abcdefgh";

fn challenge(verifier: &str) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

struct Setup {
    app: axum::Router,
    cookie: String,
}

async fn setup() -> Option<Setup> {
    let pools = pools().await?;
    let crypto = || Crypto::new(pools.app.clone(), huxley_crypto::testing::key_ring());
    let auth = AuthService::new(
        pools.app.clone(),
        AuthConfig {
            public_url: Some(format!("{ISSUER}/")),
            ..AuthConfig::default()
        },
        crypto(),
    );
    let config = ApiConfig {
        login_attempts_per_minute: 1000,
        ..ApiConfig::default()
    };
    let app = router(AppState::new(pools.app.clone(), auth, crypto(), config));
    ensure_root(&pools).await;
    let cookie = login(&app, &owner(&pools).await).await;
    Some(Setup { app, cookie })
}

async fn owner(pools: &TestPools) -> String {
    let email = format!("{}@example.com", unique("oa").replace(' ', "."));
    let ctx = RequestContext::system(Via::System);
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    let u = huxley_auth::create_user(&mut tx, &ctx, &email, "OAuth User", PASSWORD)
        .await
        .unwrap();
    huxley_authz::grant_root_owner(&mut tx, &ctx, u.id)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    email
}

async fn login(app: &axum::Router, email: &str) -> String {
    let req = Request::post("/api/v1/auth/login")
        .header("x-requested-with", "huxley")
        .header("content-type", "application/json")
        .body(Body::from(
            json!({ "email": email, "password": PASSWORD }).to_string(),
        ))
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let cookie = res.headers()["set-cookie"].to_str().unwrap();
    cookie.split(';').next().unwrap().to_owned()
}

async fn send(app: &axum::Router, req: Request<Body>) -> (StatusCode, http::HeaderMap, Value) {
    let res = app.clone().oneshot(req).await.unwrap();
    let status = res.status();
    let headers = res.headers().clone();
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        headers,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

fn query_of(url: &str) -> std::collections::HashMap<String, String> {
    url::Url::parse(url)
        .unwrap()
        .query_pairs()
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect()
}

impl Setup {
    async fn get(&self, uri: &str) -> (StatusCode, Value) {
        let (status, _, body) =
            send(&self.app, Request::get(uri).body(Body::empty()).unwrap()).await;
        (status, body)
    }

    async fn register(&self, body: Value) -> (StatusCode, Value) {
        let req = Request::post("/oauth/register")
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap();
        let (status, _, body) = send(&self.app, req).await;
        (status, body)
    }

    async fn form(&self, uri: &str, pairs: &[(&str, &str)]) -> (StatusCode, Value) {
        let body = url::form_urlencoded::Serializer::new(String::new())
            .extend_pairs(pairs)
            .finish();
        let req = Request::post(uri)
            .header("content-type", "application/x-www-form-urlencoded")
            .body(Body::from(body))
            .unwrap();
        let (status, _, body) = send(&self.app, req).await;
        (status, body)
    }

    async fn authorization(&self, params: Value, approve: Option<bool>) -> (StatusCode, Value) {
        let mut body = json!({ "params": params });
        if let Some(approve) = approve {
            body["approve"] = json!(approve);
        }
        let req = Request::post("/api/v1/oauth/authorization")
            .header("x-requested-with", "huxley")
            .header("cookie", &self.cookie)
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap();
        let (status, _, body) = send(&self.app, req).await;
        (status, body)
    }

    async fn client(&self) -> String {
        let (status, client) = self
            .register(json!({ "client_name": "Test Agent", "redirect_uris": ["http://127.0.0.1/callback"] }))
            .await;
        assert_eq!(status, StatusCode::CREATED, "{client}");
        client["client_id"].as_str().unwrap().to_owned()
    }

    /// Runs consent and the code exchange; returns the token response.
    async fn sign_in(&self, client_id: &str, scope: Option<&str>) -> Value {
        let mut params = json!({
            "responseType": "code", "clientId": client_id, "redirectUri": REDIRECT,
            "codeChallenge": challenge(VERIFIER), "codeChallengeMethod": "S256",
            "state": "xyz", "resource": format!("{ISSUER}/mcp"),
        });
        if let Some(scope) = scope {
            params["scope"] = json!(scope);
        }
        let (status, approved) = self.authorization(params, Some(true)).await;
        assert_eq!(status, StatusCode::OK, "{approved}");
        let back = query_of(approved["redirectTo"].as_str().unwrap());
        assert_eq!(back["state"], "xyz");
        assert_eq!(back["iss"], ISSUER);
        let (status, tokens) = self
            .form(
                "/oauth/token",
                &[
                    ("grant_type", "authorization_code"),
                    ("code", &back["code"]),
                    ("redirect_uri", REDIRECT),
                    ("code_verifier", VERIFIER),
                    ("client_id", client_id),
                    ("resource", &format!("{ISSUER}/mcp")),
                ],
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{tokens}");
        tokens
    }

    async fn mcp(&self, token: &str, tool: &str) -> (StatusCode, Value) {
        let req = Request::post("/mcp")
            .header("content-type", "application/json")
            .header("authorization", format!("Bearer {token}"))
            .body(Body::from(
                json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call",
                        "params": { "name": tool, "arguments": {} } })
                .to_string(),
            ))
            .unwrap();
        let (status, _, body) = send(&self.app, req).await;
        (status, body)
    }
}

#[tokio::test]
async fn discovery_points_clients_at_the_authorization_server() {
    let Some(s) = setup().await else { return };
    let req = Request::post("/mcp")
        .header("content-type", "application/json")
        .body(Body::from(
            json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize" }).to_string(),
        ))
        .unwrap();
    let (status, headers, _) = send(&s.app, req).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(
        headers["www-authenticate"],
        "Bearer realm=\"huxley\", resource_metadata=\"https://huxley.test/.well-known/oauth-protected-resource/mcp\""
    );

    for path in [
        "/.well-known/oauth-protected-resource/mcp",
        "/.well-known/oauth-protected-resource",
    ] {
        let (status, resource) = s.get(path).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(resource["resource"], "https://huxley.test/mcp");
        assert_eq!(resource["authorization_servers"], json!([ISSUER]));
    }
    let (status, server) = s.get("/.well-known/oauth-authorization-server").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(server["issuer"], ISSUER);
    assert_eq!(
        server["authorization_endpoint"],
        "https://huxley.test/oauth/authorize"
    );
    assert_eq!(server["token_endpoint"], "https://huxley.test/oauth/token");
    assert_eq!(
        server["registration_endpoint"],
        "https://huxley.test/oauth/register"
    );
    assert_eq!(server["code_challenge_methods_supported"], json!(["S256"]));

    // Browser-based clients may call these from their own origin.
    let req = Request::options("/oauth/token")
        .header("origin", "https://inspector.example")
        .header("access-control-request-method", "POST")
        .body(Body::empty())
        .unwrap();
    let (status, headers, _) = send(&s.app, req).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(headers["access-control-allow-origin"], "*");
}

#[tokio::test]
async fn registration_accepts_safe_redirects_only() {
    let Some(s) = setup().await else { return };
    let (status, client) = s
        .register(
            json!({ "client_name": "Claude Code", "redirect_uris": [REDIRECT],
                          "grant_types": ["authorization_code", "refresh_token"],
                          "token_endpoint_auth_method": "client_secret_post" }),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{client}");
    assert!(client["client_id"].as_str().unwrap().starts_with("hxc_"));
    assert_eq!(client["token_endpoint_auth_method"], "none");
    assert!(client.get("client_secret").is_none());

    for (body, error) in [
        (
            json!({ "redirect_uris": ["http://evil.example/cb"] }),
            "invalid_redirect_uri",
        ),
        (
            json!({ "redirect_uris": ["javascript:alert(1)"] }),
            "invalid_redirect_uri",
        ),
        (json!({ "redirect_uris": [] }), "invalid_redirect_uri"),
        (
            json!({ "redirect_uris": [REDIRECT], "grant_types": ["client_credentials"] }),
            "invalid_client_metadata",
        ),
        (
            json!({ "redirect_uris": [REDIRECT], "response_types": ["token"] }),
            "invalid_client_metadata",
        ),
    ] {
        let (status, body) = s.register(body).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
        assert_eq!(body["error"], error);
    }
}

#[tokio::test]
async fn consent_checks_the_request_before_anything_is_sent_back() {
    let Some(s) = setup().await else { return };
    let client_id = s.client().await;
    let good = json!({
        "responseType": "code", "clientId": client_id, "redirectUri": REDIRECT,
        "codeChallenge": challenge(VERIFIER), "codeChallengeMethod": "S256", "state": "s1",
    });

    // What the consent page shows.
    let (status, view) = s.authorization(good.clone(), None).await;
    assert_eq!(status, StatusCode::OK, "{view}");
    assert_eq!(view["clientName"], "Test Agent");
    assert_eq!(view["redirectHost"], "127.0.0.1");
    assert_eq!(view["fullAccess"], true);
    assert!(view["redirectTo"].is_null());

    // An unknown client or unregistered redirect: shown, never redirected.
    for (field, value) in [
        ("clientId", "hxc_nobody"),
        ("redirectUri", "https://evil.example/callback"),
    ] {
        let mut params = good.clone();
        params[field] = json!(value);
        let (status, problem) = s.authorization(params, Some(true)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{problem}");
    }

    // Other problems go back to the client, with its state.
    for (field, value, error) in [
        ("codeChallenge", Value::Null, "invalid_request"),
        ("codeChallengeMethod", json!("plain"), "invalid_request"),
        ("responseType", json!("token"), "unsupported_response_type"),
        ("scope", json!("no:such:scope"), "invalid_scope"),
        (
            "resource",
            json!("https://elsewhere.example/mcp"),
            "invalid_target",
        ),
    ] {
        let mut params = good.clone();
        params[field] = value;
        let (status, view) = s.authorization(params, Some(true)).await;
        assert_eq!(status, StatusCode::OK, "{view}");
        let back = query_of(view["redirectTo"].as_str().unwrap());
        assert_eq!(back["error"], error, "{field}");
        assert_eq!(back["state"], "s1");
        assert!(!back.contains_key("code"));
    }

    // Declining.
    let (_, view) = s.authorization(good.clone(), Some(false)).await;
    let back = query_of(view["redirectTo"].as_str().unwrap());
    assert_eq!(back["error"], "access_denied");

    // An API token can't approve anything: consent needs a session.
    let req = Request::post("/api/v1/oauth/authorization")
        .header("x-requested-with", "huxley")
        .header("authorization", "Bearer hxp_whatever")
        .header("content-type", "application/json")
        .body(Body::from(
            json!({ "params": good, "approve": true }).to_string(),
        ))
        .unwrap();
    let (status, _, _) = send(&s.app, req).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn tokens_work_through_mcp_only_and_follow_the_scopes() {
    let Some(s) = setup().await else { return };
    let client_id = s.client().await;
    let tokens = s.sign_in(&client_id, None).await;
    assert_eq!(tokens["token_type"], "Bearer");
    assert_eq!(tokens["expires_in"], 3600);
    assert_eq!(tokens["scope"], "mcp");
    let access = tokens["access_token"].as_str().unwrap();
    assert!(access.starts_with("hxo_"));

    // Through MCP, a tool calls the API as the person.
    let (status, reply) = s.mcp(access, "list_organizations").await;
    assert_eq!(status, StatusCode::OK, "{reply}");
    assert_eq!(reply["result"]["isError"], false, "{reply}");
    let visible = reply["result"]["structuredContent"]["items"]
        .as_array()
        .unwrap()
        .len();
    assert!(visible > 0, "{reply}");

    // The API itself refuses it: it's a token for the MCP server.
    let req = Request::get("/api/v1/orgs")
        .header("authorization", format!("Bearer {access}"))
        .body(Body::empty())
        .unwrap();
    let (status, _, problem) = send(&s.app, req).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert!(
        problem["detail"].as_str().unwrap().contains("MCP"),
        "{problem}"
    );

    // Scopes narrow, as an API token's do.
    let narrow = s.sign_in(&client_id, Some("workflow:read")).await;
    assert_eq!(narrow["scope"], "workflow:read");
    let (_, reply) = s
        .mcp(
            narrow["access_token"].as_str().unwrap(),
            "list_organizations",
        )
        .await;
    // Organizations are listed by permission: without org:read, none.
    assert_eq!(
        reply["result"]["structuredContent"]["items"],
        json!([]),
        "{reply}"
    );

    // It's listed among the person's connected apps, and disconnecting it
    // stops its tokens at once.
    let req = Request::get("/api/v1/auth/connected-apps")
        .header("cookie", &s.cookie)
        .body(Body::empty())
        .unwrap();
    let (_, _, apps) = send(&s.app, req).await;
    let apps = apps.as_array().unwrap();
    assert_eq!(apps.len(), 2);
    assert_eq!(apps[1]["name"], "Test Agent");
    assert_eq!(apps[1]["scopes"], json!(["*"]));
    let req = Request::delete(format!(
        "/api/v1/auth/connected-apps/{}",
        apps[1]["id"].as_str().unwrap()
    ))
    .header("x-requested-with", "huxley")
    .header("cookie", &s.cookie)
    .body(Body::empty())
    .unwrap();
    let (status, _, _) = send(&s.app, req).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = s.mcp(access, "list_organizations").await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn codes_need_the_verifier_and_are_single_use() {
    let Some(s) = setup().await else { return };
    let client_id = s.client().await;
    let params = json!({
        "responseType": "code", "clientId": client_id, "redirectUri": REDIRECT,
        "codeChallenge": challenge(VERIFIER), "codeChallengeMethod": "S256",
    });
    let (_, approved) = s.authorization(params, Some(true)).await;
    let code = query_of(approved["redirectTo"].as_str().unwrap())["code"].clone();
    let exchange = |verifier: &'static str, redirect: &'static str| {
        let code = code.clone();
        let client_id = client_id.clone();
        let s = &s;
        async move {
            s.form(
                "/oauth/token",
                &[
                    ("grant_type", "authorization_code"),
                    ("code", &code),
                    ("redirect_uri", redirect),
                    ("code_verifier", verifier),
                    ("client_id", &client_id),
                ],
            )
            .await
        }
    };

    let wrong_verifier = "another-long-enough-verifier-that-does-not-match-0000";
    let (status, error) = exchange(wrong_verifier, REDIRECT).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(error["error"], "invalid_grant");
    let (status, error) = exchange(VERIFIER, "http://127.0.0.1:1/callback").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(error["error"], "invalid_grant");

    let (status, tokens) = exchange(VERIFIER, REDIRECT).await;
    assert_eq!(status, StatusCode::OK, "{tokens}");
    let access = tokens["access_token"].as_str().unwrap();
    assert_eq!(s.mcp(access, "list_organizations").await.0, StatusCode::OK);

    // A second exchange fails, and revokes what the first one issued.
    let (status, error) = exchange(VERIFIER, REDIRECT).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(error["error"], "invalid_grant");
    assert_eq!(
        s.mcp(access, "list_organizations").await.0,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn refresh_tokens_rotate_and_a_replayed_one_revokes_the_grant() {
    let Some(s) = setup().await else { return };
    let client_id = s.client().await;
    let first = s.sign_in(&client_id, None).await;
    let refresh = |token: String| {
        let client_id = client_id.clone();
        let s = &s;
        async move {
            s.form(
                "/oauth/token",
                &[
                    ("grant_type", "refresh_token"),
                    ("refresh_token", &token),
                    ("client_id", &client_id),
                ],
            )
            .await
        }
    };
    let old_refresh = first["refresh_token"].as_str().unwrap().to_owned();

    // Another client can't use it.
    let other = s.client().await;
    let (status, error) = s
        .form(
            "/oauth/token",
            &[
                ("grant_type", "refresh_token"),
                ("refresh_token", &old_refresh),
                ("client_id", &other),
            ],
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(error["error"], "invalid_grant");

    let (status, second) = refresh(old_refresh.clone()).await;
    assert_eq!(status, StatusCode::OK, "{second}");
    assert_ne!(second["refresh_token"], first["refresh_token"]);
    let access = second["access_token"].as_str().unwrap().to_owned();
    assert_eq!(s.mcp(&access, "list_organizations").await.0, StatusCode::OK);

    // The replaced refresh token comes back: someone copied it. The grant
    // is revoked, so every token from it stops working.
    let (status, error) = refresh(old_refresh).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(error["error"], "invalid_grant");
    assert_eq!(
        s.mcp(&access, "list_organizations").await.0,
        StatusCode::UNAUTHORIZED
    );
    let (status, _) = refresh(second["refresh_token"].as_str().unwrap().to_owned()).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn revocation_ends_the_grant() {
    let Some(s) = setup().await else { return };
    let client_id = s.client().await;
    let tokens = s.sign_in(&client_id, None).await;
    let access = tokens["access_token"].as_str().unwrap();

    // Unknown tokens: still 200, revealing nothing.
    let (status, _) = s
        .form(
            "/oauth/revoke",
            &[("token", "hxr_nonsense"), ("client_id", &client_id)],
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(s.mcp(access, "list_organizations").await.0, StatusCode::OK);

    let (status, _) = s
        .form(
            "/oauth/revoke",
            &[
                ("token", tokens["refresh_token"].as_str().unwrap()),
                ("client_id", &client_id),
            ],
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        s.mcp(access, "list_organizations").await.0,
        StatusCode::UNAUTHORIZED
    );
}
