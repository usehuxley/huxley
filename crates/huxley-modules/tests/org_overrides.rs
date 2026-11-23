//! Narrowing modules per provider org. One rolled-back transaction: module
//! settings are instance-wide and the test database is shared.

use huxley_core::{RequestContext, Via};
use huxley_db::testing::{ensure_root, pools, unique};
use huxley_db::{OrgScope, begin_scoped};
use huxley_modules::{ModuleError, SetError, enabled_set_at, set_enabled, set_for_org, states_at};
use huxley_orgs::OrgKind;

#[tokio::test]
async fn a_provider_org_can_turn_modules_off_for_its_subtree() {
    let Some(pools) = pools().await else { return };
    let root = ensure_root(&pools).await;
    let ctx = RequestContext::system(Via::System);
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    sqlx::query("DELETE FROM module_settings")
        .execute(&mut *tx)
        .await
        .unwrap();
    set_enabled(&mut tx, &ctx, "servicedesk", true, true)
        .await
        .unwrap();
    let acme = huxley_orgs::create(&mut tx, &ctx, root, OrgKind::Provider, &unique("Acme"))
        .await
        .unwrap();
    let blue = huxley_orgs::create(&mut tx, &ctx, root, OrgKind::Provider, &unique("Blue"))
        .await
        .unwrap();
    let client = huxley_orgs::create(&mut tx, &ctx, acme.id, OrgKind::Client, &unique("Client"))
        .await
        .unwrap();
    let division = huxley_orgs::create(
        &mut tx,
        &ctx,
        acme.id,
        OrgKind::Provider,
        &unique("Division"),
    )
    .await
    .unwrap();

    // crm can't go while servicedesk needs it; servicedesk can.
    assert!(matches!(
        set_for_org(&mut tx, &ctx, acme.id, "crm", false).await,
        Err(SetError::Module(ModuleError::HasDependents(d))) if d == vec!["servicedesk"]
    ));
    assert!(
        set_for_org(&mut tx, &ctx, acme.id, "servicedesk", false)
            .await
            .unwrap()
    );
    assert!(
        !set_for_org(&mut tx, &ctx, acme.id, "servicedesk", false)
            .await
            .unwrap(),
        "no-op"
    );

    // Off for Acme and everything under it; Blue keeps it.
    for org in [acme.id, client.id, division.id] {
        assert!(
            !enabled_set_at(&mut tx, org)
                .await
                .unwrap()
                .contains("servicedesk")
        );
    }
    assert!(
        enabled_set_at(&mut tx, blue.id)
            .await
            .unwrap()
            .contains("servicedesk")
    );
    let at_client = states_at(&mut tx, client.id).await.unwrap();
    let sd = at_client
        .iter()
        .find(|m| m.module.key == "servicedesk")
        .unwrap();
    assert!(sd.enabled && !sd.enabled_here);
    assert_eq!(sd.narrowed_by.as_ref().unwrap().org_id, acme.id);

    // Only non-root provider orgs narrow; a division can't undo its parent.
    assert!(matches!(
        set_for_org(&mut tx, &ctx, client.id, "crm", false).await,
        Err(SetError::NotAProviderOrg)
    ));
    assert!(matches!(
        set_for_org(&mut tx, &ctx, root, "crm", false).await,
        Err(SetError::NotAProviderOrg)
    ));
    set_for_org(&mut tx, &ctx, division.id, "crm", false)
        .await
        .unwrap();
    assert!(matches!(
        set_for_org(&mut tx, &ctx, division.id, "servicedesk", true).await,
        Err(SetError::NarrowedAbove(name)) if name == acme.name
    ));

    tx.rollback().await.unwrap();
}

#[tokio::test]
async fn restoring_needs_the_module_on_for_the_instance() {
    let Some(pools) = pools().await else { return };
    let root = ensure_root(&pools).await;
    let ctx = RequestContext::system(Via::System);
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    sqlx::query("DELETE FROM module_settings")
        .execute(&mut *tx)
        .await
        .unwrap();
    set_enabled(&mut tx, &ctx, "servicedesk", true, true)
        .await
        .unwrap();
    let acme = huxley_orgs::create(&mut tx, &ctx, root, OrgKind::Provider, &unique("Acme"))
        .await
        .unwrap();
    let client = huxley_orgs::create(&mut tx, &ctx, acme.id, OrgKind::Client, &unique("Client"))
        .await
        .unwrap();
    set_for_org(&mut tx, &ctx, acme.id, "servicedesk", false)
        .await
        .unwrap();
    // Restoring needs the module on instance-wide.
    set_for_org(&mut tx, &ctx, acme.id, "servicedesk", true)
        .await
        .unwrap();
    assert!(
        enabled_set_at(&mut tx, client.id)
            .await
            .unwrap()
            .contains("servicedesk")
    );
    set_for_org(&mut tx, &ctx, acme.id, "servicedesk", false)
        .await
        .unwrap();
    set_enabled(&mut tx, &ctx, "servicedesk", false, false)
        .await
        .unwrap();
    assert!(matches!(
        set_for_org(&mut tx, &ctx, acme.id, "servicedesk", true).await,
        Err(SetError::OffInstanceWide)
    ));
    tx.rollback().await.unwrap();
}
