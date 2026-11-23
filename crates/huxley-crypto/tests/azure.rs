//! Key-encryption keys in Azure Key Vault, against a mock of Key Vault and
//! Microsoft Entra ID (the wrap is not real RSA: these tests check requests,
//! tokens, key versions and the data-key frame, not Azure's cryptography).

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use axum::Form;
use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{get, post};
use huxley_core::{RequestContext, Via};
use huxley_crypto::{Binding, Crypto, CryptoConfig, CryptoError, generate_key_base64};
use huxley_db::testing::{TestPools, ensure_root, pools, unique};
use huxley_db::{OrgScope, begin_scoped};
use serde_json::{Value, json};
use std::collections::HashMap;
use uuid::Uuid;

#[derive(Default)]
struct Mock {
    sign_ins: AtomicUsize,
    /// Tokens issued before this count are treated as expired.
    valid_from: AtomicUsize,
    /// The key's current version.
    version: std::sync::Mutex<String>,
}

fn token_response(mock: &Mock) -> Json<Value> {
    let n = mock.sign_ins.fetch_add(1, Ordering::SeqCst) + 1;
    Json(
        json!({ "access_token": format!("token-{n}"), "expires_in": 3599, "token_type": "Bearer" }),
    )
}

async fn entra(
    State(mock): State<Arc<Mock>>,
    Path(tenant): Path<String>,
    Form(form): Form<HashMap<String, String>>,
) -> (StatusCode, Json<Value>) {
    let ok = tenant == "tenant-1"
        && form.get("grant_type").map(String::as_str) == Some("client_credentials")
        && form.get("client_id").map(String::as_str) == Some("app-1")
        && form.get("client_secret").map(String::as_str) == Some("s3cret")
        && form.get("scope").is_some_and(|s| s.ends_with("/.default"));
    if !ok {
        return (
            StatusCode::UNAUTHORIZED,
            Json(
                json!({ "error": "invalid_client", "error_description": "AADSTS7000215: Invalid client secret provided." }),
            ),
        );
    }
    (StatusCode::OK, token_response(&mock))
}

async fn imds(
    State(mock): State<Arc<Mock>>,
    headers: HeaderMap,
    Query(query): Query<HashMap<String, String>>,
) -> (StatusCode, Json<Value>) {
    if headers.get("metadata").is_none() || !query.contains_key("resource") {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "bad request" })),
        );
    }
    (StatusCode::OK, token_response(&mock))
}

fn authorized(mock: &Mock, headers: &HeaderMap) -> bool {
    let issued = mock.sign_ins.load(Ordering::SeqCst);
    let from = mock.valid_from.load(Ordering::SeqCst);
    headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer token-"))
        .and_then(|n| n.parse::<usize>().ok())
        .is_some_and(|n| n <= issued && n > from)
}

async fn key_op(
    State(mock): State<Arc<Mock>>,
    Path(rest): Path<String>,
    headers: HeaderMap,
    Query(query): Query<HashMap<String, String>>,
    Json(body): Json<Value>,
) -> (StatusCode, Json<Value>) {
    if !authorized(&mock, &headers) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": { "code": "Unauthorized", "message": "token expired" } })),
        );
    }
    assert_eq!(query.get("api-version").map(String::as_str), Some("7.5"));
    assert_eq!(body["alg"], "RSA-OAEP-256");
    let value = body["value"].as_str().unwrap();
    let current = mock.version.lock().unwrap().clone();
    let parts: Vec<&str> = rest.split('/').collect();
    match parts.as_slice() {
        ["wrapkey"] => (
            StatusCode::OK,
            Json(
                json!({ "kid": format!("https://mock/keys/huxley/{current}"), "value": format!("W{current}{value}") }),
            ),
        ),
        [version, "unwrapkey"] => {
            let Some(inner) = value.strip_prefix(&format!("W{version}")) else {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(
                        json!({ "error": { "code": "BadParameter", "message": "wrong version" } }),
                    ),
                );
            };
            (StatusCode::OK, Json(json!({ "kid": "x", "value": inner })))
        }
        _ => (StatusCode::NOT_FOUND, Json(json!({}))),
    }
}

