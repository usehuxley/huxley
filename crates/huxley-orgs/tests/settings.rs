//! Setting inheritance and locks, against a real PostgreSQL. Rolls back.

use huxley_core::{RequestContext, Via};
use huxley_db::testing::{ensure_root, pools, unique};
use huxley_db::{OrgScope, begin_scoped};
use huxley_orgs::settings::{self, ResolvedSetting};
use huxley_orgs::{OrgError, OrgKind, Organization};
use sqlx::PgConnection;
use uuid::Uuid;

fn ctx() -> RequestContext {
    RequestContext::system(Via::System)
}

fn find<'a>(list: &'a [ResolvedSetting], key: &str) -> &'a ResolvedSetting {
    list.iter().find(|s| s.def.key == key).unwrap()
}

struct Tree {
    holding: Organization,
    msp: Organization,
    client: Organization,
    site: Organization,
}

/// holding → msp → client → site, under the shared root.
async fn tree(tx: &mut PgConnection, root: Uuid) -> Tree {
    let holding = huxley_orgs::create(tx, &ctx(), root, OrgKind::Provider, &unique("Holding"))
        .await
        .unwrap();
    let msp = huxley_orgs::create(tx, &ctx(), holding.id, OrgKind::Provider, &unique("Msp"))
        .await
        .unwrap();
    let client = huxley_orgs::create(tx, &ctx(), msp.id, OrgKind::Client, &unique("Client"))
        .await
        .unwrap();
    let site = huxley_orgs::create(tx, &ctx(), client.id, OrgKind::Client, &unique("Site"))
        .await
        .unwrap();
    Tree {
        holding,
        msp,
        client,
        site,
    }
}

#[tokio::test]
async fn nearest_value_wins_over_defaults() {
    let Some(pools) = pools().await else { return };
    let root = ensure_root(&pools).await;
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    let Tree {
        holding, msp, site, ..
    } = tree(&mut tx, root).await;
    // Defaults when nobody sets anything.
    let at_site = settings::resolve(&mut tx, site.id).await.unwrap();
    let prefix = find(&at_site, settings::TICKET_PREFIX);
    assert_eq!(prefix.value.as_deref(), Some("T"));
    assert!(prefix.source.is_none());
    assert_eq!(find(&at_site, settings::BRAND_SUPPORT_EMAIL).value, None);

    // Nearest wins.
    settings::set(
        &mut tx,
        &ctx(),
        holding.id,
        settings::TICKET_PREFIX,
        "NW",
        false,
    )
    .await
    .unwrap();
    settings::set(
        &mut tx,
        &ctx(),
        msp.id,
        settings::TICKET_PREFIX,
        "ACME",
        false,
    )
    .await
    .unwrap();
    let at_site = settings::resolve(&mut tx, site.id).await.unwrap();
    let prefix = find(&at_site, settings::TICKET_PREFIX);
    assert_eq!(prefix.value.as_deref(), Some("ACME"));
    assert_eq!(prefix.source.as_ref().unwrap().id, msp.id);
    assert!(prefix.local.is_none());
    assert_eq!(
        settings::value(&mut tx, holding.id, settings::TICKET_PREFIX)
            .await
            .unwrap()
            .as_deref(),
        Some("NW")
    );

    // Validation and unknown keys.
    assert!(matches!(
        settings::set(
            &mut tx,
            &ctx(),
            msp.id,
            settings::TICKET_PREFIX,
            "lower",
            false
        )
        .await,
        Err(OrgError::InvalidSetting(_))
    ));
    assert!(matches!(
        settings::set(&mut tx, &ctx(), msp.id, "branding.nope", "x", false).await,
        Err(OrgError::UnknownSetting)
    ));
    tx.rollback().await.unwrap();
}

#[tokio::test]
async fn locks_beat_nearer_values_and_block_writes_below() {
    let Some(pools) = pools().await else { return };
    let root = ensure_root(&pools).await;
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    let Tree {
        holding,
        msp,
        client,
        site,
    } = tree(&mut tx, root).await;
    // The client overrides its colour; then the holding company locks it.
    settings::set(
        &mut tx,
        &ctx(),
        client.id,
        settings::BRAND_ACCENT,
        "#00AA00",
        false,
    )
    .await
    .unwrap();
    assert_eq!(
        settings::value(&mut tx, site.id, settings::BRAND_ACCENT)
            .await
            .unwrap()
            .as_deref(),
        Some("#00aa00")
    );
    settings::set(
        &mut tx,
        &ctx(),
        holding.id,
        settings::BRAND_ACCENT,
        "#112233",
        true,
    )
    .await
    .unwrap();
    let at_client = settings::resolve(&mut tx, client.id).await.unwrap();
    let accent = find(&at_client, settings::BRAND_ACCENT);
    assert_eq!(
        accent.value.as_deref(),
        Some("#112233"),
        "the lock beats the nearer value"
    );
    assert_eq!(accent.locked_by.as_ref().unwrap().id, holding.id);
    assert_eq!(
        accent.local.as_ref().unwrap().value,
        "#00aa00",
        "kept, but ignored"
    );

    // Below a lock, writes are refused, at any depth.
    for org in [msp.id, site.id] {
        assert!(matches!(
            settings::set(&mut tx, &ctx(), org, settings::BRAND_ACCENT, "#ffffff", false).await,
            Err(OrgError::SettingLocked(name)) if name == holding.name
        ));
    }
    // The locking org itself isn't locked out.
    let at_holding = settings::resolve(&mut tx, holding.id).await.unwrap();
    assert!(
        find(&at_holding, settings::BRAND_ACCENT)
            .locked_by
            .is_none()
    );
    settings::set(
        &mut tx,
        &ctx(),
        holding.id,
        settings::BRAND_ACCENT,
        "#445566",
        true,
    )
    .await
    .unwrap();

    // Unlocking brings the client's own value back; clearing it inherits.
    settings::set(
        &mut tx,
        &ctx(),
        holding.id,
        settings::BRAND_ACCENT,
        "#445566",
        false,
    )
    .await
    .unwrap();
    assert_eq!(
        settings::value(&mut tx, site.id, settings::BRAND_ACCENT)
            .await
            .unwrap()
            .as_deref(),
        Some("#00aa00")
    );
    settings::clear(&mut tx, &ctx(), client.id, settings::BRAND_ACCENT)
        .await
        .unwrap();
    assert_eq!(
        settings::value(&mut tx, site.id, settings::BRAND_ACCENT)
            .await
            .unwrap()
            .as_deref(),
        Some("#445566")
    );

    tx.rollback().await.unwrap();
}
