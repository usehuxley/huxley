//! Dataset → asset mappings against a real PostgreSQL: matching, field
//! provenance and conflicts, new assets, previews and problems.

use huxley_cmdb::assets::{self, AssetInput};
use huxley_cmdb::mappings::{self, FieldMap, MappingInput, MatchKey};
use huxley_cmdb::types;
use huxley_core::{RequestContext, Via};
use huxley_db::testing::{TestPools, ensure_root, isolated};
use huxley_db::{OrgScope, begin_scoped};
use serde_json::{Map, json};
use uuid::Uuid;

async fn client(pools: &TestPools, root: Uuid, name: &str) -> Uuid {
    let ctx = RequestContext::system(Via::System);
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    let org = huxley_orgs::create(&mut tx, &ctx, root, huxley_orgs::OrgKind::Client, name)
        .await
        .unwrap()
        .id;
    tx.commit().await.unwrap();
    org
}

/// An RMM-like dataset at the root, with rows for `client`.
async fn devices(pools: &TestPools, root: Uuid, client: Uuid) -> huxley_datasets::Dataset {
    let ctx = RequestContext::system(Via::System);
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    let input: huxley_datasets::DatasetInput = serde_json::from_value(json!({
        "key": "rmm-devices", "name": "RMM devices",
        "columns": [
            { "name": "device_id", "type": "text" }, { "name": "hostname", "type": "text" },
            { "name": "serial", "type": "text" }, { "name": "os", "type": "text" },
            { "name": "ram", "type": "number" }, { "name": "vendor", "type": "text" },
        ],
        "primaryKey": ["device_id"],
    }))
    .unwrap();
    let ds = huxley_datasets::create(&mut tx, &ctx, root, &input)
        .await
        .unwrap();
    huxley_datasets::upsert(
        &mut tx,
        &ds,
        client,
        &[
            json!({ "device_id": "d1", "hostname": "LAPTOP-ADA", "serial": "SN-1", "os": "Windows 11", "ram": 16, "vendor": "Dell" }),
            json!({ "device_id": "d2", "hostname": "DESK-BOB", "serial": "SN-2", "os": "Windows 11", "ram": 32 }),
            json!({ "device_id": "d3", "hostname": "", "serial": "", "os": "Linux" }),
        ],
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    ds
}

/// Bob's desk, entered by hand with its serial and a name.
fn manual_desk(acme: Uuid, ws: Uuid) -> AssetInput {
    AssetInput {
        org_id: Some(acme),
        type_id: ws,
        name: "Bob's desk".into(),
        status: None,
        serial_number: Some("sn-2".into()),
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

fn rule(dataset: Uuid, type_id: Uuid, conflict: &str) -> MappingInput {
    let mut attributes = std::collections::BTreeMap::new();
    attributes.insert("os".to_owned(), "os".to_owned());
    attributes.insert("memory_gb".to_owned(), "ram".to_owned());
    MappingInput {
        dataset_id: dataset,
        name: "RMM devices".into(),
        source: "rmm".into(),
        type_id,
        match_keys: vec![
            MatchKey {
                field: "external_id".into(),
                column: "device_id".into(),
            },
            MatchKey {
                field: "serial_number".into(),
                column: "serial".into(),
            },
        ],
        fields: FieldMap {
            name: Some("hostname".into()),
            hostname: Some("hostname".into()),
            serial_number: Some("serial".into()),
            manufacturer: Some("vendor".into()),
            external_id: Some("device_id".into()),
            attributes,
            ..FieldMap::default()
        },
        conflict: Some(conflict.into()),
        create_missing: true,
        enabled: true,
        version: None,
    }
}

#[tokio::test]
async fn rows_become_assets_matched_and_kept_in_step() {
    let Some(db) = isolated("asset_mappings").await else {
        return;
    };
    let pools = db.pools.clone();
    let root = ensure_root(&pools).await;
    let acme = client(&pools, root, "Acme").await;
    let ds = devices(&pools, root, acme).await;
    let ctx = RequestContext::system(Via::System);
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    let ws = types::list(&mut tx)
        .await
        .unwrap()
        .into_iter()
        .find(|t| t.key == "workstation")
        .unwrap()
        .id;
    // Bob's desk exists already, entered by hand with its serial and a name.
    let desk = manual_desk(acme, ws);
    let desk_id = assets::create(&mut tx, &ctx, &desk).await.unwrap().id;
    // A bad rule is refused: unknown column.
    let mut bad = rule(ds.id, ws, "manual_wins");
    bad.match_keys[0].column = "nope".into();
    assert!(mappings::create(&mut tx, &ctx, &bad).await.is_err());
    let m = mappings::create(&mut tx, &ctx, &rule(ds.id, ws, "manual_wins"))
        .await
        .unwrap();
    tx.commit().await.unwrap();

    // A preview changes nothing.
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    let preview = mappings::run(&mut tx, &ctx, m.id, true).await.unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(
        (
            preview.rows,
            preview.created,
            preview.updated,
            preview.skipped,
            preview.preview
        ),
        (3, 1, 1, 1, true)
    );
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM assets WHERE org_id = $1")
        .bind(acme)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert_eq!(count, 1);

    // The run: Ada's laptop made, Bob's desk matched by serial, the empty row skipped.
    let r = mappings::run(&mut tx, &ctx, m.id, false).await.unwrap();
    assert_eq!((r.created, r.updated, r.skipped), (1, 1, 1), "{r:?}");
    assert!(r.problems[0].reason.contains("no name"), "{r:?}");
    let desk_now = assets::get(&mut tx, desk_id).await.unwrap();
    // Manual wins: the name entered by hand stays; the rest comes from the RMM.
    assert_eq!(desk_now.name, "Bob's desk");
    assert_eq!(
        (
            desk_now.hostname.as_str(),
            desk_now.external_ids.0["rmm"].as_str()
        ),
        ("DESK-BOB", Some("d2"))
    );
    assert_eq!(desk_now.attributes.0["memory_gb"], json!(32));
    assert_eq!(desk_now.field_sources.0["name"], "manual");
    assert_eq!(desk_now.field_sources.0["hostname"], "rmm");
    let (laptop,): (Uuid,) =
        sqlx::query_as("SELECT id FROM assets WHERE external_ids ->> 'rmm' = 'd1'")
            .fetch_one(&mut *tx)
            .await
            .unwrap();
    let laptop = assets::get(&mut tx, laptop).await.unwrap();
    assert_eq!(
        (
            laptop.name.as_str(),
            laptop.manufacturer.as_str(),
            laptop.type_key.as_str()
        ),
        ("LAPTOP-ADA", "Dell", "workstation")
    );
    assert!(laptop.synced_at.is_some());
    // A second run changes nothing.
    let again = mappings::run(&mut tx, &ctx, m.id, false).await.unwrap();
    assert_eq!(
        (again.created, again.updated, again.unchanged),
        (0, 0, 2),
        "{again:?}"
    );
    assert_eq!(
        mappings::get(&mut tx, m.id)
            .await
            .unwrap()
            .last_result
            .unwrap()
            .0
            .unchanged,
        2
    );

    conflict_policies(&mut tx, &ctx, m.id, ds.id, ws, desk, laptop.id).await;
    tx.commit().await.unwrap();
}

/// Someone renames an asset by hand: `manual_wins` keeps it, `source_wins`
/// replaces it.
async fn conflict_policies(
    tx: &mut sqlx::PgConnection,
    ctx: &RequestContext,
    m: Uuid,
    ds: Uuid,
    ws: Uuid,
    template: AssetInput,
    laptop: Uuid,
) {
    let tx = &mut *tx;
    let laptop = assets::get(tx, laptop).await.unwrap();
    let mut desk = template;
    // Someone renames the laptop by hand: manual wins keeps it, source wins replaces it.
    desk = AssetInput {
        name: "Ada's laptop".into(),
        serial_number: Some("SN-1".into()),
        version: Some(laptop.version),
        ..desk
    };
    desk.hostname = Some("LAPTOP-ADA".into());
    desk.manufacturer = Some("Dell".into());
    desk.attributes = laptop.attributes.0.clone();
    assets::update(tx, ctx, laptop.id, &desk).await.unwrap();
    mappings::run(tx, ctx, m, false).await.unwrap();
    assert_eq!(
        assets::get(tx, laptop.id).await.unwrap().name,
        "Ada's laptop"
    );
    let mut source_wins = rule(ds, ws, "source_wins");
    source_wins.version = Some(mappings::get(tx, m).await.unwrap().version);
    mappings::update(tx, ctx, m, &source_wins).await.unwrap();
    mappings::run(tx, ctx, m, false).await.unwrap();
    let laptop = assets::get(tx, laptop.id).await.unwrap();
    assert_eq!(
        (
            laptop.name.as_str(),
            laptop.field_sources.0["name"].as_str()
        ),
        ("LAPTOP-ADA", Some("rmm"))
    );
}

#[tokio::test]
async fn ambiguous_matches_are_skipped() {
    let Some(db) = isolated("asset_mappings_ambiguous").await else {
        return;
    };
    let pools = db.pools.clone();
    let root = ensure_root(&pools).await;
    let acme = client(&pools, root, "Acme").await;
    let ds = devices(&pools, root, acme).await;
    let ctx = RequestContext::system(Via::System);
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    let ws = types::list(&mut tx)
        .await
        .unwrap()
        .into_iter()
        .find(|t| t.key == "workstation")
        .unwrap()
        .id;
    for name in ["One", "Two"] {
        let input: AssetInput = serde_json::from_value(
            json!({ "orgId": acme, "typeId": ws, "name": name, "serialNumber": "SN-1" }),
        )
        .unwrap();
        assets::create(&mut tx, &ctx, &input).await.unwrap();
    }
    let mut r = rule(ds.id, ws, "manual_wins");
    r.create_missing = false;
    let m = mappings::create(&mut tx, &ctx, &r).await.unwrap();
    let result = mappings::run(&mut tx, &ctx, m.id, false).await.unwrap();
    assert_eq!((result.created, result.skipped), (0, 1), "{result:?}");
    assert!(result.problems[0].reason.contains("more than one asset"));
    tx.commit().await.unwrap();
}
