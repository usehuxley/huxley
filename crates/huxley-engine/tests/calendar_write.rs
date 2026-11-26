//! Calendar write-back (ADR-062): `core.calendar.write` puts an appointment
//! in its person's Microsoft 365 calendar (a mock Graph on localhost), keeps
//! it in step as it moves and changes hands, and removes it when it's
//! cancelled; `core.calendar.sync` reports busy time that clashes with an
//! appointment once, and doesn't count Huxley's own events as busy time.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{patch, post};
use huxley_core::{RequestContext, Via};
use huxley_crypto::Crypto;
use huxley_db::testing::{ensure_root, isolated};
use huxley_db::{OrgScope, begin_scoped};
use huxley_engine::{EgressConfig, Engine, EngineConfig, RunStatus, StartRun};
use huxley_integrations::connections::{self, ConnectionInput};
use huxley_integrations::store;
use huxley_servicedesk::appointments::{self, AppointmentInput};
use huxley_workforce::calendar;
use serde_json::{Map, Value as Json, json};
use time::OffsetDateTime;
use uuid::Uuid;

const MSGRAPH: &str = include_str!("../../huxley-packs/builtin/msgraph/integrations/msgraph.yaml");

/// The mock calendars: events by id (whose, and the body), and a log of
/// the calls (`POST ada@…`, `PATCH ada@… evt-1`, `DELETE …`).
#[derive(Default)]
struct Calendars {
    events: BTreeMap<String, (String, Json)>,
    log: Vec<String>,
    next: u32,
}

type Shared = Arc<Mutex<Calendars>>;

/// Tomorrow at an hour (UTC).
fn tomorrow(hour: i64) -> OffsetDateTime {
    let now = OffsetDateTime::now_utc().unix_timestamp();
    OffsetDateTime::from_unix_timestamp((now / 86_400 + 1) * 86_400 + hour * 3600).unwrap()
}

fn graph_at(t: OffsetDateTime) -> Json {
    let text = jiff::Timestamp::from_second(t.unix_timestamp())
        .unwrap()
        .strftime("%Y-%m-%dT%H:%M:%S")
        .to_string();
    json!({ "dateTime": text, "timeZone": "UTC" })
}

async fn create(
    State(c): State<Shared>,
    Path(user): Path<String>,
    axum::Json(body): axum::Json<Json>,
) -> (StatusCode, axum::Json<Json>) {
    let mut c = c.lock().unwrap();
    c.next += 1;
    let id = format!("evt-{}", c.next);
    c.log.push(format!("POST {user}"));
    c.events.insert(id.clone(), (user, body));
    (StatusCode::CREATED, axum::Json(json!({ "id": id })))
}

async fn update(
    State(c): State<Shared>,
    Path((user, id)): Path<(String, String)>,
    axum::Json(body): axum::Json<Json>,
) -> (StatusCode, axum::Json<Json>) {
    let mut c = c.lock().unwrap();
    c.log.push(format!("PATCH {user} {id}"));
    match c.events.get_mut(&id) {
        Some(event) => {
            event.1 = body;
            (StatusCode::OK, axum::Json(json!({ "id": id })))
        }
        None => (
            StatusCode::NOT_FOUND,
            axum::Json(json!({ "error": { "code": "ErrorItemNotFound" } })),
        ),
    }
}

async fn delete(State(c): State<Shared>, Path((user, id)): Path<(String, String)>) -> StatusCode {
    let mut c = c.lock().unwrap();
    c.log.push(format!("DELETE {user} {id}"));
    if c.events.remove(&id).is_some() {
        StatusCode::NO_CONTENT
    } else {
        StatusCode::NOT_FOUND
    }
}

