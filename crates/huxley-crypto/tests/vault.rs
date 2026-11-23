//! Key-encryption keys in HashiCorp Vault / OpenBao.
//!
//! The mock tests always run. The real-server tests need an OpenBao or Vault
//! dev server and skip without one:
//!
//! ```sh
//! docker run -d --name huxley-openbao --network host \
//!   -e BAO_DEV_ROOT_TOKEN_ID=dev-root openbao/openbao:2.4.3 server -dev
//! export HUXLEY_TEST_VAULT_ADDR=http://127.0.0.1:8200 HUXLEY_TEST_VAULT_TOKEN=dev-root
//! ```
//!
//! Set `HUXLEY_REQUIRE_VAULT_TESTS=1` to fail instead of skipping.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::post;
use huxley_core::{RequestContext, Via};
use huxley_crypto::{Binding, Crypto, CryptoConfig, CryptoError, generate_key_base64};
use huxley_db::testing::{TestPools, ensure_root, pools, unique};
use huxley_db::{OrgScope, begin_scoped};
use serde_json::{Value, json};
use uuid::Uuid;

fn config(value: Value) -> CryptoConfig {
    serde_json::from_value(value).unwrap()
}

// ---------------------------------------------------------------- mock server

#[derive(Default)]
struct Mock {
    logins: AtomicUsize,
    /// Simulates the current token being revoked.
    revoked: AtomicBool,
}

async fn login(
    State(mock): State<Arc<Mock>>,
    Json(body): Json<Value>,
) -> (StatusCode, Json<Value>) {
    if body["role_id"] != "huxley" || body["secret_id"] != "s3cret" {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "errors": ["invalid role or secret ID"] })),
        );
    }
    let n = mock.logins.fetch_add(1, Ordering::SeqCst) + 1;
    mock.revoked.store(false, Ordering::SeqCst);
    (
        StatusCode::OK,
        Json(json!({ "auth": { "client_token": format!("token-{n}"), "lease_duration": 3600 } })),
    )
}

async fn transit(
    State(mock): State<Arc<Mock>>,
    Path((op, key)): Path<(String, String)>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> (StatusCode, Json<Value>) {
    let current = format!("token-{}", mock.logins.load(Ordering::SeqCst));
    if mock.revoked.load(Ordering::SeqCst)
        || headers.get("x-vault-token").and_then(|v| v.to_str().ok()) != Some(&current)
    {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({ "errors": ["permission denied"] })),
        );
    }
    assert_eq!(key, "huxley");
    let data = match op.as_str() {
        // Not encryption: the mock only checks the plumbing.
        "encrypt" => {
            json!({ "ciphertext": format!("vault:v1:{}", body["plaintext"].as_str().unwrap()) })
        }
        "decrypt" => {
            json!({ "plaintext": body["ciphertext"].as_str().unwrap().strip_prefix("vault:v1:").unwrap() })
        }
        _ => return (StatusCode::NOT_FOUND, Json(json!({ "errors": [] }))),
    };
    (StatusCode::OK, Json(json!({ "data": data })))
}

