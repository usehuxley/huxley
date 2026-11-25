//! The credential vault against a real PostgreSQL: secrets sealed at rest,
//! never listed, revealed with a trail, and scoped to organizations.

use huxley_core::{RequestContext, Via};
use huxley_crypto::Crypto;
use huxley_db::testing::{TestPools, ensure_root, pools, unique};
use huxley_db::{OrgScope, begin_scoped};
use huxley_vault::{Filter, ItemChanges, ItemInput, VaultError};
use uuid::Uuid;

async fn client(pools: &TestPools, crypto: &Crypto) -> Uuid {
    let root = ensure_root(pools).await;
    let ctx = RequestContext::system(Via::System);
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    let org = huxley_orgs::create(
        &mut tx,
        &ctx,
        root,
        huxley_orgs::OrgKind::Client,
        &unique("Vault client"),
    )
    .await
    .unwrap()
    .id;
    tx.commit().await.unwrap();
    // The org's first data key audits on its own: make it first.
    crypto
        .encrypt(
            org,
            huxley_crypto::Binding {
                purpose: "test",
                record: org,
            },
            b"x",
        )
        .await
        .unwrap();
    org
}

fn input(org_id: Uuid, name: &str, secret: Option<&str>) -> ItemInput {
    ItemInput {
        org_id,
        name: name.into(),
        kind: None,
        username: Some("admin@contoso.example".into()),
        url: Some("https://portal.azure.com".into()),
        notes: Some("Break-glass account".into()),
        tags: Some(vec!["Entra".into(), "entra".into()]),
        secret: secret.map(str::to_owned),
    }
}

#[tokio::test]
async fn secrets_are_sealed_revealed_with_a_trail_and_changed() {
    let Some(pools) = pools().await else { return };
    let crypto = Crypto::new(pools.app.clone(), huxley_crypto::testing::key_ring());
    let org = client(&pools, &crypto).await;
    let ctx = RequestContext::system(Via::System);
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();

    let item = huxley_vault::create(
        &mut tx,
        &ctx,
        &crypto,
        &input(org, "Global admin", Some("Tr0ub4dor&3")),
    )
    .await
    .unwrap();
    assert!(item.has_secret && item.secret_changed_at.is_some());
    assert_eq!(
        (item.kind.as_str(), item.tags.clone()),
        ("login", vec!["entra".to_owned()])
    );
    // Never listed or stored in clear.
    assert!(!serde_json::to_string(&item).unwrap().contains("Tr0ub4dor"));
    let sealed: Vec<u8> = sqlx::query_scalar("SELECT secret_sealed FROM vault_items WHERE id = $1")
        .bind(item.id)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert!(!String::from_utf8_lossy(&sealed).contains("Tr0ub4dor"));
    let found = huxley_vault::list(
        &mut tx,
        &Filter {
            q: Some("contoso".into()),
            org_id: Some(org),
            limit: 50,
        },
    )
    .await
    .unwrap();
    assert_eq!(found.iter().map(|i| i.id).collect::<Vec<_>>(), [item.id]);

    // Revealed, with who and why kept.
    let secret = huxley_vault::reveal(&mut tx, &ctx, &crypto, item.id, Some("Ticket T-1042"))
        .await
        .unwrap();
    assert_eq!(secret.as_str(), "Tr0ub4dor&3");
    let log = huxley_vault::reveals(&mut tx, Some(item.id), 10)
        .await
        .unwrap();
    assert_eq!((log.len(), log[0].reason.as_str()), (1, "Ticket T-1042"));
    assert!(
        huxley_vault::get(&mut tx, item.id)
            .await
            .unwrap()
            .last_revealed_at
            .is_some()
    );
    let audited: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_events WHERE action = 'vault.secret_revealed' AND target_id = $1",
    )
    .bind(item.id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(audited, 1);

    let item = change_and_clear(&mut tx, &ctx, &crypto, item).await;

    // Deleted: the reveal trail stays.
    huxley_vault::delete(&mut tx, &ctx, item.id).await.unwrap();
    let trail: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM vault_reveals WHERE org_id = $1 AND item_name = 'Global admin'",
    )
    .bind(org)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(trail, 2);
    tx.commit().await.unwrap();
}

/// A new secret replaces the old; a stale version is refused; other
/// changes keep the secret; clearing removes it.
async fn change_and_clear(
    tx: &mut sqlx::PgConnection,
    ctx: &RequestContext,
    crypto: &Crypto,
    item: huxley_vault::Item,
) -> huxley_vault::Item {
    // A new secret replaces it; a stale version is refused; clearing removes it.
    let item = huxley_vault::update(
        &mut *tx,
        ctx,
        crypto,
        item.id,
        &ItemChanges {
            version: item.version,
            secret: Some("correct horse".into()),
            ..ItemChanges::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(
        huxley_vault::reveal(&mut *tx, ctx, crypto, item.id, None)
            .await
            .unwrap()
            .as_str(),
        "correct horse"
    );
    let stale = ItemChanges {
        version: item.version - 1,
        name: Some("x".into()),
        ..ItemChanges::default()
    };
    assert!(matches!(
        huxley_vault::update(&mut *tx, ctx, crypto, item.id, &stale).await,
        Err(VaultError::VersionConflict)
    ));
    // Changing other fields keeps the secret.
    let item = huxley_vault::update(
        &mut *tx,
        ctx,
        crypto,
        item.id,
        &ItemChanges {
            version: item.version,
            notes: Some("Rotated".into()),
            ..ItemChanges::default()
        },
    )
    .await
    .unwrap();
    assert!(item.has_secret);
    let item = huxley_vault::update(
        &mut *tx,
        ctx,
        crypto,
        item.id,
        &ItemChanges {
            version: item.version,
            clear_secret: true,
            ..ItemChanges::default()
        },
    )
    .await
    .unwrap();
    assert!(!item.has_secret);
    assert!(matches!(
        huxley_vault::reveal(&mut *tx, ctx, crypto, item.id, None).await,
        Err(VaultError::NoSecret)
    ));
    item
}

#[tokio::test]
async fn items_are_scoped_to_their_organization() {
    let Some(pools) = pools().await else { return };
    let crypto = Crypto::new(pools.app.clone(), huxley_crypto::testing::key_ring());
    let org = client(&pools, &crypto).await;
    let other = client(&pools, &crypto).await;
    let ctx = RequestContext::system(Via::System);
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    let item = huxley_vault::create(
        &mut tx,
        &ctx,
        &crypto,
        &input(org, "Firewall", Some("s3cret")),
    )
    .await
    .unwrap();
    // A bad kind is refused.
    let mut bad = input(org, "x", None);
    bad.kind = Some("bitcoin".into());
    assert!(matches!(
        huxley_vault::create(&mut tx, &ctx, &crypto, &bad).await,
        Err(VaultError::Invalid { .. })
    ));
    tx.commit().await.unwrap();

    let mut tx = begin_scoped(&pools.app, &OrgScope::Orgs(vec![other]))
        .await
        .unwrap();
    assert!(matches!(
        huxley_vault::get(&mut tx, item.id).await,
        Err(VaultError::NotFound)
    ));
    assert!(matches!(
        huxley_vault::reveal(&mut tx, &ctx, &crypto, item.id, None).await,
        Err(VaultError::NotFound)
    ));
    let all = huxley_vault::list(
        &mut tx,
        &Filter {
            limit: 500,
            ..Filter::default()
        },
    )
    .await
    .unwrap();
    assert!(all.iter().all(|i| i.org_id == other));
    tx.commit().await.unwrap();
}
