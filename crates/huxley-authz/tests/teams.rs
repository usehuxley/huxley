//! Teams against a real PostgreSQL: members receive the team's grants.

use huxley_authz::{
    Assignee, AuthzError, assign_to, list_roles, load_principal, teams, visible_orgs,
};
use huxley_core::{RequestContext, Via};
use huxley_db::testing::{TestPools, ensure_root, pools, unique};
use huxley_db::{OrgScope, begin_scoped};
use huxley_orgs::OrgKind;
use uuid::Uuid;

fn ctx() -> RequestContext {
    RequestContext::system(Via::System)
}

async fn new_user(pools: &TestPools) -> Uuid {
    let email = format!("{}@example.com", unique("team").replace(' ', "."));
    let mut tx = pools.app.begin().await.unwrap();
    let user = huxley_auth::create_user(
        &mut tx,
        &ctx(),
        &email,
        "Team User",
        "a fine long passphrase",
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    user.id
}

async fn owner(pools: &TestPools) -> Uuid {
    let user = new_user(pools).await;
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    huxley_authz::grant_root_owner(&mut tx, &ctx(), user)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    user
}

async fn role(pools: &TestPools, key: &str) -> Uuid {
    let mut conn = pools.app.acquire().await.unwrap();
    list_roles(&mut conn)
        .await
        .unwrap()
        .into_iter()
        .find(|r| r.key == key)
        .unwrap()
        .id
}

#[tokio::test]
async fn members_get_the_teams_grants_until_they_leave() {
    let Some(pools) = pools().await else { return };
    let root = ensure_root(&pools).await;
    let boss = owner(&pools).await;
    let member = new_user(&pools).await;
    let technician = role(&pools, "technician").await;

    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    let client = huxley_orgs::create(
        &mut tx,
        &ctx(),
        root,
        OrgKind::Client,
        &unique("Team client"),
    )
    .await
    .unwrap();
    let team = teams::create(&mut tx, &ctx(), &unique("Helpdesk"), "Tier 1")
        .await
        .unwrap();
    assert_eq!(team.member_count, 0);
    let boss_p = load_principal(&mut tx, boss).await.unwrap();
    let assignment = assign_to(
        &mut tx,
        &ctx(),
        &boss_p,
        Assignee::Team(team.id),
        technician,
        client.id,
        true,
    )
    .await
    .unwrap();
    assert_eq!(assignment.team_id, Some(team.id));
    assert_eq!(assignment.user_id, None);
    assert_eq!(assignment.team_name.as_deref(), Some(team.name.as_str()));

    // Not a member yet: nothing.
    let p = load_principal(&mut tx, member).await.unwrap();
    assert!(!p.can_at_path(
        "org:read",
        &huxley_authz::org_path(&mut tx, client.id).await.unwrap()
    ));

    teams::add_member(&mut tx, &ctx(), team.id, member, None)
        .await
        .unwrap();
    teams::add_member(&mut tx, &ctx(), team.id, member, None)
        .await
        .unwrap(); // idempotent
    assert_eq!(teams::get(&mut tx, team.id).await.unwrap().member_count, 1);
    let p = load_principal(&mut tx, member).await.unwrap();
    let client_path = huxley_authz::org_path(&mut tx, client.id).await.unwrap();
    assert!(p.can_at_path("org:read", &client_path));
    assert!(!p.can_at_path("org:create", &client_path));
    assert!(
        visible_orgs(&mut tx, &p)
            .await
            .unwrap()
            .contains(&client.id)
    );
    assert_eq!(
        teams::of_user(&mut tx, member).await.unwrap()[0].id,
        team.id
    );

    // Leaving the team removes the grant.
    teams::remove_member(&mut tx, &ctx(), team.id, member)
        .await
        .unwrap();
    let p = load_principal(&mut tx, member).await.unwrap();
    assert!(!p.can_at_path("org:read", &client_path));

    tx.rollback().await.unwrap();
}

#[tokio::test]
async fn teams_cannot_hold_owner_or_the_same_grant_twice() {
    let Some(pools) = pools().await else { return };
    let root = ensure_root(&pools).await;
    let boss = owner(&pools).await;
    let technician = role(&pools, "technician").await;
    let owner_role = role(&pools, "owner").await;
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    let client = huxley_orgs::create(
        &mut tx,
        &ctx(),
        root,
        OrgKind::Client,
        &unique("Dup client"),
    )
    .await
    .unwrap();
    let team = teams::create(&mut tx, &ctx(), &unique("Dup"), "")
        .await
        .unwrap();
    let boss_p = load_principal(&mut tx, boss).await.unwrap();
    assign_to(
        &mut tx,
        &ctx(),
        &boss_p,
        Assignee::Team(team.id),
        technician,
        client.id,
        true,
    )
    .await
    .unwrap();
    // Owner to a team: refused before touching the database.
    assert!(matches!(
        assign_to(
            &mut tx,
            &ctx(),
            &boss_p,
            Assignee::Team(team.id),
            owner_role,
            root,
            true
        )
        .await,
        Err(AuthzError::OwnerToTeam)
    ));
    // Same grant twice: refused by the unique index. A failed statement
    // aborts the transaction, so contain it in a savepoint.
    sqlx::query("SAVEPOINT dup")
        .execute(&mut *tx)
        .await
        .unwrap();
    assert!(matches!(
        assign_to(
            &mut tx,
            &ctx(),
            &boss_p,
            Assignee::Team(team.id),
            technician,
            client.id,
            true
        )
        .await,
        Err(AuthzError::AlreadyAssigned)
    ));
    sqlx::query("ROLLBACK TO SAVEPOINT dup")
        .execute(&mut *tx)
        .await
        .unwrap();

    tx.rollback().await.unwrap();
}

#[tokio::test]
async fn deleting_a_team_removes_its_assignments() {
    let Some(pools) = pools().await else { return };
    let root = ensure_root(&pools).await;
    let boss = owner(&pools).await;
    let member = new_user(&pools).await;
    let technician = role(&pools, "technician").await;
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    let team = teams::create(&mut tx, &ctx(), &unique("Temp"), "")
        .await
        .unwrap();
    let boss_p = load_principal(&mut tx, boss).await.unwrap();
    let assignment = assign_to(
        &mut tx,
        &ctx(),
        &boss_p,
        Assignee::Team(team.id),
        technician,
        root,
        true,
    )
    .await
    .unwrap();
    // Deleting the team removes its assignments.
    teams::add_member(&mut tx, &ctx(), team.id, member, None)
        .await
        .unwrap();
    teams::delete(&mut tx, &ctx(), team.id).await.unwrap();
    let p = load_principal(&mut tx, member).await.unwrap();
    assert!(p.grants.is_empty());
    let (left,): (i64,) = sqlx::query_as("SELECT count(*) FROM role_assignments WHERE id = $1")
        .bind(assignment.id)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert_eq!(left, 0);
    tx.rollback().await.unwrap();
}

#[tokio::test]
async fn the_database_refuses_owner_for_teams_too() {
    let Some(pools) = pools().await else { return };
    let root = ensure_root(&pools).await;
    let owner_role = role(&pools, "owner").await;
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    let team = teams::create(&mut tx, &ctx(), &unique("Owners"), "")
        .await
        .unwrap();
    let err = sqlx::query(
        "INSERT INTO role_assignments (id, team_id, role_id, org_id) VALUES ($1, $2, $3, $4)",
    )
    .bind(Uuid::now_v7())
    .bind(team.id)
    .bind(owner_role)
    .bind(root)
    .execute(&mut *tx)
    .await
    .unwrap_err();
    assert_eq!(
        huxley_db::violated_constraint(&err),
        Some("role_assignments_owner_not_team")
    );
}

#[tokio::test]
async fn team_names_are_unique_and_validated() {
    let Some(pools) = pools().await else { return };
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    let name = unique("Dispatch");
    teams::create(&mut tx, &ctx(), &name, "").await.unwrap();
    assert!(matches!(
        teams::create(&mut tx, &ctx(), &name.to_uppercase(), "").await,
        Err(AuthzError::TeamNameTaken)
    ));
    assert!(matches!(
        teams::create(&mut tx, &ctx(), " ", "").await,
        Err(AuthzError::InvalidTeamName)
    ));
    tx.rollback().await.unwrap();
}
