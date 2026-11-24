//! Overlays: edits to an integration's actions, kept across new versions
//! (also when an action is renamed) and never touching the stored specs.

use huxley_core::{RequestContext, Via};
use huxley_db::testing::{ensure_root, pools};
use huxley_db::{OrgScope, begin_scoped};
use huxley_integrations::overlay::{ActionOverlay, Overlay};
use huxley_integrations::{store, usage};
use serde_json::Value;

fn spec(list_id: &str) -> String {
    format!(
        "openapi: 3.0.3\ninfo: {{ title: Desk, version: '1' }}\nservers: [ {{ url: 'https://desk.example.com' }} ]\n\
         paths:\n  /tickets:\n    get: {{ operationId: {list_id}, tags: [Tickets], responses: {{ '200': {{ description: ok }} }} }}\n\
         \x20   post: {{ operationId: createTicket, responses: {{ '201': {{ description: ok }} }} }}\n"
    )
}

#[tokio::test]
async fn overlays_survive_new_versions_and_renames() {
    let Some(pools) = pools().await else { return };
    let org = ensure_root(&pools).await;
    let ctx = RequestContext::system(Via::System);
    let key = format!("desk-{}", &uuid::Uuid::now_v7().simple().to_string()[20..]);
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    let created = store::create(&mut tx, &ctx, org, Some(&key), &spec("listTickets"))
        .await
        .unwrap();
    let id = created.summary.id;
    let overlay = Overlay {
        actions: [
            (
                "listTickets".to_owned(),
                ActionOverlay {
                    name: Some("  Find tickets ".into()),
                    group: Some("Service desk".into()),
                    ..ActionOverlay::default()
                },
            ),
            (
                "createTicket".to_owned(),
                ActionOverlay {
                    hidden: true,
                    ..ActionOverlay::default()
                },
            ),
            ("unused".to_owned(), ActionOverlay::default()),
        ]
        .into(),
    };
    let detail = store::set_overlay(&mut tx, &ctx, id, overlay)
        .await
        .unwrap();
    assert_eq!(detail.overlay.actions.len(), 2, "empty edits are dropped");
    let list = detail
        .definition
        .actions
        .iter()
        .find(|a| a.key == "listTickets")
        .unwrap();
    assert_eq!(list.name, "Find tickets");
    assert_eq!(list.group.as_deref(), Some("Service desk"));
    assert!(
        detail
            .definition
            .actions
            .iter()
            .any(|a| a.key == "createTicket" && a.hidden)
    );

    // Pickers don't offer hidden actions, and show the new name.
    let options = usage::available_actions(&mut tx, org).await.unwrap();
    let mine: Vec<_> = options.iter().filter(|o| o.integration == key).collect();
    assert_eq!(mine.len(), 1);
    assert_eq!(
        serde_json::to_value(mine[0]).unwrap()["name"],
        "Find tickets"
    );

    // A new version renames the action: the edit follows it.
    let updated = store::update(&mut tx, &ctx, id, &spec("searchTickets"), Some(1))
        .await
        .unwrap();
    let renamed = updated
        .definition
        .actions
        .iter()
        .find(|a| a.key == "searchTickets")
        .unwrap();
    assert_eq!(renamed.name, "Find tickets");
    // The stored definitions are the vendor's.
    let stored: Value = sqlx::query_scalar(
        "SELECT definition FROM integration_versions WHERE integration_id = $1 AND version = 2",
    )
    .bind(id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert!(!stored.to_string().contains("Find tickets"));
    assert!(!stored.to_string().contains("hidden"));

    // Too long: refused at that field.
    let long = Overlay {
        actions: [(
            "createTicket".to_owned(),
            ActionOverlay {
                name: Some("x".repeat(201)),
                ..ActionOverlay::default()
            },
        )]
        .into(),
    };
    let err = store::set_overlay(&mut tx, &ctx, id, long)
        .await
        .unwrap_err();
    assert!(matches!(err, store::StoreError::Invalid(at, _) if at == "actions.createTicket.name"));
}
