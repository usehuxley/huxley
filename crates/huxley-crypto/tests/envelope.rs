//! Envelope encryption against a real PostgreSQL. Skips unless
//! `HUXLEY_TEST_DATABASE_URL` is set (see `huxley_db::testing`).

use huxley_core::{RequestContext, Via};
use huxley_crypto::{Binding, Crypto, CryptoError, KeyRing, generate_key_base64};
use huxley_db::testing::{TestPools, ensure_root, pools, unique};
use huxley_db::{OrgScope, begin_scoped};
use secrecy::SecretString;
use uuid::Uuid;

fn ring(active: &str, retired: &[&str]) -> KeyRing {
    let retired: Vec<SecretString> = retired.iter().map(|k| SecretString::from(*k)).collect();
    KeyRing::new(&SecretString::from(active), &retired).unwrap()
}

/// A fresh provider org directly under the root, so each test owns its keys.
async fn provider(pools: &TestPools) -> Uuid {
    org(pools, ensure_root(pools).await, "provider").await
}

async fn org(pools: &TestPools, parent: Uuid, kind: &str) -> Uuid {
    let id = Uuid::now_v7();
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    sqlx::query(
        "INSERT INTO organizations (id, parent_id, kind, name, path) \
         VALUES ($1, $2, $3::org_kind, $4, '')",
    )
    .bind(id)
    .bind(parent)
    .bind(kind)
    .bind(unique("crypto"))
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    id
}

async fn active_keys(pools: &TestPools, org: Uuid) -> Vec<(Uuid, String)> {
    sqlx::query_as("SELECT id, kek_id FROM data_keys WHERE org_id = $1 AND status = 'active'")
        .bind(org)
        .fetch_all(&pools.owner)
        .await
        .unwrap()
}

fn binding(record: Uuid) -> Binding<'static> {
    Binding {
        purpose: "test_secret",
        record,
    }
}

#[tokio::test]
async fn round_trip_creates_one_key_per_provider_org() {
    let Some(pools) = pools().await else { return };
    let kek = generate_key_base64();
    let crypto = Crypto::new(pools.app.clone(), ring(&kek, &[]));
    let provider = provider(&pools).await;
    let client = org(&pools, provider, "client").await;
    let record = Uuid::now_v7();

    let sealed = crypto
        .encrypt(client, binding(record), b"hunter2")
        .await
        .unwrap();
    assert!(!sealed.windows(7).any(|w| w == b"hunter2"));
    assert_eq!(
        &**crypto.decrypt(binding(record), &sealed).await.unwrap(),
        b"hunter2"
    );

    // The client's value used its provider's key, created on first use.
    let keys = active_keys(&pools, provider).await;
    assert_eq!(keys.len(), 1);
    assert_eq!(&sealed[1..17], keys[0].0.as_bytes());
    assert_eq!(active_keys(&pools, client).await, vec![]);

    // A second process with the same KEK reads it from the database.
    let other = Crypto::new(pools.app.clone(), ring(&kek, &[]));
    assert_eq!(
        &**other.decrypt(binding(record), &sealed).await.unwrap(),
        b"hunter2"
    );
    other
        .encrypt(provider, binding(record), b"x")
        .await
        .unwrap();
    assert_eq!(active_keys(&pools, provider).await.len(), 1);
}

#[tokio::test]
async fn sibling_providers_get_separate_keys() {
    let Some(pools) = pools().await else { return };
    let crypto = Crypto::new(pools.app.clone(), ring(&generate_key_base64(), &[]));
    let (a, b) = (provider(&pools).await, provider(&pools).await);
    let record = Uuid::now_v7();
    let from_a = crypto.encrypt(a, binding(record), b"v").await.unwrap();
    let from_b = crypto.encrypt(b, binding(record), b"v").await.unwrap();
    assert_ne!(from_a[1..17], from_b[1..17]);
}