/// Free/busy: Ada's meeting tomorrow 15:00–16:00, plus every event Huxley
/// wrote (each person's own).
async fn schedule(State(c): State<Shared>, axum::Json(body): axum::Json<Json>) -> axum::Json<Json> {
    let c = c.lock().unwrap();
    let value: Vec<Json> = body["schedules"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| {
            let who = s.as_str().unwrap();
            let mut items: Vec<Json> = c
                .events
                .values()
                .filter(|(user, _)| user == who)
                .map(|(_, e)| json!({ "status": "busy", "start": e["start"], "end": e["end"] }))
                .collect();
            if who == "ada@contoso.example" {
                items.push(json!({ "status": "busy", "start": graph_at(tomorrow(15)), "end": graph_at(tomorrow(16)) }));
            }
            json!({ "scheduleId": who, "scheduleItems": items })
        })
        .collect();
    axum::Json(json!({ "value": value }))
}

async fn mock(state: Shared) -> u16 {
    let app = axum::Router::new()
        .route(
            "/token",
            post(|| async {
                axum::Json(json!({ "access_token": "graph-token", "expires_in": 3600 }))
            }),
        )
        .route("/v1.0/users/{user}/calendar/getSchedule", post(schedule))
        .route("/v1.0/users/{user}/events", post(create))
        .route(
            "/v1.0/users/{user}/events/{id}",
            patch(update).delete(delete),
        )
        .with_state(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    port
}

/// The msgraph integration and connection at `root`, pointed at the mock.
async fn graph(pools: &huxley_db::testing::TestPools, crypto: &Crypto, root: Uuid, port: u16) {
    let ctx = RequestContext::system(Via::System);
    let spec = MSGRAPH
        .replace(
            "https://graph.microsoft.com/v1.0",
            &format!("http://localhost:{port}/v1.0"),
        )
        .replace(
            "https://login.microsoftonline.com/{tenant}/oauth2/v2.0/token",
            &format!("http://localhost:{port}/token"),
        );
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    let integration = store::create(&mut tx, &ctx, root, Some("msgraph"), &spec)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let mut settings = Map::new();
    settings.insert("client_id".into(), json!("huxley"));
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    connections::create(
        &mut tx,
        crypto,
        &ctx,
        root,
        integration.summary.id,
        &ConnectionInput {
            name: "Contoso".into(),
            key: Some("contoso".into()),
            auth_scheme: Some("app".into()),
            settings,
            secrets: [("client_secret".to_owned(), "s3cret".to_owned())].into(),
            ..ConnectionInput::default()
        },
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
}

/// Runs one step at `org` until it finishes: its output.
async fn step(
    engine: &Engine,
    pools: &huxley_db::testing::TestPools,
    org: Uuid,
    step: &str,
) -> Json {
    let yaml = format!(
        "apiVersion: usehuxley.io/v1\nkind: Workflow\nmetadata: {{ key: t, name: T }}\nspec:\n  steps:\n    - id: s\n{step}\n  outputs:\n    result: \"{{{{ steps.s.output }}}}\"\n"
    );
    let document = serde_json::to_value(huxley_hwf::parse_yaml(&yaml).unwrap()).unwrap();
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    let id = engine
        .start_run(
            &mut tx,
            StartRun {
                org_id: org,
                workflow_id: None,
                version: None,
                document: Some(document),
                trigger_type: "manual".into(),
                trigger: json!({}),
                inputs: json!({}),
                actor_id: None,
            },
        )
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        engine.drain("calendar-write", 50).await.unwrap();
        let (status, outputs, error): (RunStatus, Option<Json>, Option<Json>) =
            sqlx::query_as("SELECT status, outputs, error FROM runs WHERE id = $1")
                .bind(id)
                .fetch_one(&pools.owner)
                .await
                .unwrap();
        if status.is_finished() {
            assert_eq!(status, RunStatus::Succeeded, "{error:?}");
            return outputs.unwrap()["result"].clone();
        }
        assert!(Instant::now() < deadline, "run didn't finish");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

async fn write(
    engine: &Engine,
    pools: &huxley_db::testing::TestPools,
    org: Uuid,
    appointment: Uuid,
) -> Json {
    step(
        engine,
        pools,
        org,
        &format!(
            "      type: core.calendar.write\n      with: {{ appointment_id: \"{appointment}\" }}"
        ),
    )
    .await
}

async fn person(pools: &huxley_db::testing::TestPools, email: &str) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query("INSERT INTO users (id, email, display_name) VALUES ($1, $2, $3)")
        .bind(id)
        .bind(email)
        .bind(email.split('@').next().unwrap())
        .execute(&pools.owner)
        .await
        .unwrap();
    id
}

/// Modules on, Graph set up with writing back, Ada and Bob, and a ticket.
async fn world(
    pools: &huxley_db::testing::TestPools,
    crypto: &Crypto,
    port: u16,
) -> (Uuid, Uuid, Uuid, huxley_servicedesk::tickets::Ticket) {
    let root = ensure_root(pools).await;
    let ctx = RequestContext::system(Via::System);
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    for module in ["crm", "servicedesk", "workforce"] {
        huxley_modules::set_enabled(&mut tx, &ctx, module, true, false)
            .await
            .unwrap();
    }
    tx.commit().await.unwrap();
    graph(pools, crypto, root, port).await;
    let (ada, bob) = (
        person(pools, "ada@contoso.example").await,
        person(pools, "bob@contoso.example").await,
    );
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    let settings = calendar::SettingsInput {
        provider: "m365".into(),
        org_id: root,
        connection: "contoso".into(),
        keep_subjects: false,
        write_back: true,
    };
    calendar::set_settings(&mut tx, &ctx, &settings)
        .await
        .unwrap();
    let client = huxley_orgs::create(&mut tx, &ctx, root, huxley_orgs::OrgKind::Client, "Acme")
        .await
        .unwrap()
        .id;
    let ticket = huxley_servicedesk::tickets::create(
        &mut tx,
        &ctx,
        &huxley_servicedesk::tickets::NewTicket {
            org_id: client,
            title: "Firewall swap".into(),
            ..Default::default()
        },
        "T",
        None,
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    (root, ada, bob, ticket)
}

fn booking(user: Uuid, from: OffsetDateTime, hours: i64, version: Option<i32>) -> AppointmentInput {
    AppointmentInput {
        user_id: user,
        starts_at: from,
        ends_at: from + time::Duration::hours(hours),
        notes: Some("Bring the spare".into()),
        version,
    }
}

#[tokio::test]
async fn appointments_are_written_to_calendars_and_kept_in_step() {
    let Some(db) = isolated("calendar_write").await else {
        return;
    };
    let pools = db.pools.clone();
    let state: Shared = Arc::default();
    let port = mock(state.clone()).await;
    let crypto = Crypto::new(pools.app.clone(), huxley_crypto::testing::key_ring());
    let (root, ada, bob, ticket) = world(&pools, &crypto, port).await;
    let config = EngineConfig {
        egress: EgressConfig {
            private_hosts: vec!["localhost".into()],
            ..EgressConfig::default()
        },
        ..EngineConfig::default()
    };
    let engine = Engine::with_config(pools.app.clone(), crypto, &config)
        .with_public_url(Some("https://huxley.example".into()));
    let ctx = RequestContext::system(Via::System);

    // Booked: created in Ada's calendar, idempotently, linking the ticket.
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    let booked = appointments::book(&mut tx, &ctx, &ticket, &booking(ada, tomorrow(9), 2, None))
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let first = write(&engine, &pools, root, booked.id).await;
    assert_eq!(
        (&first["written"], &first["action"]),
        (&json!(true), &json!("created")),
        "{first}"
    );
    {
        let c = state.lock().unwrap();
        let (who, body) = &c.events["evt-1"];
        assert_eq!(who, "ada@contoso.example");
        assert_eq!(body["transactionId"], json!(booked.id));
        assert!(
            body["subject"].as_str().unwrap().ends_with("Firewall swap"),
            "{body}"
        );
        assert!(
            body["body"]["content"]
                .as_str()
                .unwrap()
                .contains(&format!("https://huxley.example/tickets/{}", ticket.id))
        );
        assert_eq!(body["start"], graph_at(tomorrow(9)));
    }

    // Moved: the same event follows.
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    let moved = appointments::reschedule(
        &mut tx,
        &ctx,
        booked.id,
        &booking(ada, tomorrow(11), 2, Some(booked.version)),
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(
        write(&engine, &pools, root, booked.id).await["action"],
        "updated"
    );
    assert_eq!(
        state.lock().unwrap().events["evt-1"].1["start"],
        graph_at(tomorrow(11))
    );

    // Handed to Bob: out of Ada's calendar, into his.
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    appointments::reschedule(
        &mut tx,
        &ctx,
        booked.id,
        &booking(bob, tomorrow(11), 2, Some(moved.version)),
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(
        write(&engine, &pools, root, booked.id).await["action"],
        "created"
    );
    {
        let c = state.lock().unwrap();
        assert_eq!(
            c.log[c.log.len() - 2..],
            [
                "DELETE ada@contoso.example evt-1".to_owned(),
                "POST bob@contoso.example".to_owned()
            ]
        );
        assert_eq!(c.events.keys().collect::<Vec<_>>(), ["evt-2"]);
    }

    cancelled(&engine, &pools, &state, root, booked.id).await;
    conflicts(&engine, &pools, root, ada, &ticket).await;
}

/// Cancelled: removed from the calendar, and forgotten.
async fn cancelled(
    engine: &Engine,
    pools: &huxley_db::testing::TestPools,
    state: &Shared,
    root: Uuid,
    id: Uuid,
) {
    let ctx = RequestContext::system(Via::System);
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    appointments::cancel(&mut tx, &ctx, id).await.unwrap();
    tx.commit().await.unwrap();
    assert_eq!(write(engine, pools, root, id).await["action"], "removed");
    assert!(state.lock().unwrap().events.is_empty());
    let left: i64 = sqlx::query_scalar("SELECT count(*) FROM wf_calendar_writes")
        .fetch_one(&pools.owner)
        .await
        .unwrap();
    assert_eq!(left, 0);
}

/// Ada is booked over her 15:00 meeting: syncing reports it once; her
/// written event isn't busy time on top of the appointment.
async fn conflicts(
    engine: &Engine,
    pools: &huxley_db::testing::TestPools,
    root: Uuid,
    ada: Uuid,
    ticket: &huxley_servicedesk::tickets::Ticket,
) {
    let ctx = RequestContext::system(Via::System);
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    let clash = appointments::book(&mut tx, &ctx, ticket, &booking(ada, tomorrow(15), 2, None))
        .await
        .unwrap();
    tx.commit().await.unwrap();
    write(engine, pools, root, clash.id).await;
    let synced = step(engine, pools, root, "      type: core.calendar.sync").await;
    assert_eq!(synced["conflicts"], 1, "{synced}");
    let again = step(engine, pools, root, "      type: core.calendar.sync").await;
    assert_eq!(again["conflicts"], 0, "reported once: {again}");
    let event: Json = sqlx::query_scalar(
        "SELECT payload FROM events_outbox WHERE event_type = 'calendar.conflict'",
    )
    .fetch_one(&pools.owner)
    .await
    .unwrap();
    assert_eq!(
        (&event["userId"], &event["busyStatus"]),
        (&json!(ada), &json!("busy"))
    );
    // The meeting is busy time; the written appointment isn't counted twice.
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    let busy = calendar::between(&mut tx, &[ada], tomorrow(0), tomorrow(24))
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(busy.len(), 1, "{busy:?}");
    assert_eq!(busy[0].starts_at, tomorrow(15));
    assert_eq!(busy[0].ends_at, tomorrow(16));
}
