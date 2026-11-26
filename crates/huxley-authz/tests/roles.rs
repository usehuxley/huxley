//! Custom roles and the effective-permissions explainer, against a real
//! PostgreSQL. Each test rolls back.

use huxley_authz::roles::{self, RoleInput};
use huxley_authz::{
    Assignee, AuthzError, Grant, Principal, assign_to, explain, list_roles, load_principal, teams,
};
use huxley_core::{RequestContext, Via};
use huxley_db::testing::{ensure_root, pools, unique};
use huxley_db::{OrgScope, begin_scoped};
use huxley_orgs::OrgKind;
use uuid::Uuid;

fn ctx() -> RequestContext {
    RequestContext::system(Via::System)
}

fn input(name: &str, perms: &[&str]) -> RoleInput {
    RoleInput {
        key: None,
        name: name.into(),
        description: String::new(),
        permissions: perms.iter().map(|p| (*p).to_owned()).collect(),
    }
}

/// A principal holding exactly `perms` at the root (`root_path`).
fn holder(root_path: &str, perms: &[&str]) -> Principal {
    Principal {
        user_id: Uuid::nil(),
        grants: vec![Grant {
            role_key: "custom".into(),
            org_id: Uuid::nil(),
            org_path: root_path.into(),
            inherit: true,
            permissions: perms.iter().map(|p| (*p).to_owned()).collect(),
        }],
        root_path: Some(root_path.into()),
        requires_mfa: false,
    }
}

#[tokio::test]
async fn custom_roles_never_grant_more_than_their_author_holds() {
    let Some(pools) = pools().await else { return };
    let root = ensure_root(&pools).await;
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    let root_path = huxley_authz::org_path(&mut tx, root).await.unwrap();
    let manager = holder(&root_path, &["role:manage", "org:read", "user:read"]);

    let role = roles::create(
        &mut tx,
        &ctx(),
        &manager,
        &input(
            &unique("Dispatcher"),
            &["org:read", "user:read", "org:read"],
        ),
    )
    .await
    .unwrap();
    assert!(!role.is_system);
    assert_eq!(
        role.permissions,
        vec!["org:read", "user:read"],
        "sorted and de-duplicated"
    );

    // Permissions the author lacks, or everything: refused.
    assert!(matches!(
        roles::create(
            &mut tx,
            &ctx(),
            &manager,
            &input("Grabby", &["user:create"])
        )
        .await,
        Err(AuthzError::Escalation(_))
    ));
    assert!(matches!(
        roles::create(&mut tx, &ctx(), &manager, &input("Everything", &["*"])).await,
        Err(AuthzError::Escalation(_))
    ));
    assert!(matches!(
        roles::update(
            &mut tx,
            &ctx(),
            &manager,
            role.id,
            &input("Dispatcher", &["audit:read"])
        )
        .await,
        Err(AuthzError::Escalation(_))
    ));
    assert!(matches!(
        roles::create(&mut tx, &ctx(), &manager, &input("Typo", &["org:raed"])).await,
        Err(AuthzError::InvalidRole(_))
    ));

    tx.rollback().await.unwrap();
}

#[tokio::test]
async fn system_roles_are_fixed_and_assigned_roles_stay() {
    let Some(pools) = pools().await else { return };
    let root = ensure_root(&pools).await;
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    let root_path = huxley_authz::org_path(&mut tx, root).await.unwrap();
    let manager = holder(&root_path, &["role:manage", "org:read", "user:read"]);
    let role = roles::create(
        &mut tx,
        &ctx(),
        &manager,
        &input(&unique("Desk"), &["org:read"]),
    )
    .await
    .unwrap();
    // System roles stay as shipped.
    let mut conn_roles = list_roles(&mut tx).await.unwrap();
    let technician = conn_roles
        .iter_mut()
        .find(|r| r.key == "technician")
        .unwrap()
        .id;
    assert!(matches!(
        roles::update(
            &mut tx,
            &ctx(),
            &manager,
            technician,
            &input("Tech", &["org:read"])
        )
        .await,
        Err(AuthzError::SystemRole)
    ));
    assert!(matches!(
        roles::delete(&mut tx, &ctx(), technician).await,
        Err(AuthzError::SystemRole)
    ));

    // An assigned role can't be deleted; an unassigned one can.
    let owner = holder(&root_path, &["*"]);
    let user = huxley_auth::create_user(
        &mut tx,
        &ctx(),
        &format!("{}@example.com", unique("r").replace(' ', ".")),
        "R",
        "a fine long passphrase",
    )
    .await
    .unwrap();
    let owner_principal = Principal {
        user_id: Uuid::nil(),
        ..owner
    };
    assign_to(
        &mut tx,
        &ctx(),
        &owner_principal,
        Assignee::User(user.id),
        role.id,
        root,
        true,
    )
    .await
    .unwrap();
    assert!(matches!(
        roles::delete(&mut tx, &ctx(), role.id).await,
        Err(AuthzError::RoleInUse(1))
    ));
    let spare = roles::create(
        &mut tx,
        &ctx(),
        &manager,
        &input(&unique("Spare"), &["org:read"]),
    )
    .await
    .unwrap();
    roles::delete(&mut tx, &ctx(), spare.id).await.unwrap();
    tx.rollback().await.unwrap();
}

