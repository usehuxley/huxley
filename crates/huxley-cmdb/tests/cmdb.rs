//! Assets against a real PostgreSQL: type fields, lifecycle, relationship
//! rules, impact analysis and organization scope.

use huxley_cmdb::CmdbError;
use huxley_cmdb::assets::{self, AssetInput, Filter};
use huxley_cmdb::relationships::{self, RelationshipInput};
use huxley_cmdb::types::{self, AssetTypeInput, TypeField};
use huxley_core::{RequestContext, Via};
use huxley_db::testing::{TestPools, ensure_root, pools, unique};
use huxley_db::{OrgScope, begin_scoped};
use serde_json::{Map, Value as Json, json};
use uuid::Uuid;

async fn client(pools: &TestPools) -> Uuid {
    let root = ensure_root(pools).await;
    let ctx = RequestContext::system(Via::System);
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    let org = huxley_orgs::create(
        &mut tx,
        &ctx,
        root,
        huxley_orgs::OrgKind::Client,
        &unique("CMDB client"),
    )
    .await
    .unwrap()
    .id;
    tx.commit().await.unwrap();
    org
}

async fn type_id(tx: &mut sqlx::PgConnection, key: &str) -> Uuid {
    types::list(tx)
        .await
        .unwrap()
        .into_iter()
        .find(|t| t.key == key)
        .unwrap()
        .id
}

fn attrs(v: &Json) -> Map<String, Json> {
    v.as_object().unwrap().clone()
}

fn input(org: Uuid, type_id: Uuid, name: &str) -> AssetInput {
    AssetInput {
        org_id: Some(org),
        type_id,
        name: name.into(),
        status: None,
        serial_number: None,
        hostname: None,
        manufacturer: None,
        model: None,
        site_id: None,
        contact_id: None,
        purchased_on: None,
        warranty_ends_on: None,
        notes: None,
        attributes: Map::new(),
        external_ids: None,
        version: None,
    }
}

#[tokio::test]
async fn assets_follow_their_type_and_lifecycle() {
    let Some(pools) = pools().await else { return };
    let org = client(&pools).await;
    let ctx = RequestContext::system(Via::System);
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    let server = type_id(&mut tx, "server").await;

    // The type's fields are checked: types, unknown keys.
    let mut dc = input(org, server, "DC01");
    dc.attributes = attrs(&json!({ "os": "Windows Server 2025", "memory_gb": "lots" }));
    assert!(
        matches!(assets::create(&mut tx, &ctx, &dc).await, Err(CmdbError::Invalid { field, .. }) if field == "attributes/memory_gb")
    );
    dc.attributes = attrs(&json!({ "os": "Windows Server 2025", "colour": "blue" }));
    assert!(
        matches!(assets::create(&mut tx, &ctx, &dc).await, Err(CmdbError::Invalid { field, .. }) if field == "attributes/colour")
    );
    dc.attributes =
        attrs(&json!({ "os": "Windows Server 2025", "memory_gb": 64, "virtual": true }));
    dc.serial_number = Some("SN-123".into());
    dc.external_ids = Some(attrs(&json!({ "rmm": "dev-42" })));
    let a = assets::create(&mut tx, &ctx, &dc).await.unwrap();
    assert_eq!(
        (a.status.as_str(), a.type_key.as_str()),
        ("active", "server")
    );
    assert_eq!(a.attributes.0["memory_gb"], json!(64));

    // A required field added to a type, and a custom type.
    let custom = types::create(
        &mut tx,
        &ctx,
        &AssetTypeInput {
            key: None,
            name: unique("Door Controller"),
            fields: vec![TypeField {
                key: "zone".into(),
                label: "Zone".into(),
                help: String::new(),
                field_type: huxley_entity::custom_fields::FieldType::Text,
                options: vec![],
                required: true,
            }],
            position: None,
            version: None,
        },
    )
    .await
    .unwrap();
    assert!(custom.key.starts_with("door_controller"));
    assert!(
        assets::create(&mut tx, &ctx, &input(org, custom.id, "Front door"))
            .await
            .is_err()
    );

    // Retired: a status change, and out of "in use".
    let mut change = dc.clone();
    change.status = Some("retired".into());
    change.version = Some(a.version);
    let a = assets::update(&mut tx, &ctx, a.id, &change).await.unwrap();
    assert_eq!(a.status, "retired");
    change.version = Some(a.version - 1);
    assert!(matches!(
        assets::update(&mut tx, &ctx, a.id, &change).await,
        Err(CmdbError::VersionConflict)
    ));
    let in_use = Filter {
        org_id: Some(org),
        in_use: true,
        limit: 50,
        ..Filter::default()
    };
    assert_eq!(assets::list(&mut tx, &in_use).await.unwrap().1, 0);
    let found = Filter {
        q: Some("sn-123".into()),
        org_id: Some(org),
        limit: 50,
        ..Filter::default()
    };
    assert_eq!(assets::list(&mut tx, &found).await.unwrap().0[0].id, a.id);
    let events: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM events_outbox WHERE subject_id = $1 AND event_type = 'asset.status_changed'",
    )
    .bind(a.id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(events, 1);
    tx.commit().await.unwrap();
}

