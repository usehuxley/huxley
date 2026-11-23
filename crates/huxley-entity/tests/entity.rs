//! Custom fields and tags against a real PostgreSQL. Each test rolls back:
//! field definitions are shared by every record of a type.

use huxley_core::{RequestContext, Via};
use huxley_db::testing::{ensure_root, pools, unique};
use huxley_db::{OrgScope, begin_scoped};
use huxley_entity::custom_fields::{self, FieldChanges, FieldType, NewField, SelectOption};
use huxley_entity::{EntityError, ORGANIZATION, tags};
use huxley_orgs::OrgKind;
use serde_json::{Map, Value, json};

fn ctx() -> RequestContext {
    RequestContext::system(Via::System)
}

fn field(label: &str, field_type: FieldType) -> NewField {
    NewField {
        entity_type: ORGANIZATION.into(),
        key: None,
        label: label.into(),
        help: String::new(),
        field_type,
        options: vec![],
        required: false,
        scope_org_id: None,
        position: 0,
    }
}

fn changes(pairs: &[(&str, Value)]) -> Map<String, Value> {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_owned(), v.clone()))
        .collect()
}

#[tokio::test]
async fn fields_store_validated_values_on_the_record() {
    let Some(pools) = pools().await else { return };
    let root = ensure_root(&pools).await;
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    let client = huxley_orgs::create(&mut tx, &ctx(), root, OrgKind::Client, &unique("Fields"))
        .await
        .unwrap()
        .id;

    let seats =
        custom_fields::create_def(&mut tx, &ctx(), &field(&unique("Seats"), FieldType::Number))
            .await
            .unwrap();
    let mut tier = field(&unique("Tier"), FieldType::Select);
    tier.options = vec![
        SelectOption {
            value: "gold".into(),
            label: "Gold".into(),
        },
        SelectOption {
            value: "silver".into(),
            label: String::new(),
        },
    ];
    tier.required = true;
    let tier = custom_fields::create_def(&mut tx, &ctx(), &tier)
        .await
        .unwrap();
    assert_eq!(tier.options[1].label, "silver", "label defaults to value");

    let values = custom_fields::set_values(
        &mut tx,
        &ctx(),
        ORGANIZATION,
        client,
        &changes(&[(&seats.key, json!(25)), (&tier.key, json!("gold"))]),
    )
    .await
    .unwrap();
    let get = |key: &str| {
        values
            .iter()
            .find(|v| v.def.key == key)
            .unwrap()
            .value
            .clone()
    };
    assert_eq!(get(&seats.key), json!(25));
    assert_eq!(get(&tier.key), json!("gold"));
    let (stored,): (Value,) =
        sqlx::query_as("SELECT custom_fields FROM organizations WHERE id = $1")
            .bind(client)
            .fetch_one(&mut *tx)
            .await
            .unwrap();
    assert_eq!(stored[&seats.key], json!(25));

    // Invalid values and unknown keys are refused; required can't be cleared.
    for bad in [
        changes(&[(&seats.key, json!("lots"))]),
        changes(&[(&tier.key, json!("bronze"))]),
        changes(&[(&tier.key, Value::Null)]),
        changes(&[("not_a_field", json!(1))]),
    ] {
        assert!(matches!(
            custom_fields::set_values(&mut tx, &ctx(), ORGANIZATION, client, &bad).await,
            Err(EntityError::Invalid { .. })
        ));
    }

    // Clearing an optional field removes it; a change is audited and emitted.
    custom_fields::set_values(
        &mut tx,
        &ctx(),
        ORGANIZATION,
        client,
        &changes(&[(&seats.key, Value::Null)]),
    )
    .await
    .unwrap();
    let (stored,): (Value,) =
        sqlx::query_as("SELECT custom_fields FROM organizations WHERE id = $1")
            .bind(client)
            .fetch_one(&mut *tx)
            .await
            .unwrap();
    assert!(stored.get(&seats.key).is_none());
    let (events,): (i64,) = sqlx::query_as(
        "SELECT count(*) FROM events_outbox WHERE event_type = 'organization.custom_fields_changed' AND subject_id = $1",
    )
    .bind(client)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(events, 2);

    // Keys are unique per entity type, forever.
    let mut dup = field("Anything", FieldType::Text);
    dup.key = Some(seats.key.clone());
    assert!(matches!(
        custom_fields::create_def(&mut tx, &ctx(), &dup).await,
        Err(EntityError::KeyTaken)
    ));
    tx.rollback().await.unwrap();
}

