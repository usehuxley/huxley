//! Organization tree behavior against a real PostgreSQL (see huxley-db's
//! `testing` module for how these run and skip).

use huxley_core::{RequestContext, Via};
use huxley_db::audit::{ChainStatus, verify_chain, verify_chain_in};
use huxley_db::testing::{TestPools, ensure_root, pools, unique};
use huxley_db::{OrgScope, begin_scoped};
use huxley_orgs::{OrgError, OrgKind, OrgStatus, Organization};
use uuid::Uuid;

fn ctx() -> RequestContext {
    RequestContext::system(Via::System)
}

/// Creates `parent > child` under the root in system scope and commits.
async fn create(pools: &TestPools, parent: Uuid, kind: OrgKind, name: &str) -> Organization {
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    let org = huxley_orgs::create(&mut tx, &ctx(), parent, kind, name)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    org
}

#[tokio::test]
async fn creates_children_and_reports_depth_and_ancestors() {
    let Some(pools) = pools().await else { return };
    let root = ensure_root(&pools).await;

    let msp = create(&pools, root, OrgKind::Provider, &unique("MSP")).await;
    let client = create(&pools, msp.id, OrgKind::Client, &unique("Client")).await;
    let site = create(&pools, client.id, OrgKind::Client, &unique("Site")).await;

    assert_eq!(msp.depth, 2);
    assert_eq!(site.depth, 4);
    assert_eq!(site.status, OrgStatus::Active);

    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    let chain: Vec<Uuid> = huxley_orgs::ancestors(&mut tx, site.id)
        .await
        .unwrap()
        .iter()
        .map(|o| o.id)
        .collect();
    assert_eq!(chain, vec![root, msp.id, client.id, site.id]);

    let kids = huxley_orgs::children(&mut tx, msp.id).await.unwrap();
    assert_eq!(kids.len(), 1);
    assert_eq!(kids[0].id, client.id);
}

#[tokio::test]
async fn moving_an_org_rewrites_its_whole_subtree() {
    let Some(pools) = pools().await else { return };
    let root = ensure_root(&pools).await;

    let a = create(&pools, root, OrgKind::Provider, &unique("A")).await;
    let b = create(&pools, root, OrgKind::Provider, &unique("B")).await;
    let client = create(&pools, a.id, OrgKind::Client, &unique("Client")).await;
    let site = create(&pools, client.id, OrgKind::Client, &unique("Site")).await;

    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    let moved = huxley_orgs::move_to(&mut tx, &ctx(), client.id, client.version, b.id)
        .await
        .unwrap();
    assert_eq!(moved.parent_id, Some(b.id));
    assert_eq!(moved.version, client.version + 1);

    let chain: Vec<Uuid> = huxley_orgs::ancestors(&mut tx, site.id)
        .await
        .unwrap()
        .iter()
        .map(|o| o.id)
        .collect();
    assert_eq!(chain, vec![root, b.id, client.id, site.id]);
    assert_eq!(huxley_orgs::subtree(&mut tx, a.id).await.unwrap().len(), 1);
}

#[tokio::test]
async fn rejects_cycles_and_providers_under_clients() {
    let Some(pools) = pools().await else { return };
    let root = ensure_root(&pools).await;

    let msp = create(&pools, root, OrgKind::Provider, &unique("MSP")).await;
    let client = create(&pools, msp.id, OrgKind::Client, &unique("Client")).await;

    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    let cycle = huxley_orgs::move_to(&mut tx, &ctx(), msp.id, msp.version, client.id).await;
    assert!(matches!(cycle, Err(OrgError::Cycle)), "got {cycle:?}");
    tx.rollback().await.unwrap();

    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    let bad = huxley_orgs::create(&mut tx, &ctx(), client.id, OrgKind::Provider, "Nope").await;
    assert!(
        matches!(bad, Err(OrgError::ProviderUnderClient)),
        "got {bad:?}"
    );
}

#[tokio::test]
async fn sibling_names_are_unique_case_insensitively() {
    let Some(pools) = pools().await else { return };
    let root = ensure_root(&pools).await;
    let msp = create(&pools, root, OrgKind::Provider, &unique("MSP")).await;
    create(&pools, msp.id, OrgKind::Client, "Contoso").await;

    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    let dup = huxley_orgs::create(&mut tx, &ctx(), msp.id, OrgKind::Client, "CONTOSO").await;
    assert!(matches!(dup, Err(OrgError::NameTaken)), "got {dup:?}");
}

#[tokio::test]
async fn only_one_root_and_it_cannot_move_or_archive() {
    let Some(pools) = pools().await else { return };
    let root = ensure_root(&pools).await;

    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    let second = huxley_orgs::create_root(&mut tx, &ctx(), &unique("Another root")).await;
    assert!(
        matches!(second, Err(OrgError::RootExists)),
        "got {second:?}"
    );
    tx.rollback().await.unwrap();

    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    let root_org = huxley_orgs::get(&mut tx, root).await.unwrap();
    let archive = huxley_orgs::archive(&mut tx, &ctx(), root, root_org.version).await;
    assert!(matches!(archive, Err(OrgError::RootImmutable)));
}

