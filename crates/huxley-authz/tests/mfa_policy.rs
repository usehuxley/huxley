//! Who the MFA policy covers, against a real PostgreSQL.
//!
//! Everything runs in one transaction that rolls back: the root org's flag
//! covers every user, so committing it would break concurrent tests.

use huxley_authz::mfa_policy::{self, user_requires_mfa};
use huxley_core::{RequestContext, Via};
use huxley_db::testing::{ensure_root, pools, unique};
use huxley_db::{OrgScope, begin_scoped};
use huxley_orgs::OrgKind;
use sqlx::PgConnection;
use uuid::Uuid;

fn ctx() -> RequestContext {
    RequestContext::system(Via::System)
}

async fn user(conn: &mut PgConnection) -> Uuid {
    let email = format!("{}@example.com", unique("policy").replace(' ', "."));
    huxley_auth::create_user(
        conn,
        &ctx(),
        &email,
        "Policy User",
        "a fine long passphrase",
    )
    .await
    .unwrap()
    .id
}

/// A fresh role so flagging it can't affect other tests.
async fn role(conn: &mut PgConnection) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO roles (id, key, name, description, is_system, permissions) \
         VALUES ($1, $2, $3, 'test', false, '{org:read}')",
    )
    .bind(id)
    .bind(format!("policy_{}", id.simple()))
    .bind(unique("Policy role"))
    .execute(conn)
    .await
    .unwrap();
    id
}

async fn assign(conn: &mut PgConnection, user: Uuid, role: Uuid, org: Uuid, inherit: bool) {
    sqlx::query(
        "INSERT INTO role_assignments (id, user_id, role_id, org_id, inherit) VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(Uuid::now_v7())
    .bind(user)
    .bind(role)
    .bind(org)
    .bind(inherit)
    .execute(conn)
    .await
    .unwrap();
}

#[tokio::test]
async fn policy_covers_roles_orgs_descendants_and_inherited_access() {
    let Some(pools) = pools().await else { return };
    let root = ensure_root(&pools).await;
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    let c = &ctx();

    // root ─ msp ─ client ─ site
    let msp = huxley_orgs::create(&mut tx, c, root, OrgKind::Provider, &unique("MSP"))
        .await
        .unwrap()
        .id;
    let client = huxley_orgs::create(&mut tx, c, msp, OrgKind::Client, &unique("Client"))
        .await
        .unwrap()
        .id;
    let site = huxley_orgs::create(&mut tx, c, client, OrgKind::Client, &unique("Site"))
        .await
        .unwrap()
        .id;
    let plain = role(&mut tx).await;

    let at_msp_inherit = user(&mut tx).await;
    assign(&mut tx, at_msp_inherit, plain, msp, true).await;
    let at_msp_only = user(&mut tx).await;
    assign(&mut tx, at_msp_only, plain, msp, false).await;
    let at_site = user(&mut tx).await;
    assign(&mut tx, at_site, plain, site, false).await;
    let no_roles = user(&mut tx).await;

    for u in [at_msp_inherit, at_msp_only, at_site, no_roles] {
        assert!(!user_requires_mfa(&mut tx, u).await.unwrap());
    }

    // Flag the client: users assigned below it, and above it with
    // inheritance, can reach it. Someone at the MSP without inheritance can't.
    mfa_policy::set_org(&mut tx, c, client, true).await.unwrap();
    assert!(user_requires_mfa(&mut tx, at_site).await.unwrap());
    assert!(user_requires_mfa(&mut tx, at_msp_inherit).await.unwrap());
    assert!(!user_requires_mfa(&mut tx, at_msp_only).await.unwrap());
    assert!(!user_requires_mfa(&mut tx, no_roles).await.unwrap());
    let policy = mfa_policy::get(&mut tx).await.unwrap();
    assert!(policy.orgs.iter().any(|o| o.id == client && !o.is_root));
    mfa_policy::set_org(&mut tx, c, client, false)
        .await
        .unwrap();
    assert!(!user_requires_mfa(&mut tx, at_site).await.unwrap());

    // Flag the role: everyone holding it, wherever.
    mfa_policy::set_role(&mut tx, c, plain, true).await.unwrap();
    assert!(user_requires_mfa(&mut tx, at_msp_only).await.unwrap());
    assert!(user_requires_mfa(&mut tx, at_site).await.unwrap());
    assert!(!user_requires_mfa(&mut tx, no_roles).await.unwrap());
    assert!(
        mfa_policy::get(&mut tx)
            .await
            .unwrap()
            .role_ids
            .contains(&plain)
    );
    mfa_policy::set_role(&mut tx, c, plain, false)
        .await
        .unwrap();

    // Flag the root: every user, even one with no roles.
    mfa_policy::set_org(&mut tx, c, root, true).await.unwrap();
    assert!(user_requires_mfa(&mut tx, no_roles).await.unwrap());
    assert!(user_requires_mfa(&mut tx, at_msp_only).await.unwrap());
    assert!(mfa_policy::get(&mut tx).await.unwrap().everyone);

    // Changes are audited.
    let (changes,): (i64,) = sqlx::query_as(
        "SELECT count(*) FROM audit_events WHERE action = 'security.mfa_policy_changed' \
           AND target_id = ANY($1)",
    )
    .bind(vec![client, plain, root])
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(changes, 5);

    tx.rollback().await.unwrap();
}

#[tokio::test]
async fn unknown_targets_are_reported() {
    let Some(pools) = pools().await else { return };
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    assert!(matches!(
        mfa_policy::set_org(&mut tx, &ctx(), Uuid::now_v7(), true).await,
        Err(huxley_authz::AuthzError::OrgNotFound)
    ));
    assert!(matches!(
        mfa_policy::set_role(&mut tx, &ctx(), Uuid::now_v7(), true).await,
        Err(huxley_authz::AuthzError::RoleNotFound)
    ));
}
