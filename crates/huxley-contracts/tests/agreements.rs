//! Agreements against a real PostgreSQL: which agreement time falls under,
//! allowances drawn down in order, rounding and rates, and the rules for
//! keeping agreements.

use huxley_contracts::ContractsError;
use huxley_contracts::agreements::{self, AgreementInput};
use huxley_contracts::coverage::{self, Evaluation, Outcome};
use huxley_core::{RequestContext, Via};
use huxley_db::testing::{ensure_root, isolated};
use huxley_db::{OrgScope, begin_scoped};
use huxley_servicedesk::tickets::{self, NewTicket, Ticket};
use huxley_servicedesk::time::{self as desk_time, TimeEntryInput};
use rust_decimal::Decimal;
use serde_json::{Value, json};
use sqlx::PgConnection;
use time::macros::date;
use uuid::Uuid;

fn agreement(v: Value) -> AgreementInput {
    serde_json::from_value(v).unwrap()
}

async fn ticket(tx: &mut PgConnection, org: Uuid, title: &str) -> Ticket {
    let ctx = RequestContext::system(Via::System);
    tickets::create(
        tx,
        &ctx,
        &NewTicket {
            org_id: org,
            title: title.into(),
            ..Default::default()
        },
        "T",
        None,
    )
    .await
    .unwrap()
}

async fn log(
    tx: &mut PgConnection,
    user: Uuid,
    t: &Ticket,
    work_type: Uuid,
    day: &str,
    minutes: i32,
) -> Uuid {
    desk_time::log(
        tx,
        user,
        Some(t),
        &TimeEntryInput {
            work_type_id: work_type,
            worked_on: Some(day.into()),
            minutes,
            billable: Some(true),
            notes: None,
            version: None,
        },
    )
    .await
    .unwrap()
    .id
}

fn summary(e: &Evaluation) -> (Outcome, i32, i32, Option<String>) {
    (
        e.outcome,
        e.covered_minutes,
        e.billable_minutes,
        e.amount.map(|a| a.normalize().to_string()),
    )
}

struct World {
    root: Uuid,
    acme: Uuid,
    globex: Uuid,
    user: Uuid,
    remote: Uuid,
    onsite: Uuid,
}

async fn world(tx: &mut PgConnection) -> World {
    let ctx = RequestContext::system(Via::System);
    let root: Uuid = sqlx::query_scalar("SELECT id FROM organizations WHERE parent_id IS NULL")
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    let client = async |tx: &mut PgConnection, name: &str| {
        huxley_orgs::create(tx, &ctx, root, huxley_orgs::OrgKind::Client, name)
            .await
            .unwrap()
            .id
    };
    let acme = client(tx, "Acme").await;
    let globex = client(tx, "Globex").await;
    let user = huxley_auth::create_user(
        tx,
        &ctx,
        "tech@example.com",
        "Tech",
        "agreements passphrase",
    )
    .await
    .unwrap()
    .id;
    let work_type = async |tx: &mut PgConnection, name: &str| -> Uuid {
        sqlx::query_scalar("SELECT id FROM sd_work_types WHERE name = $1")
            .bind(name)
            .fetch_one(tx)
            .await
            .unwrap()
    };
    let remote = work_type(tx, "Remote support").await;
    let onsite = work_type(tx, "Onsite").await;
    World {
        root,
        acme,
        globex,
        user,
        remote,
        onsite,
    }
}

