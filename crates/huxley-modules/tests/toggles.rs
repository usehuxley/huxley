//! Module toggles against a real PostgreSQL. Everything runs in one
//! transaction that is rolled back: module settings are instance-wide and
//! the test database is shared.

use huxley_core::{RequestContext, Via};
use huxley_db::testing::{ensure_root, pools};
use huxley_db::{OrgScope, begin_scoped};
use huxley_modules::{
    ModuleError, Preset, SetError, apply_preset, enabled_set, is_active, set_enabled, states,
};

#[tokio::test]
async fn toggles_respect_dependencies_and_are_audited() {
    let Some(pools) = pools().await else { return };
    ensure_root(&pools).await;
    let ctx = RequestContext::system(Via::System);
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    sqlx::query("DELETE FROM module_settings")
        .execute(&mut *tx)
        .await
        .unwrap();

    // Enabling without dependencies is refused with the missing list.
    let refused = set_enabled(&mut tx, &ctx, "cx", true, false).await;
    assert!(matches!(
        refused,
        Err(SetError::Module(ModuleError::MissingDependencies(ref deps))) if deps == &vec!["crm", "servicedesk"]
    ));

    // Including dependencies turns them on first.
    let changed = set_enabled(&mut tx, &ctx, "cx", true, true).await.unwrap();
    assert_eq!(changed, vec!["crm", "servicedesk", "cx"]);
    assert!(enabled_set(&mut tx).await.unwrap().contains("servicedesk"));

    // Unreleased modules are enabled but not active.
    assert!(!is_active(&mut tx, "cx").await.unwrap());
    let cx = states(&mut tx)
        .await
        .unwrap()
        .into_iter()
        .find(|m| m.module.key == "cx")
        .unwrap();
    assert!(cx.enabled && !cx.active);

    // A module others depend on can't be turned off.
    let blocked = set_enabled(&mut tx, &ctx, "crm", false, false).await;
    assert!(matches!(
        blocked,
        Err(SetError::Module(ModuleError::HasDependents(_)))
    ));
    assert_eq!(
        set_enabled(&mut tx, &ctx, "cx", false, false)
            .await
            .unwrap(),
        vec!["cx"]
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