#[tokio::test]
async fn the_explainer_shows_where_each_permission_comes_from() {
    let Some(pools) = pools().await else { return };
    let root = ensure_root(&pools).await;
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    let root_path = huxley_authz::org_path(&mut tx, root).await.unwrap();
    let owner = holder(&root_path, &["*"]);
    let client = huxley_orgs::create(&mut tx, &ctx(), root, OrgKind::Client, &unique("Explained"))
        .await
        .unwrap()
        .id;
    let other = huxley_orgs::create(&mut tx, &ctx(), root, OrgKind::Client, &unique("Elsewhere"))
        .await
        .unwrap()
        .id;
    let user = huxley_auth::create_user(
        &mut tx,
        &ctx(),
        &format!("{}@example.com", unique("x").replace(' ', ".")),
        "X",
        "a fine long passphrase",
    )
    .await
    .unwrap();
    let all = list_roles(&mut tx).await.unwrap();
    let id_of = |k: &str| all.iter().find(|r| r.key == k).unwrap().id;

    // Technician at the client directly; Read-only at the root through a team.
    assign_to(
        &mut tx,
        &ctx(),
        &owner,
        Assignee::User(user.id),
        id_of("technician"),
        client,
        true,
    )
    .await
    .unwrap();
    let team = teams::create(&mut tx, &ctx(), &unique("Auditors"), "")
        .await
        .unwrap();
    teams::add_member(&mut tx, &ctx(), team.id, user.id, None)
        .await
        .unwrap();
    assign_to(
        &mut tx,
        &ctx(),
        &owner,
        Assignee::Team(team.id),
        id_of("read_only"),
        root,
        true,
    )
    .await
    .unwrap();

    let at_client = explain::explain(&mut tx, user.id, client).await.unwrap();
    let find = |list: &[explain::PermissionExplanation], k: &str| {
        list.iter().find(|p| p.key == k).cloned().unwrap()
    };
    let org_read = find(&at_client, "org:read");
    assert!(org_read.allowed);
    assert_eq!(
        org_read.via.len(),
        2,
        "technician directly and read-only via the team"
    );
    assert!(
        org_read
            .via
            .iter()
            .any(|v| v.team_name.as_deref() == Some(team.name.as_str()))
    );
    assert!(
        find(&at_client, "audit:read").allowed,
        "instance permission from the root grant"
    );
    assert!(!find(&at_client, "user:create").allowed);
    assert!(find(&at_client, "user:create").via.is_empty());

    // Elsewhere only the team's root grant reaches.
    let at_other = explain::explain(&mut tx, user.id, other).await.unwrap();
    let org_read = find(&at_other, "org:read");
    assert_eq!(org_read.via.len(), 1);
    assert_eq!(org_read.via[0].role_name, "Read-only");

    // It agrees with the real evaluator.
    let principal = load_principal(&mut tx, user.id).await.unwrap();
    let client_path = huxley_authz::org_path(&mut tx, client).await.unwrap();
    for p in &at_client {
        if p.scope == huxley_authz::PermissionScope::Org {
            assert_eq!(
                p.allowed,
                principal.can_at_path(p.key, &client_path),
                "{}",
                p.key
            );
        }
    }
    tx.rollback().await.unwrap();
}