async fn mock_server() -> (String, Arc<Mock>) {
    let mock = Arc::new(Mock::default());
    *mock.version.lock().unwrap() = "v1aaaa".into();
    let app = axum::Router::new()
        .route("/{tenant}/oauth2/v2.0/token", post(entra))
        .route("/metadata/identity/oauth2/token", get(imds))
        .route("/keys/huxley/{*rest}", post(key_op))
        .with_state(mock.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (address, mock)
}

fn secret_auth(address: &str, secret: &str) -> Value {
    json!({
        "vault_url": address,
        "auth": {
            "tenant_id": "tenant-1",
            "client_id": "app-1",
            "client_secret": secret,
            "authority_host": address,
        },
    })
}

fn config(value: Value) -> CryptoConfig {
    serde_json::from_value(value).unwrap()
}

#[tokio::test]
async fn client_secret_sign_in_is_cached_and_renewed_when_refused() {
    let (address, mock) = mock_server().await;
    let ring = config(json!({ "azure_key_vault": secret_auth(&address, "s3cret") }))
        .key_ring()
        .unwrap();
    assert_eq!(ring.active_id(), "azure:127/huxley");
    ring.check().await.unwrap();
    ring.check().await.unwrap();
    assert_eq!(mock.sign_ins.load(Ordering::SeqCst), 1);

    mock.valid_from.store(1, Ordering::SeqCst);
    ring.check().await.unwrap();
    assert_eq!(mock.sign_ins.load(Ordering::SeqCst), 2);

    let wrong = config(json!({ "azure_key_vault": secret_auth(&address, "nope") }))
        .key_ring()
        .unwrap();
    let err = wrong.check().await.unwrap_err();
    assert!(
        matches!(&err, CryptoError::Kms(m) if m.contains("AADSTS7000215")),
        "{err}"
    );
}

#[tokio::test]
async fn managed_identity_from_an_explicit_endpoint() {
    let (address, mock) = mock_server().await;
    let ring = config(json!({
        "azure_key_vault": {
            "vault_url": address,
            "id": "azure:contoso/huxley",
            "auth": { "managed_identity_endpoint": format!("{address}/metadata/identity/oauth2/token") },
        },
    }))
    .key_ring()
    .unwrap();
    assert_eq!(ring.active_id(), "azure:contoso/huxley");
    ring.check().await.unwrap();
    assert_eq!(mock.sign_ins.load(Ordering::SeqCst), 1);
}

async fn provider(pools: &TestPools) -> Uuid {
    let id = Uuid::now_v7();
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    sqlx::query(
        "INSERT INTO organizations (id, parent_id, kind, name, path) \
         VALUES ($1, $2, 'provider', $3, '')",
    )
    .bind(id)
    .bind(ensure_root(pools).await)
    .bind(unique("azure"))
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    id
}

async fn stored(pools: &TestPools, org: Uuid) -> (String, Vec<u8>) {
    sqlx::query_as("SELECT kek_id, wrapped_key FROM data_keys WHERE org_id = $1")
        .bind(org)
        .fetch_one(&pools.owner)
        .await
        .unwrap()
}

#[tokio::test]
async fn data_keys_move_to_key_vault_and_keep_their_key_version() {
    let Some(pools) = pools().await else { return };
    let (address, mock) = mock_server().await;
    let local = generate_key_base64();
    let ctx = RequestContext::system(Via::Cli);
    let org = provider(&pools).await;
    let record = Uuid::now_v7();
    let binding = Binding {
        purpose: "azure_test",
        record,
    };
    let sealed = Crypto::new(
        pools.app.clone(),
        config(json!({ "kek": local })).key_ring().unwrap(),
    )
    .encrypt(org, binding, b"secret")
    .await
    .unwrap();

    let azure = || json!({ "kek": local, "azure_key_vault": secret_auth(&address, "s3cret"), "active": "azure_key_vault" });
    let both = Crypto::new(pools.app.clone(), config(azure()).key_ring().unwrap());
    assert_eq!(both.rewrap(&ctx, Some(org), false).await.unwrap(), 1);
    let (kek_id, wrapped) = stored(&pools, org).await;
    assert_eq!(kek_id, "azure:127/huxley");
    assert!(wrapped.starts_with(b"azkv1:v1aaaa:"));

    // After the key rotates in Key Vault, old wraps still unwrap with the
    // version they name, and --all moves them to the new one.
    *mock.version.lock().unwrap() = "v2bbbb".into();
    let azure_only = || {
        Crypto::new(
            pools.app.clone(),
            config(json!({ "azure_key_vault": secret_auth(&address, "s3cret") }))
                .key_ring()
                .unwrap(),
        )
    };
    assert_eq!(
        &**azure_only().decrypt(binding, &sealed).await.unwrap(),
        b"secret"
    );
    assert_eq!(azure_only().rewrap(&ctx, Some(org), true).await.unwrap(), 1);
    assert!(stored(&pools, org).await.1.starts_with(b"azkv1:v2bbbb:"));
    assert_eq!(
        &**azure_only().decrypt(binding, &sealed).await.unwrap(),
        b"secret"
    );
}