#[tokio::test]
async fn impact_follows_what_runs_on_depends_on_and_is_hosted() {
    let Some(pools) = pools().await else { return };
    let org = client(&pools).await;
    let other = client(&pools).await;
    let ctx = RequestContext::system(Via::System);
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    let server = type_id(&mut tx, "server").await;
    let app = type_id(&mut tx, "application").await;
    let mk = async |tx: &mut sqlx::PgConnection, org, ty, name: &str| {
        assets::create(tx, &ctx, &input(org, ty, name))
            .await
            .unwrap()
            .id
    };
    let host = mk(&mut tx, org, server, "Host").await;
    let vm = mk(&mut tx, org, server, "VM").await;
    let sql = mk(&mut tx, org, server, "SQL").await;
    let erp = mk(&mut tx, org, app, "ERP").await;
    let nas = mk(&mut tx, org, server, "NAS").await;
    let elsewhere = mk(&mut tx, other, server, "Elsewhere").await;
    let rel = |kind: &str, to| RelationshipInput {
        kind: kind.into(),
        to_id: to,
    };
    // Host hosts VM; SQL runs on VM; ERP depends on SQL; NAS backs up SQL.
    relationships::add(&mut tx, &ctx, host, &rel("hosts", vm))
        .await
        .unwrap();
    relationships::add(&mut tx, &ctx, sql, &rel("runs_on", vm))
        .await
        .unwrap();
    relationships::add(&mut tx, &ctx, erp, &rel("depends_on", sql))
        .await
        .unwrap();
    relationships::add(&mut tx, &ctx, erp, &rel("depends_on", sql))
        .await
        .unwrap();
    relationships::add(&mut tx, &ctx, nas, &rel("backs_up", sql))
        .await
        .unwrap();
    // Not to itself, not across unrelated clients, only known kinds.
    assert!(
        relationships::add(&mut tx, &ctx, host, &rel("hosts", host))
            .await
            .is_err()
    );
    assert!(
        relationships::add(&mut tx, &ctx, host, &rel("hosts", elsewhere))
            .await
            .is_err()
    );
    assert!(
        relationships::add(&mut tx, &ctx, host, &rel("powers", vm))
            .await
            .is_err()
    );
    // A cycle doesn't loop.
    relationships::add(&mut tx, &ctx, vm, &rel("depends_on", erp))
        .await
        .unwrap();

    let hit = relationships::impact(&mut tx, host, 10).await.unwrap();
    let names: Vec<(&str, i32)> = hit.iter().map(|h| (h.name.as_str(), h.depth)).collect();
    assert_eq!(names, [("VM", 1), ("SQL", 2), ("ERP", 3)], "{hit:?}");
    assert_eq!(
        (hit[1].via_name.as_str(), hit[1].via_kind.as_str()),
        ("VM", "runs_on")
    );
    // The backup isn't affected; depth is limited.
    assert!(
        relationships::impact(&mut tx, sql, 10)
            .await
            .unwrap()
            .iter()
            .all(|h| h.id != nas)
    );
    assert_eq!(
        relationships::impact(&mut tx, host, 1).await.unwrap().len(),
        1
    );
    let seen = relationships::for_asset(&mut tx, sql).await.unwrap();
    assert_eq!(seen.len(), 3);
    assert!(
        seen.iter()
            .any(|r| r.kind == "backs_up" && r.direction == "in" && r.other_id == nas)
    );
    // Removed.
    let edge = seen.iter().find(|r| r.kind == "runs_on").unwrap().id;
    relationships::remove(&mut tx, &ctx, edge).await.unwrap();
    assert!(
        relationships::impact(&mut tx, host, 10)
            .await
            .unwrap()
            .iter()
            .all(|h| h.id != sql)
    );
    tx.commit().await.unwrap();

    // Another client's scope sees none of it.
    let mut tx = begin_scoped(&pools.app, &OrgScope::Orgs(vec![other]))
        .await
        .unwrap();
    assert!(matches!(
        assets::get(&mut tx, host).await,
        Err(CmdbError::NotFound)
    ));
    tx.commit().await.unwrap();
}
