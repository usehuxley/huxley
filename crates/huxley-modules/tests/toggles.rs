//! Module toggles against a real PostgreSQL, in a database of their own:
//! module settings are instance-wide.

use huxley_core::{RequestContext, Via};
use huxley_db::testing::{ensure_root, isolated};
use huxley_db::{OrgScope, begin_scoped};
use huxley_modules::{
    ModuleError, Preset, SetError, apply_preset, enabled_set, is_active, set_enabled, states,
};

#[tokio::test]
async fn toggles_respect_dependencies_and_are_audited() {
    // Its own database: other tests turn modules on concurrently.
    let Some(db) = isolated("toggles").await else {
        return;
    };
    let pools = &db.pools;
    ensure_root(pools).await;
    let ctx = RequestContext::system(Via::System);
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    sqlx::query("DELETE FROM module_settings")
        .execute(&mut *tx)
        .await
        .unwrap();

    // Enabling without dependencies is refused with the missing list.
    let refused = set_enabled(&mut tx, &ctx, "billing", true, false).await;
    let Err(SetError::Module(ModuleError::MissingDependencies(mut deps))) = refused else {
        panic!("{refused:?}")
    };
    deps.sort_unstable();
    assert_eq!(deps, ["contracts", "crm"]);

    // Including dependencies turns them on first.
    let changed = set_enabled(&mut tx, &ctx, "billing", true, true)
        .await
        .unwrap();
    assert_eq!(changed, vec!["crm", "contracts", "billing"]);
    assert!(enabled_set(&mut tx).await.unwrap().contains("contracts"));

    assert!(is_active(&mut tx, "billing").await.unwrap());

    // Enabled shows as active (every module is released in this version).
    let change = states(&mut tx)
        .await
        .unwrap()
        .into_iter()
        .find(|m| m.module.key == "billing")
        .unwrap();
    assert!(change.enabled && change.active);

    // A module others depend on can't be turned off.
    let blocked = set_enabled(&mut tx, &ctx, "contracts", false, false).await;
    assert!(matches!(
        blocked,
        Err(SetError::Module(ModuleError::HasDependents(_)))
    ));
    assert_eq!(
        set_enabled(&mut tx, &ctx, "billing", false, false)
            .await
            .unwrap(),
        vec!["billing"]
    );

    // Each change is audited.
    let (audited,): (i64,) = sqlx::query_as(
        "SELECT count(*) FROM audit_events WHERE action IN ('module.enabled', 'module.disabled') \
         AND occurred_at >= now() - interval '1 minute'",
    )
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert!(audited >= 4);

    // Presets apply cleanly.
    sqlx::query("DELETE FROM module_settings")
        .execute(&mut *tx)
        .await
        .unwrap();
    apply_preset(&mut tx, &ctx, Preset::ServiceDesk)
        .await
        .unwrap();
    assert_eq!(
        enabled_set(&mut tx).await.unwrap().len(),
        Preset::ServiceDesk.modules().len()
    );

    tx.rollback().await.unwrap();
}