#[tokio::test]
async fn time_falls_under_the_right_agreement_and_draws_down_allowances() {
    let Some(db) = isolated("agreements").await else {
        return;
    };
    let pools = db.pools.clone();
    ensure_root(&pools).await;
    let ctx = RequestContext::system(Via::System);
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    let tx = &mut *tx;
    let world_ = world(tx).await;
    let w = &world_;

    // Acme: remote support covered, two hours a month, 15-minute increments.
    let managed = agreements::create(
        tx,
        &ctx,
        w.acme,
        &agreement(json!({
            "name": "Acme managed", "kind": "managed", "status": "active",
            "startsOn": "2026-01-01", "endsOn": "2026-12-31",
            "coversAllWorkTypes": false, "workTypeIds": [w.remote],
            "monthlyMinutes": 120, "billingIncrement": 15,
            "rates": [{ "hourlyRate": "100" }, { "workTypeId": w.onsite, "hourlyRate": 150 }],
        })),
    )
    .await
    .unwrap();
    // Everyone: time and materials at the MSP.
    let tm = agreements::create(
        tx,
        &ctx,
        w.root,
        &agreement(json!({
            "name": "Standard T&M", "kind": "time_materials", "status": "active",
            "startsOn": "2026-01-01", "rates": [{ "hourlyRate": "90" }],
        })),
    )
    .await
    .unwrap();

    let acme_ticket = ticket(tx, w.acme, "Printer").await;
    let mar2 = log(tx, w.user, &acme_ticket, w.remote, "2026-03-02", 90).await;
    let mar3 = log(tx, w.user, &acme_ticket, w.remote, "2026-03-03", 45).await;
    let mar4 = log(tx, w.user, &acme_ticket, w.remote, "2026-03-04", 20).await;
    let mar5_onsite = log(tx, w.user, &acme_ticket, w.onsite, "2026-03-05", 60).await;
    let apr1 = log(tx, w.user, &acme_ticket, w.remote, "2026-04-01", 30).await;
    let globex_ticket = ticket(tx, w.globex, "Laptop").await;
    let globex_entry = log(tx, w.user, &globex_ticket, w.remote, "2026-03-02", 40).await;

    let evaluated = coverage::evaluate(tx, &[mar2, mar3, mar4, mar5_onsite, apr1, globex_entry])
        .await
        .unwrap();
    let got: Vec<_> = evaluated.iter().map(summary).collect();
    assert_eq!(
        got,
        [
            (Outcome::Covered, 90, 0, None),
            // 30 left: 15 over, rounded to 15, at 100/h.
            (Outcome::PartlyCovered, 30, 15, Some("25".into())),
            // Used up: 20 rounds up to 30.
            (Outcome::Overage, 0, 30, Some("50".into())),
            // Onsite isn't covered: the MSP's T&M bills it.
            (Outcome::Billable, 0, 60, Some("90".into())),
            // A new month, a new allowance.
            (Outcome::Covered, 30, 0, None),
            (Outcome::Billable, 0, 40, Some("60".into())),
        ]
    );
    assert_eq!(evaluated[3].agreement_id, Some(tm.id));
    assert_eq!(evaluated[0].agreement_id, Some(managed.id));

    // Usage for March, and the ticket's view, agree.
    let march = coverage::usage(
        tx,
        managed.id,
        date!(2026 - 03 - 01),
        date!(2026 - 03 - 31),
        false,
    )
    .await
    .unwrap();
    assert_eq!(march.entries.len(), 3);
    assert_eq!((march.covered_minutes, march.billable_minutes), (120, 45));
    assert_eq!(march.amount, Some(Decimal::from(75)));
    assert_eq!(
        coverage::for_ticket(tx, acme_ticket.id)
            .await
            .unwrap()
            .len(),
        5
    );

    // Earlier work logged later still comes first.
    let early = log(tx, w.user, &acme_ticket, w.remote, "2026-03-01", 60).await;
    let again = coverage::evaluate(tx, &[early, mar2]).await.unwrap();
    assert_eq!(summary(&again[0]).0, Outcome::Covered);
    assert_eq!(
        summary(&again[1]),
        (Outcome::PartlyCovered, 60, 30, Some("50".into()))
    );

    keeping_rules(tx, &ctx, w, managed.id).await;
}

async fn keeping_rules(tx: &mut PgConnection, ctx: &RequestContext, w: &World, active: Uuid) {
    let invalid = |r: Result<agreements::Agreement, ContractsError>| {
        assert!(matches!(r, Err(ContractsError::Invalid { .. })), "{r:?}");
    };
    // A block needs its hours; time and materials has no coverage.
    invalid(
        agreements::create(
            tx,
            ctx,
            w.acme,
            &agreement(json!({ "name": "Block", "kind": "block_hours", "startsOn": "2026-01-01" })),
        )
        .await,
    );
    invalid(
        agreements::create(
            tx,
            ctx,
            w.acme,
            &agreement(json!({
                "name": "T&M", "kind": "time_materials", "startsOn": "2026-01-01",
                "workTypeIds": [w.remote],
            })),
        )
        .await,
    );
    invalid(
        agreements::create(
            tx,
            ctx,
            w.acme,
            &agreement(json!({
                "name": "Backwards", "kind": "project", "startsOn": "2026-05-01", "endsOn": "2026-04-01",
            })),
        )
        .await,
    );
    // Lines use the catalog above, in the agreement's currency.
    let product = huxley_contracts::catalog::create(
        tx,
        ctx,
        w.root,
        &serde_json::from_value(json!({
            "sku": "MS-WS", "name": "Managed workstation", "kind": "service", "price": "45", "billing": "monthly",
        }))
        .unwrap(),
    )
    .await
    .unwrap();
    let other = huxley_contracts::catalog::create(
        tx,
        ctx,
        w.globex,
        &serde_json::from_value(json!({ "sku": "G-1", "name": "Globex only", "kind": "service" }))
            .unwrap(),
    )
    .await
    .unwrap();
    let with_lines = agreements::create(
        tx,
        ctx,
        w.acme,
        &agreement(json!({
            "name": "Seats", "kind": "managed", "startsOn": "2026-01-01",
            "lines": [{ "productId": product.id, "quantity": 12 }, { "description": "Setup", "quantity": 1, "unitPrice": "250" }],
        })),
    )
    .await
    .unwrap();
    assert_eq!(with_lines.lines[0].description, "Managed workstation");
    assert_eq!(with_lines.lines[0].effective_price, Some(Decimal::from(45)));
    assert_eq!(
        with_lines.lines[1].effective_price,
        Some(Decimal::from(250))
    );
    invalid(
        agreements::create(
            tx,
            ctx,
            w.acme,
            &agreement(json!({
                "name": "Bad line", "kind": "managed", "startsOn": "2026-01-01",
                "lines": [{ "productId": other.id, "quantity": 1 }],
            })),
        )
        .await,
    );
    // Drafts can be deleted; active agreements are cancelled instead.
    agreements::delete(tx, ctx, with_lines.id).await.unwrap();
    assert!(matches!(
        agreements::delete(tx, ctx, active).await,
        Err(ContractsError::Conflict(_))
    ));
}