#[tokio::test]
async fn binding_mismatch_fails() {
    let Some(pools) = pools().await else { return };
    let crypto = Crypto::new(pools.app.clone(), ring(&generate_key_base64(), &[]));
    let provider = provider(&pools).await;
    let record = Uuid::now_v7();
    let sealed = crypto
        .encrypt(provider, binding(record), b"v")
        .await
        .unwrap();

    let moved = crypto.decrypt(binding(Uuid::now_v7()), &sealed).await;
    assert!(matches!(moved, Err(CryptoError::Decrypt)));
    let repurposed = Binding {
        purpose: "other",
        record,
    };
    assert!(matches!(
        crypto.decrypt(repurposed, &sealed).await,
        Err(CryptoError::Decrypt)
    ));
    assert!(matches!(
        crypto.decrypt(binding(record), b"\x02garbage").await,
        Err(CryptoError::Decrypt)
    ));
}

#[tokio::test]
async fn wrong_kek_is_reported() {
    let Some(pools) = pools().await else { return };
    let kek = generate_key_base64();
    let provider = provider(&pools).await;
    let record = Uuid::now_v7();
    let sealed = Crypto::new(pools.app.clone(), ring(&kek, &[]))
        .encrypt(provider, binding(record), b"v")
        .await
        .unwrap();

    let stranger = Crypto::new(pools.app.clone(), ring(&generate_key_base64(), &[]));
    assert!(matches!(
        stranger.decrypt(binding(record), &sealed).await,
        Err(CryptoError::UnknownKek(_))
    ));
    assert!(matches!(
        stranger.verify_keys().await,
        Err(CryptoError::UnknownKek(_))
    ));
}

#[tokio::test]
async fn rewrap_moves_keys_to_the_new_kek() {
    let Some(pools) = pools().await else { return };
    let old = generate_key_base64();
    let new = generate_key_base64();
    let provider = provider(&pools).await;
    let record = Uuid::now_v7();
    let sealed = Crypto::new(pools.app.clone(), ring(&old, &[]))
        .encrypt(provider, binding(record), b"v")
        .await
        .unwrap();
    let old_id = active_keys(&pools, provider).await[0].1.clone();
    let ctx = RequestContext::system(Via::Cli);

    // Without the old KEK nothing can be re-wrapped, and nothing changes.
    let lost = Crypto::new(pools.app.clone(), ring(&new, &[]));
    assert!(matches!(
        lost.rewrap(&ctx, Some(provider), false).await,
        Err(CryptoError::UnknownKek(_))
    ));
    assert_eq!(active_keys(&pools, provider).await[0].1, old_id);

    // Other tests share the database with keys under KEKs this test doesn't
    // hold, so re-wrap only this test's org.
    let rotated = Crypto::new(pools.app.clone(), ring(&new, &[&old]));
    assert_eq!(
        rotated.rewrap(&ctx, Some(provider), false).await.unwrap(),
        1
    );
    assert_ne!(active_keys(&pools, provider).await[0].1, old_id);
    assert_eq!(
        rotated.rewrap(&ctx, Some(provider), false).await.unwrap(),
        0
    );

    // The old KEK is no longer needed.
    let new_only = Crypto::new(pools.app.clone(), ring(&new, &[]));
    assert_eq!(
        &**new_only.decrypt(binding(record), &sealed).await.unwrap(),
        b"v"
    );
}

#[tokio::test]
async fn keys_belong_to_provider_orgs_and_cannot_be_deleted() {
    let Some(pools) = pools().await else { return };
    let provider = provider(&pools).await;
    let client = org(&pools, provider, "client").await;
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    let err = sqlx::query(
        "INSERT INTO data_keys (id, org_id, kek_id, wrapped_key) \
         VALUES ($1, $2, '0123456789abcdef', '\\x00')",
    )
    .bind(Uuid::now_v7())
    .bind(client)
    .execute(&mut *tx)
    .await
    .unwrap_err();
    assert_eq!(
        huxley_db::violated_constraint(&err),
        Some("data_keys_provider_org")
    );
    drop(tx);

    let crypto = Crypto::new(pools.app.clone(), ring(&generate_key_base64(), &[]));
    crypto
        .encrypt(provider, binding(Uuid::now_v7()), b"v")
        .await
        .unwrap();
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    let denied = sqlx::query("DELETE FROM data_keys WHERE org_id = $1")
        .bind(provider)
        .execute(&mut *tx)
        .await;
    assert!(denied.is_err(), "the app role must not delete data keys");
}
