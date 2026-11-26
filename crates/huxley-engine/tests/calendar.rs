//! Calendar sync (ADR-061): `core.calendar.sync` reads busy time from
//! Microsoft 365 (the `msgraph` integration's getSchedule, against a mock
//! Graph on localhost) and from an iCalendar feed, stores it as busy
//! blocks, and availability then treats busy time as unavailable.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::extract::State;
use axum::http::HeaderMap;
use axum::routing::{get, post};
use huxley_core::{RequestContext, Via};
use huxley_crypto::Crypto;
use huxley_db::testing::{ensure_root, isolated};
use huxley_db::{OrgScope, begin_scoped};
use huxley_engine::{EgressConfig, Engine, EngineConfig, RunStatus, StartRun};
use huxley_integrations::connections::{self, ConnectionInput};
use huxley_integrations::store;
use huxley_workforce::calendar;
use serde_json::{Map, Value as Json, json};
use uuid::Uuid;

const MSGRAPH: &str = include_str!("../../huxley-packs/builtin/msgraph/integrations/msgraph.yaml");

/// What the mock Graph was asked: each getSchedule body and Prefer header.
#[derive(Default)]
struct Seen {
    bodies: Vec<Json>,
    prefer: Vec<String>,
}

type Shared = Arc<Mutex<Seen>>;

fn graph_at(at: i64) -> Json {
    let text = jiff::Timestamp::from_second(at)
        .unwrap()
        .strftime("%Y-%m-%dT%H:%M:%S.0000000")
        .to_string();
    json!({ "dateTime": text, "timeZone": "UTC" })
}

/// Tomorrow at an hour (UTC), unix seconds.
fn tomorrow(hour: i64) -> i64 {
    let now = jiff::Timestamp::now().as_second();
    (now / 86_400 + 1) * 86_400 + hour * 3600
}

async fn schedule(
    State(seen): State<Shared>,
    headers: HeaderMap,
    axum::Json(body): axum::Json<Json>,
) -> axum::Json<Json> {
    let mut s = seen.lock().unwrap();
    s.prefer.push(
        headers
            .get("prefer")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_owned(),
    );
    s.bodies.push(body.clone());
    let items = |id: &str| -> Json {
        match id {
            "ada@contoso.example" => json!({ "scheduleId": id, "scheduleItems": [
                { "status": "busy", "subject": "Board meeting", "start": graph_at(tomorrow(15)), "end": graph_at(tomorrow(16)) },
                { "status": "free", "start": graph_at(tomorrow(17)), "end": graph_at(tomorrow(18)) },
                { "status": "oof", "isPrivate": true, "start": graph_at(tomorrow(24)), "end": graph_at(tomorrow(48)) }
            ]}),
            other => {
                json!({ "scheduleId": other, "error": { "message": "The mailbox couldn't be found", "responseCode": "ErrorMailRecipientNotFound" } })
            }
        }
    };
    let value: Vec<Json> = body["schedules"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| items(s.as_str().unwrap()))
        .collect();
    axum::Json(json!({ "value": value }))
}

fn feed() -> String {
    let start = jiff::Timestamp::from_second(tomorrow(9))
        .unwrap()
        .strftime("%Y%m%dT%H%M%SZ")
        .to_string();
    let end = jiff::Timestamp::from_second(tomorrow(10))
        .unwrap()
        .strftime("%Y%m%dT%H%M%SZ")
        .to_string();
    format!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:x\r\nDTSTART:{start}\r\nDTEND:{end}\r\nSUMMARY:Dentist\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n"
    )
}

async fn mock(seen: Shared) -> u16 {
    let app = axum::Router::new()
        .route(
            "/token",
            post(|| async {
                axum::Json(json!({ "access_token": "graph-token", "expires_in": 3600 }))
            }),
        )
        .route("/v1.0/users/{user}/calendar/getSchedule", post(schedule))
        .route(
            "/feed.ics",
            get(|| async { ([("content-type", "text/calendar")], feed()) }),
        )
        .with_state(seen);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    port
}

/// The msgraph integration at `root` pointed at the mock, and a connection.
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

