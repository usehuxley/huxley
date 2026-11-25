//! Assets on tickets, and dependency maps, against a real PostgreSQL.

use huxley_cmdb::assets::{self, AssetInput};
use huxley_cmdb::relationships::{self, RelationshipInput};
use huxley_cmdb::{CmdbError, tickets, types};
use huxley_core::{RequestContext, Via};
use huxley_db::testing::{TestPools, ensure_root, pools, unique};
use huxley_db::{OrgScope, begin_scoped};
use huxley_servicedesk::tickets::NewTicket;
use serde_json::Map;
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
        &unique("Map client"),
    )
    .await
    .unwrap()
    .id;
    tx.commit().await.unwrap();
    org
}

fn input(org: Uuid, type_id: Uuid, name: &str, contact: Option<Uuid>) -> AssetInput {
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
        contact_id: contact,
        purchased_on: None,
        warranty_ends_on: None,
        notes: None,
        attributes: Map::new(),
        external_ids: None,
        version: None,
    }
}

#[tokio::test]
async fn assets_are_linked_to_tickets_and_the_requesters_suggested() {
    let Some(pools) = pools().await else { return };
    let org = client(&pools).await;
    let other = client(&pools).await;
    let ctx = RequestContext::system(Via::System);
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    let ada = Uuid::now_v7();
    sqlx::query("INSERT INTO crm_contacts (id, org_id, first_name) VALUES ($1, $2, 'Ada')")
        .bind(ada)
        .bind(org)
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("INSERT INTO crm_contact_companies (contact_id, org_id) VALUES ($1, $2)")
        .bind(ada)
        .bind(org)
        .execute(&mut *tx)
        .await
        .unwrap();
    let ws = types::list(&mut tx)
        .await
        .unwrap()
        .into_iter()
        .find(|t| t.key == "workstation")
        .unwrap()
        .id;
    let laptop = assets::create(&mut tx, &ctx, &input(org, ws, "Ada's laptop", Some(ada)))
        .await
        .unwrap()
        .id;
    let printer = assets::create(&mut tx, &ctx, &input(org, ws, "Printer", None))
        .await
        .unwrap()
        .id;
    let theirs = assets::create(&mut tx, &ctx, &input(other, ws, "Theirs", None))
        .await
        .unwrap()
        .id;
    let ticket = huxley_servicedesk::tickets::create(
        &mut tx,
        &ctx,
        &NewTicket {
            org_id: org,
            title: "Laptop is slow".into(),
            requester_id: Some(ada),
            ..NewTicket::default()
        },
        "T",
        None,
    )
    .await
    .unwrap();

    // The requester's assets are suggested; linking takes them out.
    let suggested = tickets::requester_assets(&mut tx, ticket.id).await.unwrap();
    assert_eq!(suggested.iter().map(|a| a.id).collect::<Vec<_>>(), [laptop]);
    tickets::link(&mut tx, &ctx, laptop, ticket.id)
        .await
        .unwrap();
    tickets::link(&mut tx, &ctx, laptop, ticket.id)
        .await
        .unwrap();
    tickets::link(&mut tx, &ctx, printer, ticket.id)
        .await
        .unwrap();
    assert!(
        tickets::requester_assets(&mut tx, ticket.id)
            .await
            .unwrap()
            .is_empty()
    );
    let linked = tickets::for_ticket(&mut tx, ticket.id).await.unwrap();
    assert_eq!(
        linked.iter().map(|a| a.id).collect::<Vec<_>>(),
        [laptop, printer]
    );
    assert_eq!(
        tickets::for_asset(&mut tx, laptop).await.unwrap()[0].reference,
        ticket.reference
    );
    // Not another client's asset.
    assert!(matches!(
        tickets::link(&mut tx, &ctx, theirs, ticket.id).await,
        Err(CmdbError::Invalid { .. })
    ));
    tickets::unlink(&mut tx, &ctx, printer, ticket.id)
        .await
        .unwrap();
    assert!(matches!(
        tickets::unlink(&mut tx, &ctx, printer, ticket.id).await,
        Err(CmdbError::NotFound)
    ));
    tx.commit().await.unwrap();
}

#[tokio::test]
async fn a_map_shows_the_neighbourhood_both_ways() {
    let Some(pools) = pools().await else { return };
    let org = client(&pools).await;
    let ctx = RequestContext::system(Via::System);
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    let server = types::list(&mut tx)
        .await
        .unwrap()
        .into_iter()
        .find(|t| t.key == "server")
        .unwrap()
        .id;
    let mut ids = Vec::new();
    for name in ["Host", "VM", "App", "Far", "Island"] {
        ids.push(
            assets::create(&mut tx, &ctx, &input(org, server, name, None))
                .await
                .unwrap()
                .id,
        );
    }
    let rel = |kind: &str, to| RelationshipInput {
        kind: kind.into(),
        to_id: to,
    };
    relationships::add(&mut tx, &ctx, ids[0], &rel("hosts", ids[1]))
        .await
        .unwrap();
    relationships::add(&mut tx, &ctx, ids[2], &rel("runs_on", ids[1]))
        .await
        .unwrap();
    relationships::add(&mut tx, &ctx, ids[3], &rel("depends_on", ids[2]))
        .await
        .unwrap();

    // From the VM: the host and the app at 1, Far at 2; not the island.
    let m = relationships::map(&mut tx, ids[1], 2).await.unwrap();
    let nodes: Vec<(&str, i32)> = m.nodes.iter().map(|n| (n.name.as_str(), n.depth)).collect();
    assert_eq!(nodes, [("VM", 0), ("App", 1), ("Host", 1), ("Far", 2)]);
    assert_eq!(m.edges.len(), 3);
    assert!(!m.truncated);
    // One step only: Far and its edge are left out.
    let m = relationships::map(&mut tx, ids[1], 1).await.unwrap();
    assert_eq!((m.nodes.len(), m.edges.len()), (3, 2));
    tx.commit().await.unwrap();
}