async fn mock_server() -> (String, Arc<Mock>) {
    let mock = Arc::new(Mock::default());
    let app = axum::Router::new()
        .route("/v1/auth/approle/login", post(login))
        .route("/v1/transit/{op}/{key}", post(transit))
        .with_state(mock.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (address, mock)
}

#[tokio::test]
async fn approle_logs_in_once_and_again_after_revocation() {
    let (address, mock) = mock_server().await;
    let ring = config(json!({
        "vault": {
            "address": address,
            "approle": { "role_id": "huxley", "secret_id": "s3cret" },
        },
    }))
    .key_ring()
    .unwrap();
    assert_eq!(ring.active_id(), "vault:transit/huxley");

    ring.check().await.unwrap();
    ring.check().await.unwrap();
    assert_eq!(mock.logins.load(Ordering::SeqCst), 1, "the token is reused");

    mock.revoked.store(true, Ordering::SeqCst);
    ring.check().await.unwrap();
    assert_eq!(mock.logins.load(Ordering::SeqCst), 2, "logged in again");
}

#[tokio::test]
async fn bad_credentials_and_unreachable_servers_are_reported() {
    let (address, _) = mock_server().await;
    let wrong = config(json!({
        "vault": { "address": address, "approle": { "role_id": "huxley", "secret_id": "nope" } },
    }))
    .key_ring()
    .unwrap();
    let err = wrong.check().await.unwrap_err();
    assert!(
        matches!(&err, CryptoError::Kms(m) if m.contains("invalid role or secret ID")),
        "{err}"
    );
    // A static token that is refused isn't retried.
    let refused = config(json!({ "vault": { "address": address, "token": "stale" } }))
        .key_ring()
        .unwrap();
    assert!(matches!(refused.check().await, Err(CryptoError::Kms(m)) if m.contains("403")));

    let down = config(json!({
        "vault": { "address": "http://127.0.0.1:9", "token": "t", "timeout_secs": 2 },
    }))
    .key_ring()
    .unwrap();
    assert!(matches!(down.check().await, Err(CryptoError::Kms(_))));
}

#[test]
fn choosing_the_active_kek() {
    let kek = generate_key_base64();
    let vault = json!({ "address": "http://127.0.0.1:8200", "token": "t" });
    // One configured: that one.
    let ring = config(json!({ "vault": vault })).key_ring().unwrap();
    assert_eq!(ring.active_id(), "vault:transit/huxley");
    // Both: must say which; the other stays readable.
    assert!(matches!(
        config(json!({ "kek": kek, "vault": vault })).key_ring(),
        Err(CryptoError::AmbiguousKek)
    ));
    let ring = config(json!({ "kek": kek, "vault": vault, "active": "vault" }))
        .key_ring()
        .unwrap();
    assert_eq!(ring.active_id(), "vault:transit/huxley");
    assert_eq!(ring.retired_ids().count(), 1);
    let ring = config(json!({ "kek": kek, "vault": vault, "active": "local" }))
        .key_ring()
        .unwrap();
    assert_eq!(
        ring.retired_ids().collect::<Vec<_>>(),
        ["vault:transit/huxley"]
    );
    // Naming one that isn't configured.
    assert!(
        config(json!({ "kek": kek, "active": "vault" }))
            .key_ring()
            .is_err()
    );
    assert!(matches!(
        config(json!({})).key_ring(),
        Err(CryptoError::MissingKek)
    ));
}

// ---------------------------------------------------------------- real server

struct Bao {
    address: String,
    token: String,
    client: reqwest::Client,
}

impl Bao {
    async fn connect() -> Option<Self> {
        let (Ok(address), Ok(token)) = (
            std::env::var("HUXLEY_TEST_VAULT_ADDR"),
            std::env::var("HUXLEY_TEST_VAULT_TOKEN"),
        ) else {
            assert!(
                std::env::var_os("HUXLEY_REQUIRE_VAULT_TESTS").is_none(),
                "HUXLEY_REQUIRE_VAULT_TESTS is set but HUXLEY_TEST_VAULT_ADDR/_TOKEN are not"
            );
            eprintln!("skipping: HUXLEY_TEST_VAULT_ADDR not set");
            return None;
        };
        let bao = Self {
            address,
            token,
            client: reqwest::Client::new(),
        };
        // Mount Transit once; "already in use" afterwards is fine.
        bao.post("sys/mounts/transit", &json!({ "type": "transit" }))
            .await;
        Some(bao)
    }

    async fn post(&self, path: &str, body: &Value) -> StatusCode {
        let status = self
            .client
            .post(format!("{}/v1/{path}", self.address))
            .header("X-Vault-Token", &self.token)
            .json(body)
            .send()
            .await
            .unwrap()
            .status();
        StatusCode::from_u16(status.as_u16()).unwrap()
    }

    /// A fresh Transit key, so tests don't share one.
    async fn key(&self) -> String {
        let name = format!("huxley-test-{}", Uuid::now_v7().simple());
        let status = self.post(&format!("transit/keys/{name}"), &json!({})).await;
        assert!(status.is_success(), "creating {name}: {status}");
        name
    }

    fn config(&self, key: &str) -> Value {
        json!({ "address": self.address, "token": self.token, "key": key })
    }
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
    .bind(unique("vault"))
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    id
}

async fn stored_keys(pools: &TestPools, org: Uuid) -> Vec<(String, Vec<u8>)> {
    sqlx::query_as("SELECT kek_id, wrapped_key FROM data_keys WHERE org_id = $1")
        .bind(org)
        .fetch_all(&pools.owner)
        .await
        .unwrap()
}

const fn binding(record: Uuid) -> Binding<'static> {
    Binding {
        purpose: "vault_test",
        record,
    }
}

