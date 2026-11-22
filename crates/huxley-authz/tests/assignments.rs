//! Role assignments against a real PostgreSQL.

use huxley_authz::{
    AuthzError, OWNER_ROLE, assign, list_roles, load_principal, unassign, visible_orgs,
};
use huxley_core::{Actor, RequestContext, Via};
use huxley_db::testing::{TestPools, ensure_root, pools, unique};
use huxley_db::{OrgScope, begin_scoped};
use huxley_orgs::OrgKind;
use uuid::Uuid;

fn ctx() -> RequestContext {
    RequestContext::system(Via::System)
}

async fn new_user(pools: &TestPools) -> Uuid {
    let email = format!("{}@example.com", unique("authz").replace(' ', "."));
    let mut tx = pools.app.begin().await.unwrap();
    let user = huxley_auth::create_user(
        &mut tx,
        &ctx(),
        &email,
        "Authz User",
        "a fine long passphrase",
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    user.id
}

async fn role_id(pools: &TestPools, key: &str) -> Uuid {
    let mut conn = pools.app.acquire().await.unwrap();
    list_roles(&mut conn)
        .await
        .unwrap()
        .into_iter()
        .find(|r| r.key == key)
        .unwrap()
        .id
}

/// A fresh owner principal, as if bootstrap had run for them.
async fn owner(pools: &TestPools) -> Uuid {
    let user = new_user(pools).await;
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    huxley_authz::grant_root_owner(&mut tx, &ctx(), user)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    user
}

#[tokio::test]
async fn scoped_grants_limit_visibility() {
    let Some(pools) = pools().await else { return };
    let root = ensure_root(&pools).await;
    let boss = owner(&pools).await;
    // Create users first: each holds the audit chain lock until it commits,
    // so a second writer inside an open transaction would wait on it.
    let tech = new_user(&pools).await;
    let technician = role_id(&pools, "technician").await;

    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    let msp = huxley_orgs::create(&mut tx, &ctx(), root, OrgKind::Provider, &unique("MSP"))
        .await
        .unwrap();
    let client = huxley_orgs::create(&mut tx, &ctx(), msp.id, OrgKind::Client, &unique("Client"))
        .await
        .unwrap();
    let other = huxley_orgs::create(&mut tx, &ctx(), root, OrgKind::Provider, &unique("Other"))
        .await
        .unwrap();
    let boss_p = load_principal(&mut tx, boss).await.unwrap();
    let as_boss = ctx().with_actor(Actor::User(boss));
    assign(&mut tx, &as_boss, &boss_p, tech, technician, msp.id, true)
        .await
        .unwrap();

    let tech_p = load_principal(&mut tx, tech).await.unwrap();
    let visible = visible_orgs(&mut tx, &tech_p).await.unwrap();
    tx.commit().await.unwrap();

    assert!(visible.contains(&msp.id) && visible.contains(&client.id));
    assert!(!visible.contains(&other.id) && !visible.contains(&root));
    assert!(
        !tech_p.can_instance("user:read"),
        "a scoped grant is not instance-wide"
    );
}

#[tokio::test]
async fn only_owners_grant_owner_and_the_last_owner_stays() {
    let Some(pools) = pools().await else { return };
    let root = ensure_root(&pools).await;
    let admin_role = role_id(&pools, "admin").await;
    let owner_role = role_id(&pools, OWNER_ROLE).await;

    // An admin at the root can assign roles but not Owner.
    let boss = owner(&pools).await;
    let admin = new_user(&pools).await;
    let target = new_user(&pools).await;
    let second = new_user(&pools).await;
    let dup_user = new_user(&pools).await;
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    let boss_p = load_principal(&mut tx, boss).await.unwrap();
    assign(&mut tx, &ctx(), &boss_p, admin, admin_role, root, true)
        .await
        .unwrap();
    let admin_p = load_principal(&mut tx, admin).await.unwrap();
    let denied = assign(&mut tx, &ctx(), &admin_p, target, owner_role, root, true).await;
    assert!(
        matches!(denied, Err(AuthzError::OwnerOnly)),
        "got {denied:?}"
    );
    tx.rollback().await.unwrap();

    // Removing an owner works while another owner remains.
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    let a = assign(&mut tx, &ctx(), &boss_p, second, owner_role, root, true)
        .await
        .unwrap();
    unassign(&mut tx, &ctx(), &boss_p, a.id).await.unwrap();
    tx.rollback().await.unwrap();

    // Duplicate assignments are rejected.
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    assign(&mut tx, &ctx(), &boss_p, dup_user, admin_role, root, true)
        .await
        .unwrap();
    let dup = assign(&mut tx, &ctx(), &boss_p, dup_user, admin_role, root, true).await;
    assert!(
        matches!(dup, Err(AuthzError::AlreadyAssigned)),
        "got {dup:?}"
    );
}

#[tokio::test]
async fn users_without_permission_cannot_assign() {
    let Some(pools) = pools().await else { return };
    let root = ensure_root(&pools).await;
    let nobody = new_user(&pools).await;
    let target = new_user(&pools).await;
    let admin_role = role_id(&pools, "admin").await;

    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    let nobody_p = load_principal(&mut tx, nobody).await.unwrap();
    let result = assign(&mut tx, &ctx(), &nobody_p, target, admin_role, root, true).await;
    assert!(
        matches!(result, Err(AuthzError::Forbidden)),
        "got {result:?}"
    );
}
