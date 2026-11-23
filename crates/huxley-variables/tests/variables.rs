//! Variables against a real PostgreSQL. Each test builds its own org tree.

use huxley_core::{RequestContext, Via};
use huxley_crypto::Crypto;
use huxley_db::testing::{TestPools, ensure_root, pools, unique};
use huxley_db::{OrgScope, begin_scoped};
use huxley_orgs::OrgKind;
use huxley_variables::{VariableError, VariableInput, VariableType};
use serde_json::{Value, json};
use sqlx::PgConnection;
use uuid::Uuid;

fn ctx() -> RequestContext {
    RequestContext::system(Via::System)
}

fn input(kind: VariableType, value: Value, locked: bool) -> VariableInput {
    VariableInput {
        kind,
        value,
        description: None,
        locked,
    }
}

/// msp → client → site.
async fn tree(tx: &mut PgConnection, pools: &TestPools) -> (Uuid, Uuid, Uuid) {
    let root = ensure_root(pools).await;
    let msp = huxley_orgs::create(tx, &ctx(), root, OrgKind::Provider, &unique("Msp"))
        .await
        .unwrap();
    let client = huxley_orgs::create(tx, &ctx(), msp.id, OrgKind::Client, &unique("Client"))
        .await
        .unwrap();
    let site = huxley_orgs::create(tx, &ctx(), client.id, OrgKind::Client, &unique("Site"))
        .await
        .unwrap();
    (msp.id, client.id, site.id)
}

#[tokio::test]
async fn nearest_wins_locks_win_and_secrets_stay_hidden() {
    let Some(pools) = pools().await else { return };
    let crypto = Crypto::new(pools.app.clone(), huxley_crypto::testing::key_ring());
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    let (msp, client, site) = tree(&mut tx, &pools).await;
    tx.commit().await.unwrap();

    // One change per transaction, as the API makes them.
    for (org, key, value) in [
        (
            msp,
            "domain",
            input(VariableType::String, json!("msp.example"), false),
        ),
        (
            client,
            "domain",
            input(VariableType::String, json!("client.example"), false),
        ),
        (msp, "seats", input(VariableType::Number, json!(25), true)),
        (
            msp,
            "api_token",
            input(VariableType::Secret, json!("tok-123456"), false),
        ),
    ] {
        let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
        huxley_variables::set(&mut tx, &ctx(), &crypto, org, key, &value)
            .await
            .unwrap();
        tx.commit().await.unwrap();
    }

    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    let at_site = huxley_variables::resolve(&mut tx, site).await.unwrap();
    let get = |key: &str| at_site.iter().find(|v| v.key == key).unwrap().clone();
    assert_eq!(
        get("domain").value,
        Some(json!("client.example")),
        "nearest wins"
    );
    assert_eq!(get("domain").source.id, client);
    assert_eq!(get("seats").locked_by.as_ref().unwrap().id, msp);
    assert_eq!(get("api_token").value, None, "secrets are never returned");
    assert_eq!(get("api_token").kind, VariableType::Secret);

    // Locked above: refused below, at any depth.
    for org in [client, site] {
        assert!(matches!(
            huxley_variables::set(
                &mut tx,
                &ctx(),
                &crypto,
                org,
                "seats",
                &input(VariableType::Number, json!(1), false)
            )
            .await,
            Err(VariableError::Locked(_))
        ));
    }

    // A run sees plain values and decrypted secrets.
    let run = huxley_variables::for_run(&mut tx, &crypto, site)
        .await
        .unwrap();
    assert_eq!(run.vars.get("domain"), Some(&json!("client.example")));
    assert_eq!(run.vars.get("seats"), Some(&json!(25)));
    assert!(run.vars.get("api_token").is_none());
    assert_eq!(
        run.secrets.get("api_token").map(String::as_str),
        Some("tok-123456")
    );
    assert!(!format!("{run:?}").contains("tok-123456"));

    // The secret isn't stored in the clear.
    let stored: (Option<Value>, Vec<u8>) = sqlx::query_as(
        "SELECT value, secret_sealed FROM org_variables WHERE org_id = $1 AND key = 'api_token'",
    )
    .bind(msp)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert!(stored.0.is_none());
    assert!(!stored.1.windows(10).any(|w| w == b"tok-123456"));

    // Clearing the client's value inherits the MSP's again.
    huxley_variables::clear(&mut tx, &ctx(), client, "domain")
        .await
        .unwrap();
    let run = huxley_variables::for_run(&mut tx, &crypto, site)
        .await
        .unwrap();
    assert_eq!(run.vars.get("domain"), Some(&json!("msp.example")));
    tx.commit().await.unwrap();
}

#[tokio::test]
async fn values_must_match_their_type() {
    let Some(pools) = pools().await else { return };
    let crypto = Crypto::new(pools.app.clone(), huxley_crypto::testing::key_ring());
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    let (msp, _, _) = tree(&mut tx, &pools).await;
    for (kind, value) in [
        (VariableType::Number, json!("12")),
        (VariableType::Bool, json!("yes")),
        (VariableType::String, json!(1)),
        (VariableType::Secret, json!("")),
    ] {
        assert!(
            matches!(
                huxley_variables::set(
                    &mut tx,
                    &ctx(),
                    &crypto,
                    msp,
                    "x",
                    &input(kind, value.clone(), false)
                )
                .await,
                Err(VariableError::InvalidValue(_))
            ),
            "{kind:?} {value}"
        );
    }
    assert!(matches!(
        huxley_variables::set(
            &mut tx,
            &ctx(),
            &crypto,
            msp,
            "Bad-Key",
            &input(VariableType::Json, json!({}), false)
        )
        .await,
        Err(VariableError::InvalidKey)
    ));
    huxley_variables::set(
        &mut tx,
        &ctx(),
        &crypto,
        msp,
        "config",
        &input(VariableType::Json, json!({ "a": [1] }), false),
    )
    .await
    .unwrap();
    tx.rollback().await.unwrap();
}