#[tokio::test]
async fn data_keys_are_wrapped_by_vault() {
    let Some(bao) = Bao::connect().await else {
        return;
    };
    let Some(pools) = pools().await else { return };
    let key = bao.key().await;
    let crypto = Crypto::new(
        pools.app.clone(),
        config(json!({ "vault": bao.config(&key) }))
            .key_ring()
            .unwrap(),
    );
    let org = provider(&pools).await;
    let record = Uuid::now_v7();
    let sealed = crypto
        .encrypt(org, binding(record), b"secret")
        .await
        .unwrap();

    let stored = stored_keys(&pools, org).await;
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0].0, format!("vault:transit/{key}"));
    assert!(stored[0].1.starts_with(b"vault:v1:"));

    // A fresh process unwraps through Vault.
    let other = Crypto::new(
        pools.app.clone(),
        config(json!({ "vault": bao.config(&key) }))
            .key_ring()
            .unwrap(),
    );
    assert_eq!(
        &**other.decrypt(binding(record), &sealed).await.unwrap(),
        b"secret"
    );

    // A wrapped key moved onto another org's row is refused.
    let victim = provider(&pools).await;
    other.encrypt(victim, binding(record), b"x").await.unwrap();
    sqlx::query("UPDATE data_keys SET wrapped_key = $1 WHERE org_id = $2")
        .bind(&stored[0].1)
        .bind(victim)
        .execute(&pools.owner)
        .await
        .unwrap();
    let fresh = Crypto::new(
        pools.app.clone(),
        config(json!({ "vault": bao.config(&key) }))
            .key_ring()
            .unwrap(),
    );
    let sealed_victim = fresh.encrypt(victim, binding(record), b"y").await;
    assert!(matches!(sealed_victim, Err(CryptoError::Decrypt)));
}

#[tokio::test]
async fn moving_from_a_local_kek_to_vault_and_rotating() {
    let Some(bao) = Bao::connect().await else {
        return;
    };
    let Some(pools) = pools().await else { return };
    let key = bao.key().await;
    let local = generate_key_base64();
    let ctx = RequestContext::system(Via::Cli);
    let org = provider(&pools).await;
    let record = Uuid::now_v7();
    let sealed = Crypto::new(
        pools.app.clone(),
        config(json!({ "kek": local })).key_ring().unwrap(),
    )
    .encrypt(org, binding(record), b"before")
    .await
    .unwrap();

    // Both configured, Vault active: re-wrap moves the key over.
    let both = Crypto::new(
        pools.app.clone(),
        config(json!({ "kek": local, "vault": bao.config(&key), "active": "vault" }))
            .key_ring()
            .unwrap(),
    );
    assert_eq!(both.rewrap(&ctx, Some(org), false).await.unwrap(), 1);
    assert_eq!(
        stored_keys(&pools, org).await[0].0,
        format!("vault:transit/{key}")
    );

    // The local KEK can go.
    let vault_only = Crypto::new(
        pools.app.clone(),
        config(json!({ "vault": bao.config(&key) }))
            .key_ring()
            .unwrap(),
    );
    assert_eq!(
        &**vault_only.decrypt(binding(record), &sealed).await.unwrap(),
        b"before"
    );

    // After rotating the key in Vault, --all moves data keys to the new version.
    assert!(
        bao.post(&format!("transit/keys/{key}/rotate"), &json!({}))
            .await
            .is_success()
    );
    assert_eq!(vault_only.rewrap(&ctx, Some(org), false).await.unwrap(), 0);
    assert_eq!(vault_only.rewrap(&ctx, Some(org), true).await.unwrap(), 1);
    assert!(
        stored_keys(&pools, org).await[0]
            .1
            .starts_with(b"vault:v2:")
    );
    let after = Crypto::new(
        pools.app.clone(),
        config(json!({ "vault": bao.config(&key) }))
            .key_ring()
            .unwrap(),
    );
    assert_eq!(
        &**after.decrypt(binding(record), &sealed).await.unwrap(),
        b"before"
    );
}
