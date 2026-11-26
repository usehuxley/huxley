//! The timesheet reminder workflows against a real PostgreSQL: who's late
//! (and their lead), who logged a short day, and whose entries lack notes.

use std::time::{Duration, Instant};

use huxley_core::{RequestContext, Via};
use huxley_db::testing::{TestPools, ensure_root, isolated};
use huxley_db::{OrgScope, begin_scoped};
use huxley_engine::{Engine, RunStatus, StartRun};
use serde_json::{Value as Json, json};
use uuid::Uuid;

const UNSUBMITTED: &str = include_str!(
    "../../huxley-packs/builtin/timesheet-reminders/workflows/timesheet-unsubmitted.yaml"
);
const UNDER_LOGGED: &str = include_str!(
    "../../huxley-packs/builtin/timesheet-reminders/workflows/timesheet-under-logged.yaml"
);
const MISSING_NOTES: &str = include_str!(
    "../../huxley-packs/builtin/timesheet-reminders/workflows/timesheet-missing-notes.yaml"
);

async fn run(pools: &TestPools, engine: &Engine, root: Uuid, workflow: &str, inputs: Json) -> Json {
    let document = serde_json::to_value(huxley_hwf::parse_yaml(workflow).unwrap()).unwrap();
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    let id = engine
        .start_run(
            &mut tx,
            StartRun {
                org_id: root,
                workflow_id: None,
                version: None,
                document: Some(document),
                trigger_type: "manual".into(),
                trigger: json!({}),
                inputs,
                actor_id: None,
            },
        )
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        engine.drain("reminders", 50).await.unwrap();
        let (status, outputs, error): (RunStatus, Option<Json>, Option<Json>) =
            sqlx::query_as("SELECT status, outputs, error FROM runs WHERE id = $1")
                .bind(id)
                .fetch_one(&pools.owner)
                .await
                .unwrap();
        if status.is_finished() {
            assert_eq!(status, RunStatus::Succeeded, "{error:?}");
            return outputs.unwrap_or_default();
        }
        assert!(Instant::now() < deadline, "run didn't finish");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// Schedules, yesterday's entries (the technician's short and without
/// notes), and `done`'s submitted timesheets: this period's start and
/// yesterday.
async fn seed(tx: &mut sqlx::PgConnection, tech: Uuid, done: Uuid) -> (time::Date, time::Date) {
    // Every day 9 to 5, so "yesterday" is a working day whenever this runs.
    let every_day: Json = ["mon", "tue", "wed", "thu", "fri", "sat", "sun"]
        .iter()
        .map(|d| (d.to_string(), json!([["09:00", "17:00"]])))
        .collect::<serde_json::Map<_, _>>()
        .into();
    for person in [tech, done] {
        sqlx::query("INSERT INTO wf_schedules (user_id, timezone, hours) VALUES ($1, 'UTC', $2)")
            .bind(person)
            .bind(&every_day)
            .execute(&mut *tx)
            .await
            .unwrap();
    }
    let work_type: Uuid = sqlx::query_scalar("SELECT id FROM sd_work_types WHERE name = 'Admin'")
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    let today = time::OffsetDateTime::now_utc().date();
    let yesterday = today - time::Duration::days(1);
    for (person, day, minutes, notes) in [
        (tech, yesterday, 60, ""),
        (done, yesterday, 480, "Onboarding"),
    ] {
        sqlx::query(
            "INSERT INTO sd_time_entries (id, user_id, work_type_id, worked_on, minutes, billable, notes) \
             VALUES ($1, $2, $3, $4, $5, false, $6)",
        )
        .bind(Uuid::now_v7())
        .bind(person)
        .bind(work_type)
        .bind(day)
        .bind(minutes)
        .bind(notes)
        .execute(&mut *tx)
        .await
        .unwrap();
    }
    // `done` already submitted last period.
    let (period_days, anchor): (i32, time::Date) =
        sqlx::query_as("SELECT period_days, anchor FROM wf_timesheet_settings")
            .fetch_one(&mut *tx)
            .await
            .unwrap();
    let (this_start, _) = huxley_workforce::timesheets::period_of(period_days, anchor, today);
    for start in [
        this_start - time::Duration::days(7),
        this_start - time::Duration::days(14),
    ] {
        sqlx::query(
            "INSERT INTO wf_timesheets (id, user_id, period_start, period_end, status) \
             VALUES ($1, $2, $3, $4, 'submitted')",
        )
        .bind(Uuid::now_v7())
        .bind(done)
        .bind(start)
        .bind(start + time::Duration::days(6))
        .execute(&mut *tx)
        .await
        .unwrap();
    }
    (this_start, yesterday)
}

async fn make_user(pools: &TestPools, email: &str) -> Uuid {
    let ctx = RequestContext::system(Via::System);
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    let id = huxley_auth::create_user(&mut tx, &ctx, email, email, "reminder passphrase")
        .await
        .unwrap()
        .id;
    tx.commit().await.unwrap();
    id
}

async fn titles(pools: &TestPools, user: Uuid) -> Vec<String> {
    sqlx::query_scalar(
        "SELECT title FROM notifications WHERE user_id = $1 AND kind = 'workflow.notification' \
          ORDER BY created_at",
    )
    .bind(user)
    .fetch_all(&pools.owner)
    .await
    .unwrap()
}

#[tokio::test]
async fn reminders_reach_the_right_people() {
    let Some(db) = isolated("timesheet_reminders").await else {
        return;
    };
    let pools = db.pools.clone();
    let root = ensure_root(&pools).await;
    let ctx = RequestContext::system(Via::System);
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    for module in ["crm", "servicedesk", "workforce"] {
        huxley_modules::set_enabled(&mut tx, &ctx, module, true, false)
            .await
            .unwrap();
    }
    tx.commit().await.unwrap();
    let tech = make_user(&pools, "tech@example.com").await;
    let lead = make_user(&pools, "lead@example.com").await;
    let done = make_user(&pools, "done@example.com").await;
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    let team = huxley_authz::teams::create(&mut tx, &ctx, "Desk", "")
        .await
        .unwrap();
    huxley_authz::teams::add_member(&mut tx, &ctx, team.id, lead, Some(true))
        .await
        .unwrap();
    huxley_authz::teams::add_member(&mut tx, &ctx, team.id, tech, None)
        .await
        .unwrap();
    let (this_start, yesterday) = seed(&mut tx, tech, done).await;
    tx.commit().await.unwrap();

    let engine = Engine::new(
        pools.app.clone(),
        huxley_crypto::Crypto::new(pools.app.clone(), huxley_crypto::testing::key_ring()),
    );
    // Late: the technician (not `done`), and their lead (from day 0 here).
    let out = run(
        &pools,
        &engine,
        root,
        UNSUBMITTED,
        json!({ "lead_after_days": 0 }),
    )
    .await;
    assert_eq!(out["reminded"], 1, "{out}");
    let t = titles(&pools, tech).await;
    assert!(t[0].contains("isn't submitted"), "{t:?}");
    assert!(titles(&pools, lead).await[0].starts_with("tech@example.com's timesheet"));
    assert_eq!(titles(&pools, done).await.len(), 0);

    // Short day: the technician logged 1h of 8h yesterday.
    let out = run(&pools, &engine, root, UNDER_LOGGED, json!({})).await;
    assert_eq!(out["reminded"], 1, "{out}");
    assert_eq!(
        titles(&pools, tech).await[1],
        "Yesterday: 1h 0m logged of 8h 0m"
    );

    // Missing notes: the technician's entry has none (if yesterday is in
    // this period).
    let out = run(&pools, &engine, root, MISSING_NOTES, json!({})).await;
    let expected = i32::from(yesterday >= this_start);
    assert_eq!(out["reminded"], expected, "{out}");
}