#[tokio::test]
async fn scoped_fields_apply_to_one_subtree_and_archiving_hides_them() {
    let Some(pools) = pools().await else { return };
    let root = ensure_root(&pools).await;
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    let a = huxley_orgs::create(&mut tx, &ctx(), root, OrgKind::Client, &unique("A"))
        .await
        .unwrap()
        .id;
    let a_site = huxley_orgs::create(&mut tx, &ctx(), a, OrgKind::Client, &unique("A site"))
        .await
        .unwrap()
        .id;
    let b = huxley_orgs::create(&mut tx, &ctx(), root, OrgKind::Client, &unique("B"))
        .await
        .unwrap()
        .id;
    let mut only_a = field(&unique("Badge colour"), FieldType::Text);
    only_a.scope_org_id = Some(a);
    let def = custom_fields::create_def(&mut tx, &ctx(), &only_a)
        .await
        .unwrap();

    let has = |values: &[custom_fields::FieldValue]| values.iter().any(|v| v.def.id == def.id);
    assert!(has(&custom_fields::values(&mut tx, ORGANIZATION, a)
        .await
        .unwrap()));
    assert!(has(&custom_fields::values(&mut tx, ORGANIZATION, a_site)
        .await
        .unwrap()));
    assert!(!has(&custom_fields::values(&mut tx, ORGANIZATION, b)
        .await
        .unwrap()));
    let on_b = changes(&[(&def.key, json!("red"))]);
    assert!(
        custom_fields::set_values(&mut tx, &ctx(), ORGANIZATION, b, &on_b)
            .await
            .is_err()
    );

    // Archived: hidden, value kept, back on restore.
    custom_fields::set_values(
        &mut tx,
        &ctx(),
        ORGANIZATION,
        a,
        &changes(&[(&def.key, json!("red"))]),
    )
    .await
    .unwrap();
    custom_fields::set_archived(&mut tx, &ctx(), def.id, true)
        .await
        .unwrap();
    assert!(!has(&custom_fields::values(&mut tx, ORGANIZATION, a)
        .await
        .unwrap()));
    custom_fields::set_archived(&mut tx, &ctx(), def.id, false)
        .await
        .unwrap();
    let restored = custom_fields::values(&mut tx, ORGANIZATION, a)
        .await
        .unwrap();
    assert_eq!(
        restored.iter().find(|v| v.def.id == def.id).unwrap().value,
        json!("red")
    );

    // Widen to everyone.
    let wide = FieldChanges {
        label: None,
        help: None,
        options: None,
        required: None,
        scope_org_id: None,
        clear_scope: true,
        position: None,
    };
    custom_fields::update_def(&mut tx, &ctx(), def.id, &wide)
        .await
        .unwrap();
    assert!(has(&custom_fields::values(&mut tx, ORGANIZATION, b)
        .await
        .unwrap()));
    tx.rollback().await.unwrap();
}

#[tokio::test]
async fn tags_apply_to_records_and_go_when_deleted() {
    let Some(pools) = pools().await else { return };
    let root = ensure_root(&pools).await;
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    let client = huxley_orgs::create(&mut tx, &ctx(), root, OrgKind::Client, &unique("Tagged"))
        .await
        .unwrap()
        .id;
    let input = tags::TagInput {
        name: unique("VIP"),
        color: "warning".into(),
        description: String::new(),
    };
    let vip = tags::create(&mut tx, &ctx(), &input).await.unwrap();
    sqlx::query("SAVEPOINT dup")
        .execute(&mut *tx)
        .await
        .unwrap();
    assert!(matches!(
        tags::create(&mut tx, &ctx(), &input).await,
        Err(EntityError::TagNameTaken)
    ));
    sqlx::query("ROLLBACK TO SAVEPOINT dup")
        .execute(&mut *tx)
        .await
        .unwrap();

    tags::apply(&mut tx, &ctx(), ORGANIZATION, client, vip.id)
        .await
        .unwrap();
    tags::apply(&mut tx, &ctx(), ORGANIZATION, client, vip.id)
        .await
        .unwrap(); // idempotent
    let on = tags::of_entity(&mut tx, ORGANIZATION, client)
        .await
        .unwrap();
    assert_eq!(on.len(), 1);
    assert_eq!(on[0].usage, 1);
    tags::remove(&mut tx, &ctx(), ORGANIZATION, client, vip.id)
        .await
        .unwrap();
    assert!(
        tags::of_entity(&mut tx, ORGANIZATION, client)
            .await
            .unwrap()
            .is_empty()
    );

    tags::apply(&mut tx, &ctx(), ORGANIZATION, client, vip.id)
        .await
        .unwrap();
    tags::delete(&mut tx, &ctx(), vip.id).await.unwrap();
    assert!(
        tags::of_entity(&mut tx, ORGANIZATION, client)
            .await
            .unwrap()
            .is_empty()
    );
    let bad = tags::TagInput {
        name: "x".into(),
        color: "chartreuse".into(),
        description: String::new(),
    };
    assert!(matches!(
        tags::create(&mut tx, &ctx(), &bad).await,
        Err(EntityError::Invalid { .. })
    ));
    tx.rollback().await.unwrap();
}