#[tokio::test]
async fn archive_requires_no_active_children_and_checks_version() {
    let Some(pools) = pools().await else { return };
    let root = ensure_root(&pools).await;
    let msp = create(&pools, root, OrgKind::Provider, &unique("MSP")).await;
    let client = create(&pools, msp.id, OrgKind::Client, &unique("Client")).await;

    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    let blocked = huxley_orgs::archive(&mut tx, &ctx(), msp.id, msp.version).await;
    assert!(matches!(blocked, Err(OrgError::HasActiveChildren)));

    let stale = huxley_orgs::archive(&mut tx, &ctx(), client.id, client.version + 7).await;
    assert!(matches!(stale, Err(OrgError::VersionConflict)));

    let archived = huxley_orgs::archive(&mut tx, &ctx(), client.id, client.version)
        .await
        .unwrap();
    assert_eq!(archived.status, OrgStatus::Archived);
}

#[tokio::test]
async fn row_level_security_hides_orgs_outside_the_scope() {
    let Some(pools) = pools().await else { return };
    let root = ensure_root(&pools).await;
    let mine = create(&pools, root, OrgKind::Provider, &unique("Mine")).await;
    let theirs = create(&pools, root, OrgKind::Provider, &unique("Theirs")).await;

    let mut tx = begin_scoped(&pools.app, &OrgScope::Orgs(vec![mine.id]))
        .await
        .unwrap();
    assert!(huxley_orgs::get(&mut tx, mine.id).await.is_ok());
    assert!(matches!(
        huxley_orgs::get(&mut tx, theirs.id).await,
        Err(OrgError::NotFound)
    ));
    // Cannot create under an org outside the scope either.
    let under_theirs =
        huxley_orgs::create(&mut tx, &ctx(), theirs.id, OrgKind::Client, "Sneaky").await;
    assert!(matches!(under_theirs, Err(OrgError::NotFound)));
    tx.rollback().await.unwrap();

    // A new child of an in-scope org is readable within the same transaction.
    let mut tx = begin_scoped(&pools.app, &OrgScope::Orgs(vec![mine.id]))
        .await
        .unwrap();
    let child = huxley_orgs::create(&mut tx, &ctx(), mine.id, OrgKind::Client, &unique("Kid"))
        .await
        .unwrap();
    assert_eq!(child.parent_id, Some(mine.id));
    tx.commit().await.unwrap();

    // An empty scope sees nothing, not everything.
    let mut tx = begin_scoped(&pools.app, &OrgScope::Orgs(vec![]))
        .await
        .unwrap();
    assert!(huxley_orgs::root(&mut tx).await.unwrap().is_none());
}

#[tokio::test]
async fn mutations_write_audit_records_and_the_chain_verifies() {
    let Some(pools) = pools().await else { return };
    let root = ensure_root(&pools).await;
    let org = create(&pools, root, OrgKind::Provider, &unique("Audited")).await;

    let (count,): (i64,) = sqlx::query_as(
        "SELECT count(*) FROM audit_events WHERE target_id = $1 AND action = 'org.created'",
    )
    .bind(org.id)
    .fetch_one(&pools.owner)
    .await
    .unwrap();
    assert_eq!(count, 1);

    let (events,): (i64,) = sqlx::query_as(
        "SELECT count(*) FROM events_outbox WHERE subject_id = $1 AND event_type = 'org.created'",
    )
    .bind(org.id)
    .fetch_one(&pools.owner)
    .await
    .unwrap();
    assert_eq!(events, 1);

    assert!(matches!(
        verify_chain(&pools.owner).await.unwrap(),
        ChainStatus::Intact { .. }
    ));
}

#[tokio::test]
async fn tampering_with_an_audit_record_breaks_the_chain() {
    let Some(pools) = pools().await else { return };
    let root = ensure_root(&pools).await;
    let org = create(&pools, root, OrgKind::Provider, &unique("Tampered")).await;

    // Tamper inside a transaction that is rolled back, so other tests sharing
    // the database never see the damage.
    let mut tx = pools.owner.begin().await.unwrap();
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ")
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query(
        "UPDATE audit_events SET details = '{\"forged\": true}' \
         WHERE target_id = $1 AND action = 'org.created'",
    )
    .bind(org.id)
    .execute(&mut *tx)
    .await
    .unwrap();
    let status = verify_chain_in(&mut tx).await.unwrap();
    tx.rollback().await.unwrap();

    assert!(
        matches!(status, ChainStatus::Broken { reason, .. } if reason.contains("content")),
        "got {status:?}"
    );
}

#[tokio::test]
async fn the_application_role_cannot_rewrite_audit_history() {
    let Some(pools) = pools().await else { return };
    let result = sqlx::query("UPDATE audit_events SET details = '{}'")
        .execute(&pools.app)
        .await;
    let err = result.expect_err("app role must not update audit_events");
    assert!(err.to_string().contains("permission denied"), "got {err}");

    let result = sqlx::query("DELETE FROM audit_events")
        .execute(&pools.app)
        .await;
    assert!(result.is_err(), "app role must not delete audit_events");
}
