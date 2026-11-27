//! Key-encryption keys in AWS KMS (ADR-092), against a mock of KMS's JSON
//! API (the "encryption" is not real: these tests check signed requests,
//! encryption context, the key named on decrypt and the data-key frame,
//! not AWS's cryptography).

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use axum::Json;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::routing::post;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use huxley_core::{RequestContext, Via};
use huxley_crypto::{Binding, Crypto, CryptoConfig, CryptoError, generate_key_base64};
use huxley_db::testing::{TestPools, ensure_root, pools, unique};
use huxley_db::{OrgScope, begin_scoped};
use serde_json::{Value, json};
use uuid::Uuid;

const KEY: &str = "alias/huxley";
const ARN: &str = "arn:aws:kms:us-east-1:111122223333:key/1234abcd-12ab-34cd-56ef-1234567890ab";

#[derive(Default)]
struct Mock {
    calls: AtomicUsize,
}

fn error(kind: &str, message: &str) -> (StatusCode, HeaderMap, Json<Value>) {
    (
        StatusCode::BAD_REQUEST,
        HeaderMap::new(),
        Json(json!({ "__type": kind, "message": message })),
    )
}

/// KMS's JSON 1.1 API: `X-Amz-Target: TrentService.<Operation>`.
async fn kms(
    State(mock): State<Arc<Mock>>,
    headers: HeaderMap,
    body: Bytes,
) -> (StatusCode, HeaderMap, Json<Value>) {
    mock.calls.fetch_add(1, Ordering::SeqCst);
    let auth = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if !auth.starts_with("AWS4-HMAC-SHA256 Credential=AKIATESTKEY/")
        || !auth.contains("/us-east-1/kms/aws4_request")
    {
        return error(
            "UnrecognizedClientException",
            "The security token included in the request is invalid.",
        );
    }
    let body: Value = serde_json::from_slice(&body).unwrap();
    if body["EncryptionContext"] != json!({ "huxley": "data-key" }) {
        return error("InvalidCiphertextException", "context mismatch");
    }
    let target = headers
        .get("x-amz-target")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let mut out = HeaderMap::new();
    out.insert(
        "content-type",
        "application/x-amz-json-1.1".parse().unwrap(),
    );
    match target {
        "TrentService.Encrypt" => {
            if body["KeyId"] != KEY {
                return error("NotFoundException", "Alias is not found.");
            }
            let mut plain = STANDARD
                .decode(body["Plaintext"].as_str().unwrap())
                .unwrap();
            plain.reverse();
            let mut blob = b"mockkms:".to_vec();
            blob.extend(plain);
            (
                StatusCode::OK,
                out,
                Json(
                    json!({ "CiphertextBlob": STANDARD.encode(blob), "KeyId": ARN, "EncryptionAlgorithm": "SYMMETRIC_DEFAULT" }),
                ),
            )
        }
        "TrentService.Decrypt" => {
            if body["KeyId"] != KEY {
                return error(
                    "IncorrectKeyException",
                    "The key ID in the request does not identify a KMS key that can perform this operation.",
                );
            }
            let blob = STANDARD
                .decode(body["CiphertextBlob"].as_str().unwrap())
                .unwrap();
            let Some(rest) = blob.strip_prefix(b"mockkms:") else {
                return error("InvalidCiphertextException", "");
            };
            let mut plain = rest.to_vec();
            plain.reverse();
            (
                StatusCode::OK,
                out,
                Json(
                    json!({ "Plaintext": STANDARD.encode(plain), "KeyId": ARN, "EncryptionAlgorithm": "SYMMETRIC_DEFAULT" }),
                ),
            )
        }
        _ => error("UnknownOperationException", target),
    }
}

async fn mock_server() -> (String, Arc<Mock>) {
    let mock = Arc::new(Mock::default());
    let app = axum::Router::new()
        .route("/", post(kms))
        .with_state(mock.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (address, mock)
}

fn aws(address: &str, secret_key: &str) -> Value {
    json!({
        "key_id": KEY,
        "region": "us-east-1",
        "endpoint_url": address,
        "access_key_id": "AKIATESTKEY",
        "secret_access_key": secret_key,
    })
}

fn config(value: Value) -> CryptoConfig {
    serde_json::from_value(value).unwrap()
}

#[tokio::test]
async fn requests_are_signed_and_bound_to_their_context() {
    let (address, mock) = mock_server().await;
    let ring = config(json!({ "aws_kms": aws(&address, "secret") }))
        .key_ring()
        .unwrap();
    assert_eq!(ring.active_id(), "aws:alias/huxley");
    ring.check().await.unwrap();
    assert!(
        mock.calls.load(Ordering::SeqCst) >= 2,
        "a check wraps and unwraps"
    );

    let unknown = config(json!({ "aws_kms": { "key_id": "alias/other", "region": "us-east-1", "endpoint_url": address,
        "access_key_id": "AKIATESTKEY", "secret_access_key": "secret" } }))
    .key_ring()
    .unwrap();
    let err = unknown.check().await.unwrap_err();
    assert!(
        matches!(&err, CryptoError::Kms(m) if m.contains("Alias is not found")),
        "{err}"
    );
}

async fn provider(pools: &TestPools) -> Uuid {
    let id = Uuid::now_v7();
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    sqlx::query("INSERT INTO organizations (id, parent_id, kind, name, path) VALUES ($1, $2, 'provider', $3, '')")
        .bind(id)
        .bind(ensure_root(pools).await)
        .bind(unique("aws"))
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    id
}

#[tokio::test]
async fn data_keys_move_to_kms_and_still_decrypt() {
    let Some(pools) = pools().await else { return };
    let (address, _) = mock_server().await;
    let local = generate_key_base64();
    let ctx = RequestContext::system(Via::Cli);
    let org = provider(&pools).await;
    let binding = Binding {
        purpose: "aws_test",
        record: Uuid::now_v7(),
    };
    let sealed = Crypto::new(
        pools.app.clone(),
        config(json!({ "kek": local })).key_ring().unwrap(),
    )
    .encrypt(org, binding, b"secret")
    .await
    .unwrap();

    let both = Crypto::new(
        pools.app.clone(),
        config(json!({ "kek": local, "aws_kms": aws(&address, "secret"), "active": "aws_kms" }))
            .key_ring()
            .unwrap(),
    );
    assert_eq!(both.rewrap(&ctx, Some(org), false).await.unwrap(), 1);
    let (kek_id, wrapped): (String, Vec<u8>) =
        sqlx::query_as("SELECT kek_id, wrapped_key FROM data_keys WHERE org_id = $1")
            .bind(org)
            .fetch_one(&pools.owner)
            .await
            .unwrap();
    assert_eq!(kek_id, "aws:alias/huxley");
    assert!(wrapped.starts_with(b"awskms1:mockkms:"));

    // KMS alone now reads it.
    let kms_only = Crypto::new(
        pools.app.clone(),
        config(json!({ "aws_kms": aws(&address, "secret") }))
            .key_ring()
            .unwrap(),
    );
    assert_eq!(
        &**kms_only.decrypt(binding, &sealed).await.unwrap(),
        b"secret"
    );
}