#[tokio::test]
async fn agreements_bind_slas_and_renew() {
    let Some(db) = isolated("agreement_renewals").await else {
        return;
    };
    let pools = db.pools.clone();
    ensure_root(&pools).await;
    let ctx = RequestContext::system(Via::System);
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    let tx = &mut *tx;
    for module in ["crm", "servicedesk", "contracts"] {
        huxley_modules::set_enabled(tx, &ctx, module, true, false)
            .await
            .unwrap();
    }
    let world_ = world(tx).await;
    let w = &world_;
    let gold = huxley_servicedesk::sla::create(
        tx,
        &ctx,
        &serde_json::from_value(json!({ "name": "Gold", "hoursSource": "always" })).unwrap(),
    )
    .await
    .unwrap();
    let silver = huxley_servicedesk::sla::create(
        tx,
        &ctx,
        &serde_json::from_value(json!({ "name": "Silver", "hoursSource": "always" })).unwrap(),
    )
    .await
    .unwrap();
    let today = time::OffsetDateTime::now_utc().date();
    let day = |offset: i64| (today + time::Duration::days(offset)).to_string();
    let gold_deal = agreements::create(
        tx,
        &ctx,
        w.acme,
        &agreement(json!({
            "name": "Gold support", "kind": "managed", "status": "active",
            "startsOn": day(-300), "endsOn": day(20), "renewalNoticeDays": 30,
            "slaPolicyId": gold.id,
        })),
    )
    .await
    .unwrap();
    assert_eq!(gold_deal.sla_policy_name.as_deref(), Some("Gold"));
    let source =
        |a: &huxley_servicedesk::sla::SlaAssignment| (a.effective_policy_id, a.source.clone());
    let acme_sla = huxley_servicedesk::sla::assignment(tx, w.acme)
        .await
        .unwrap();
    assert_eq!(source(&acme_sla), (Some(gold.id), Some("agreement".into())));
    // An assignment above doesn't override it.
    huxley_servicedesk::sla::assign(tx, &ctx, w.root, Some(silver.id))
        .await
        .unwrap();
    let acme_sla = huxley_servicedesk::sla::assignment(tx, w.acme)
        .await
        .unwrap();
    assert_eq!(source(&acme_sla), (Some(gold.id), Some("agreement".into())));
    // Globex has no agreement: the assignment above applies.
    let globex_sla = huxley_servicedesk::sla::assignment(tx, w.globex)
        .await
        .unwrap();
    assert_eq!(source(&globex_sla), (Some(silver.id), Some("above".into())));

    // 20 days left, 30 days' notice: due, once.
    let first = huxley_contracts::renewals::sweep(tx, today).await.unwrap();
    assert_eq!((first.announced, first.renewed), (1, 0));
    let again = huxley_contracts::renewals::sweep(tx, today).await.unwrap();
    assert_eq!((again.announced, again.renewed), (0, 0));
    let due: Value = sqlx::query_scalar(
        "SELECT payload FROM events_outbox WHERE event_type = 'contract.renewal_due' ORDER BY occurred_at DESC LIMIT 1",
    )
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(
        (&due["days_left"], &due["name"]),
        (&json!(20), &json!("Gold support"))
    );

    // Renewing automatically: once it ends, the term rolls on (321 days).
    let mut input = agreement(json!({
        "name": "Gold support", "kind": "managed", "status": "active",
        "startsOn": day(-300), "endsOn": day(20), "autoRenew": true, "renewalNoticeDays": 30,
        "slaPolicyId": gold.id, "version": gold_deal.version,
    }));
    agreements::update(tx, &ctx, gold_deal.id, &input)
        .await
        .unwrap();
    let later = today + time::Duration::days(25);
    let swept = huxley_contracts::renewals::sweep(tx, later).await.unwrap();
    assert_eq!(swept.renewed, 1);
    let renewed = agreements::get(tx, gold_deal.id).await.unwrap();
    assert_eq!(renewed.starts_on, today + time::Duration::days(21));
    assert_eq!(
        renewed.ends_on,
        Some(today + time::Duration::days(21 + 320))
    );
    // A bad SLA is refused.
    input.sla_policy_id = Some(uuid::Uuid::now_v7());
    input.version = Some(renewed.version);
    let refused = agreements::update(tx, &ctx, gold_deal.id, &input).await;
    assert!(
        matches!(refused, Err(ContractsError::Invalid { .. })),
        "{refused:?}"
    );
}
