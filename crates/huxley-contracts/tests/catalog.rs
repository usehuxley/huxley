//! The product catalog against a real PostgreSQL: SKUs, money, bundles,
//! versions, retiring and deleting.

use huxley_contracts::ContractsError;
use huxley_contracts::catalog::{self, Filter, Kind, ProductInput};
use huxley_core::{RequestContext, Via};
use huxley_db::testing::{ensure_root, isolated};
use huxley_db::{OrgScope, begin_scoped};
use rust_decimal::Decimal;
use serde_json::{Value, json};

fn input(v: Value) -> ProductInput {
    serde_json::from_value(v).unwrap()
}

#[tokio::test]
async fn products_bundles_and_their_rules() {
    let Some(db) = isolated("catalog").await else {
        return;
    };
    let pools = db.pools.clone();
    let root = ensure_root(&pools).await;
    let ctx = RequestContext::system(Via::System);
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    let tx = &mut *tx;
    let other = huxley_orgs::create(tx, &ctx, root, huxley_orgs::OrgKind::Provider, "Other")
        .await
        .unwrap()
        .id;

    let laptop = catalog::create(
        tx,
        &ctx,
        root,
        &input(json!({
            "sku": "LAT-5450", "name": "Dell Latitude 5450", "kind": "product",
            "vendor": "Dell", "category": "Hardware", "cost": "899.99", "price": "1149.00",
        })),
    )
    .await
    .unwrap();
    assert_eq!(laptop.price, Some(Decimal::new(114_900, 2)));
    assert_eq!(serde_json::to_value(&laptop).unwrap()["price"], "1149");
    let setup = catalog::create(
        tx,
        &ctx,
        root,
        &input(json!({
            "sku": "SVC-SETUP", "name": "Laptop setup", "kind": "service",
            "unit": "hour", "price": 125.5,
        })),
    )
    .await
    .unwrap();
    // SKUs are unique here, in any case.
    let again = catalog::create(
        tx,
        &ctx,
        root,
        &input(json!({ "sku": "lat-5450", "name": "Copy", "kind": "product" })),
    )
    .await;
    assert!(
        matches!(again, Err(ContractsError::Conflict(_))),
        "{again:?}"
    );

    // A bundle adds up its components, unless it has its own price.
    let kit = catalog::create(
        tx,
        &ctx,
        root,
        &input(json!({
            "sku": "KIT-NEWHIRE", "name": "New hire kit", "kind": "bundle",
            "components": [
                { "productId": laptop.id, "quantity": 1 },
                { "productId": setup.id, "quantity": "1.5" },
            ],
        })),
    )
    .await
    .unwrap();
    assert_eq!(kit.components.len(), 2);
    assert_eq!(kit.components_price, Some(Decimal::new(133_725, 2)));
    // Components are the bundle's organization's products, and not bundles.
    let foreign = catalog::create(
        tx,
        &ctx,
        other,
        &input(json!({ "sku": "LAT-5450", "name": "Theirs", "kind": "product" })),
    )
    .await
    .unwrap();
    for part in [foreign.id, kit.id] {
        let bad = catalog::create(
            tx,
            &ctx,
            root,
            &input(json!({
                "sku": format!("BAD-{}", part.simple()), "name": "Bad", "kind": "bundle",
                "components": [{ "productId": part, "quantity": 1 }],
            })),
        )
        .await;
        assert!(
            matches!(bad, Err(ContractsError::Invalid { .. })),
            "{bad:?}"
        );
    }

    retiring_and_listing(tx, &ctx, root, laptop, kit.id).await;
}

async fn retiring_and_listing(
    tx: &mut sqlx::PgConnection,
    ctx: &RequestContext,
    root: uuid::Uuid,
    laptop: catalog::Product,
    kit: uuid::Uuid,
) {
    // In a bundle: can't be deleted, can be retired.
    let refused = catalog::delete(tx, ctx, laptop.id).await;
    assert!(
        matches!(refused, Err(ContractsError::Conflict(_))),
        "{refused:?}"
    );
    let mut retire = input(json!({
        "sku": "LAT-5450", "name": "Dell Latitude 5450", "kind": "product", "active": false,
        "vendor": "Dell", "category": "Hardware",
        "cost": "899.99", "price": "1199", "version": laptop.version,
    }));
    let retired = catalog::update(tx, ctx, laptop.id, &retire).await.unwrap();
    assert!(!retired.active);
    assert_eq!(retired.version, laptop.version + 1);
    // An old version is refused.
    retire.name = "Stale".into();
    let stale = catalog::update(tx, ctx, laptop.id, &retire).await;
    assert!(matches!(stale, Err(ContractsError::VersionConflict)));

    // Lists filter by words, SKU, kind, category, active and organization.
    let names = |ps: Vec<catalog::Product>| ps.into_iter().map(|p| p.name).collect::<Vec<_>>();
    let mine = |f: Filter| Filter {
        org_id: Some(root),
        ..f
    };
    let all = catalog::list(tx, &mine(Filter::default())).await.unwrap();
    assert_eq!(
        names(all),
        ["Dell Latitude 5450", "Laptop setup", "New hire kit"]
    );
    let by_sku = catalog::list(
        tx,
        &mine(Filter {
            q: Some("kit-".into()),
            ..Filter::default()
        }),
    )
    .await
    .unwrap();
    assert_eq!(names(by_sku), ["New hire kit"]);
    let services = catalog::list(
        tx,
        &mine(Filter {
            kind: Some(Kind::Service),
            ..Filter::default()
        }),
    )
    .await
    .unwrap();
    assert_eq!(names(services), ["Laptop setup"]);
    let active = catalog::list(
        tx,
        &mine(Filter {
            active: Some(true),
            ..Filter::default()
        }),
    )
    .await
    .unwrap();
    assert_eq!(names(active), ["Laptop setup", "New hire kit"]);
    let hardware = catalog::list(
        tx,
        &mine(Filter {
            category: Some("hardware".into()),
            ..Filter::default()
        }),
    )
    .await
    .unwrap();
    assert_eq!(names(hardware), ["Dell Latitude 5450"]);

    // Deleting the bundle frees its parts.
    catalog::delete(tx, ctx, kit).await.unwrap();
    catalog::delete(tx, ctx, laptop.id).await.unwrap();
    assert!(matches!(
        catalog::get(tx, laptop.id).await,
        Err(ContractsError::NotFound)
    ));
}
