//! Webhook triggers over HTTP, against a real PostgreSQL, with a worker
//! draining the queue in the background.

use std::time::Duration;

use axum::body::Body;
use http::{Request, StatusCode};
use http_body_util::BodyExt as _;
use huxley_api::{ApiConfig, AppState, router};
use huxley_auth::{AuthConfig, AuthService};
use huxley_core::{RequestContext, Via};
use huxley_crypto::Crypto;
use huxley_db::testing::{TestPools, ensure_root, pools, unique};
use huxley_db::{OrgScope, begin_scoped};
use ring::hmac;
use serde_json::{Value, json};
use tower::ServiceExt as _;
use uuid::Uuid;

fn crypto(pools: &TestPools) -> Crypto {
    Crypto::new(pools.app.clone(), huxley_crypto::testing::key_ring())
}

fn app(pools: &TestPools) -> axum::Router {
    let auth = AuthService::new(pools.app.clone(), AuthConfig::default(), crypto(pools));
    router(AppState::new(
        pools.app.clone(),
        auth,
        crypto(pools),
        ApiConfig::default(),
    ))
}

/// Publishes a workflow with one webhook trigger; returns its public id.
async fn webhook(pools: &TestPools, trigger: Value) -> (Uuid, String) {
    let root = ensure_root(pools).await;
    let ctx = RequestContext::system(Via::System);
    let expr = huxley_expr::Engine::new();
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    let org = huxley_orgs::create(
        &mut tx,
        &ctx,
        root,
        huxley_orgs::OrgKind::Provider,
        &unique("Hook"),
    )
    .await
    .unwrap();
    let key = format!("hook-{}", &Uuid::now_v7().simple().to_string()[20..]);
    let document = json!({
        "apiVersion": "usehuxley.io/v1",
        "kind": "Workflow",
        "metadata": { "key": key, "name": "Hook" },
        "spec": {
            "inputs": { "type": "object", "properties": { "n": { "type": "integer" } } },
            "triggers": [trigger],
            "steps": [{ "id": "double", "type": "transform", "output": "{{ (inputs.n or 0) * 2 }}" }],
            "outputs": {
                "doubled": "{{ steps.double.output }}",
                "event": "{{ trigger.headers['x-event'] }}",
                "secret_seen": "{{ trigger.headers.authorization is defined }}"
            }
        }
    });
    tx.commit().await.unwrap();
    // The HMAC secret lives in a secret organization variable.
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    let secret = huxley_variables::VariableInput {
        kind: huxley_variables::VariableType::Secret,
        value: json!("hook-secret"),
        description: None,
        locked: false,
    };
    huxley_variables::set(
        &mut tx,
        &ctx,
        &crypto(pools),
        org.id,
        "hook_secret",
        &secret,
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    let created = huxley_workflows::create(&mut tx, &ctx, &expr, org.id, document)
        .await
        .unwrap();
    let id = created.summary.id;
    huxley_workflows::publish(&mut tx, &ctx, &expr, id, None)
        .await
        .unwrap();
    let triggers = huxley_workflows::triggers::list(&mut tx, id).await.unwrap();
    tx.commit().await.unwrap();
    (id, triggers[0].public_id.clone().unwrap())
}

async fn post(
    app: &axum::Router,
    uri: &str,
    headers: &[(&str, String)],
    body: &str,
) -> (StatusCode, Value) {
    let mut req = Request::post(uri).header("content-type", "application/json");
    for (name, value) in headers {
        req = req.header(*name, value);
    }
    let res = app
        .clone()
        .oneshot(req.body(Body::from(body.to_owned())).unwrap())
        .await
        .unwrap();
    let status = res.status();
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

fn sign(secret: &str, body: &str) -> String {
    let key = hmac::Key::new(hmac::HMAC_SHA256, secret.as_bytes());
    let tag = hmac::sign(&key, body.as_bytes());
    format!(
        "sha256={}",
        tag.as_ref().iter().fold(String::new(), |mut s, b| {
            use std::fmt::Write as _;
            let _ = write!(s, "{b:02x}");
            s
        })
    )
}

/// Drains the job queue until dropped.
fn worker(pools: &TestPools) -> tokio::task::JoinHandle<()> {
    let engine = huxley_engine::Engine::new(pools.app.clone(), crypto(pools));
    tokio::spawn(async move {
        loop {
            let _ = engine.drain("hook-test", 20).await;
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
}

#[tokio::test]
async fn signed_sync_webhooks_return_outputs() {
    let Some(pools) = pools().await else { return };
    let app = app(&pools);
    let (_, public_id) = webhook(
        &pools,
        json!({
            "type": "webhook",
            "response": "sync",
            "timeout": "15s",
            "auth": { "type": "hmac", "secret": "{{ secrets.hook_secret }}", "prefix": "sha256=" }
        }),
    )
    .await;
    let uri = format!("/hooks/{public_id}");
    let worker = worker(&pools);

    let body = r#"{"n":21}"#;
    let (status, outputs) = post(
        &app,
        &uri,
        &[
            ("x-signature", sign("hook-secret", body)),
            ("x-event", "ping".into()),
        ],
        body,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{outputs}");
    assert_eq!(
        outputs,
        json!({ "doubled": 42, "event": "ping", "secret_seen": false })
    );

    // Unsigned, wrongly signed, tampered: refused before any run starts.
    assert_eq!(
        post(&app, &uri, &[], body).await.0,
        StatusCode::UNAUTHORIZED
    );
    let wrong = [("x-signature", sign("other", body))];
    assert_eq!(
        post(&app, &uri, &wrong, body).await.0,
        StatusCode::UNAUTHORIZED
    );
    let signed = [("x-signature", sign("hook-secret", body))];
    assert_eq!(
        post(&app, &uri, &signed, r#"{"n":22}"#).await.0,
        StatusCode::UNAUTHORIZED
    );

    // Signed but not valid for the input schema.
    let bad = r#"{"n":"x"}"#;
    let (status, _) = post(
        &app,
        &uri,
        &[("x-signature", sign("hook-secret", bad))],
        bad,
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    worker.abort();
}

#[tokio::test]
async fn async_webhooks_answer_at_once_and_unknown_ones_404() {
    let Some(pools) = pools().await else { return };
    let app = app(&pools);
    let (workflow, public_id) = webhook(
        &pools,
        json!({ "type": "webhook", "auth": { "type": "bearer", "token": "t0ken" } }),
    )
    .await;
    let uri = format!("/hooks/{public_id}");
    let auth = [("authorization", "Bearer t0ken".to_owned())];
    let (status, accepted) = post(&app, &uri, &auth, r#"{"n":1}"#).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{accepted}");
    let run: Uuid = accepted["runId"].as_str().unwrap().parse().unwrap();
    let (trigger_type, trigger): (String, Value) =
        sqlx::query_as("SELECT trigger_type, trigger FROM runs WHERE id = $1")
            .bind(run)
            .fetch_one(&pools.owner)
            .await
            .unwrap();
    assert_eq!(trigger_type, "webhook");
    assert!(
        trigger["headers"].get("authorization").is_none(),
        "credentials aren't kept"
    );

    assert_eq!(
        post(&app, "/hooks/wh_nope", &auth, "{}").await.0,
        StatusCode::NOT_FOUND
    );
    // Archived workflows stop answering.
    let ctx = RequestContext::system(Via::System);
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    huxley_workflows::set_archived(&mut tx, &ctx, workflow, true)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(post(&app, &uri, &auth, "{}").await.0, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn webhook_runs_act_as_the_run_as_account() {
    let Some(pools) = pools().await else { return };
    let app = app(&pools);
    let (workflow, public_id) = webhook(
        &pools,
        json!({ "type": "webhook", "auth": { "type": "bearer", "token": "t0ken" } }),
    )
    .await;
    let ctx = RequestContext::system(Via::System);
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    let account =
        huxley_auth::api_tokens::create_service_account(&mut tx, &ctx, &unique("Bot"), "")
            .await
            .unwrap();
    huxley_workflows::set_run_as(&mut tx, &ctx, workflow, Some(account.id))
        .await
        .unwrap();
    tx.commit().await.unwrap();

    let uri = format!("/hooks/{public_id}");
    let auth = [("authorization", "Bearer t0ken".to_owned())];
    let (status, accepted) = post(&app, &uri, &auth, r#"{"n":1}"#).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{accepted}");
    let run: Uuid = accepted["runId"].as_str().unwrap().parse().unwrap();
    let actor: Option<Uuid> = sqlx::query_scalar("SELECT actor_id FROM runs WHERE id = $1")
        .bind(run)
        .fetch_one(&pools.owner)
        .await
        .unwrap();
    assert_eq!(actor, Some(account.id));

    // A disabled account starts nothing.
    sqlx::query("UPDATE users SET status = 'disabled' WHERE id = $1")
        .bind(account.id)
        .execute(&pools.owner)
        .await
        .unwrap();
    let (status, problem) = post(&app, &uri, &auth, r#"{"n":1}"#).await;
    assert_eq!(status, StatusCode::CONFLICT, "{problem}");
    assert_eq!(
        problem["type"],
        "https://usehuxley.io/problems/run-as-unavailable"
    );
    let runs: i64 = sqlx::query_scalar("SELECT count(*) FROM runs WHERE workflow_id = $1")
        .bind(workflow)
        .fetch_one(&pools.owner)
        .await
        .unwrap();
    assert_eq!(runs, 1);
}

#[tokio::test]
async fn unknown_callbacks_are_not_found() {
    let Some(pools) = pools().await else { return };
    let app = app(&pools);
    let (status, _) = post(
        &app,
        "/callbacks/not-a-real-token",
        &[],
        r#"{"status":"done"}"#,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn signed_deliveries_are_accepted_once() {
    let Some(pools) = pools().await else { return };
    let app = app(&pools);
    let (_, public_id) = webhook(
        &pools,
        json!({
            "type": "webhook",
            "response": "async",
            "auth": {
                "type": "hmac",
                "secret": "{{ secrets.hook_secret }}",
                "prefix": "sha256=",
                "timestamp_header": "x-timestamp"
            }
        }),
    )
    .await;
    let uri = format!("/hooks/{public_id}");
    let body = r#"{"event":"ping","n":21}"#;
    let at = time::OffsetDateTime::now_utc().unix_timestamp();
    let signed = [
        ("x-timestamp", at.to_string()),
        ("x-signature", sign("hook-secret", &format!("{at}.{body}"))),
    ];
    let (status, first) = post(&app, &uri, &signed, body).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{first}");

    // The same signed request again starts nothing.
    let (status, again) = post(&app, &uri, &signed, body).await;
    assert_eq!(status, StatusCode::OK, "{again}");
    assert_eq!(again, json!({ "duplicate": true }));
    let runs: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM runs WHERE trigger->'headers'->>'x-timestamp' = $1",
    )
    .bind(at.to_string())
    .fetch_one(&pools.owner)
    .await
    .unwrap();
    assert_eq!(runs, 1);

    // A new delivery (another timestamp) is accepted.
    let next = at + 1;
    let fresh = [
        ("x-timestamp", next.to_string()),
        (
            "x-signature",
            sign("hook-secret", &format!("{next}.{body}")),
        ),
    ];
    assert_eq!(post(&app, &uri, &fresh, body).await.0, StatusCode::ACCEPTED);

    // A delivery that isn't accepted (invalid JSON) is forgotten: sent
    // again, it fails the same way rather than being called a duplicate.
    let bad = "{";
    let failing = [
        ("x-timestamp", at.to_string()),
        ("x-signature", sign("hook-secret", &format!("{at}.{bad}"))),
    ];
    assert_eq!(
        post(&app, &uri, &failing, bad).await.0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        post(&app, &uri, &failing, bad).await.0,
        StatusCode::BAD_REQUEST
    );
}