async fn sync(engine: &Engine, pools: &huxley_db::testing::TestPools, org: Uuid) -> Json {
    let yaml = "apiVersion: usehuxley.io/v1\nkind: Workflow\nmetadata: { key: t, name: T }\nspec:\n  steps:\n    - id: sync\n      type: core.calendar.sync\n  outputs:\n    result: \"{{ steps.sync.output }}\"\n";
    let document = serde_json::to_value(huxley_hwf::parse_yaml(yaml).unwrap()).unwrap();
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
        engine.drain("calendar", 50).await.unwrap();
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

#[tokio::test]
async fn busy_time_is_read_from_microsoft_365_and_feeds() {
    let Some(db) = isolated("calendar").await else {
        return;
    };
    let pools = db.pools.clone();
    let root = ensure_root(&pools).await;
    let ctx = RequestContext::system(Via::System);
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    huxley_modules::set_enabled(&mut tx, &ctx, "workforce", true, false)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let seen: Shared = Arc::default();
    let port = mock(seen.clone()).await;
    let crypto = Crypto::new(pools.app.clone(), huxley_crypto::testing::key_ring());
    graph(&pools, &crypto, root, port).await;

    let ada = person(&pools, "ada@contoso.example").await;
    let bob = person(&pools, "bob@contoso.example").await;
    let carol = person(&pools, "carol@contoso.example").await;
    let sealed = crypto
        .encrypt(
            root,
            calendar::ics_binding(carol),
            format!("http://localhost:{port}/feed.ics").as_bytes(),
        )
        .await
        .unwrap();
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    calendar::set_settings(
        &mut tx,
        &ctx,
        &calendar::SettingsInput {
            provider: "m365".into(),
            org_id: root,
            connection: "contoso".into(),
            keep_subjects: false,
        },
    )
    .await
    .unwrap();
    let ics = calendar::CalendarPersonInput {
        source: "ics".into(),
        calendar: None,
        ics_url: None,
    };
    calendar::set_person(&mut tx, &ctx, carol, &ics, Some(sealed))
        .await
        .unwrap();
    tx.commit().await.unwrap();

    let config = EngineConfig {
        egress: EgressConfig {
            private_hosts: vec!["localhost".into()],
            ..EgressConfig::default()
        },
        ..EngineConfig::default()
    };
    let engine = Engine::with_config(pools.app.clone(), crypto, &config);
    let result = sync(&engine, &pools, root).await;
    assert_eq!(result["people"], 3, "{result}");
    assert_eq!(result["failed"][0]["user_id"], json!(bob), "{result}");
    assert!(
        result["failed"][0]["error"]
            .as_str()
            .unwrap()
            .contains("couldn't be found")
    );

    // One call for the Microsoft 365 people, in UTC, with a 60-day window.
    {
        let s = seen.lock().unwrap();
        assert_eq!(s.bodies.len(), 1);
        let mut asked: Vec<&str> = s.bodies[0]["schedules"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        asked.sort_unstable();
        assert_eq!(asked, ["ada@contoso.example", "bob@contoso.example"]);
        assert_eq!(s.prefer[0], "outlook.timezone=\"UTC\"");
    }

    let blocks: Vec<(Uuid, String, Option<String>)> =
        sqlx::query_as("SELECT user_id, status, subject FROM wf_busy_blocks ORDER BY starts_at")
            .fetch_all(&pools.owner)
            .await
            .unwrap();
    // Carol's feed at 9, Ada's meeting at 3 (no subject: they aren't kept),
    // then Ada away the next day. Free time isn't a block.
    assert_eq!(
        blocks,
        vec![
            (carol, "busy".into(), None),
            (ada, "busy".into(), None),
            (ada, "oof".into(), None)
        ]
    );
    let errors: Vec<(Uuid, Option<String>)> = sqlx::query_as(
        "SELECT user_id, last_error FROM wf_calendar_people WHERE last_error IS NOT NULL",
    )
    .fetch_all(&pools.owner)
    .await
    .unwrap();
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].0, bob);

    availability_and_resync(&engine, &pools, root, ada, carol).await;
}

/// Availability follows the blocks; a second sync replaces the window.
async fn availability_and_resync(
    engine: &Engine,
    pools: &huxley_db::testing::TestPools,
    root: Uuid,
    ada: Uuid,
    carol: Uuid,
) {
    // Availability: Ada's busy during her meeting, away the next day.
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    let during =
        huxley_workforce::availability::unavailable(&mut tx, &[ada, carol], tomorrow(15) + 600)
            .await
            .unwrap();
    let away = huxley_workforce::availability::unavailable(&mut tx, &[ada], tomorrow(30))
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(during.get(&ada).copied(), Some("busy"));
    assert_eq!(during.get(&carol), None);
    assert_eq!(away.get(&ada).copied(), Some("away"));

    // A second sync replaces the window rather than adding to it.
    sync(engine, pools, root).await;
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM wf_busy_blocks")
        .fetch_one(&pools.owner)
        .await
        .unwrap();
    assert_eq!(count, 3);
}
