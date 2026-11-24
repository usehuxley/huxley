//! Datasets over HTTP (ADR-029), against a real PostgreSQL.

use axum::body::Body;
use http::{Request, StatusCode};
use http_body_util::BodyExt as _;
use huxley_api::{ApiConfig, AppState, router};
use huxley_auth::{AuthConfig, AuthService};
use huxley_core::{RequestContext, Via};
use huxley_crypto::Crypto;
use huxley_db::testing::{TestPools, ensure_root, pools, unique};
use huxley_db::{OrgScope, begin_scoped};
use serde_json::{Value, json};
use tower::ServiceExt as _;
use uuid::Uuid;

const PASSWORD: &str = "dataset api passphrase";

fn crypto(pools: &TestPools) -> Crypto {
    Crypto::new(pools.app.clone(), huxley_crypto::testing::key_ring())
}

struct Setup {
    pools: TestPools,
    app: axum::Router,
    org: Uuid,
    owner: String,
}

async fn setup() -> Option<Setup> {
    let pools = pools().await?;
    let auth = AuthService::new(pools.app.clone(), AuthConfig::default(), crypto(&pools));
    let config = ApiConfig {
        login_attempts_per_minute: 1000,
        ..ApiConfig::default()
    };
    let state = AppState::new(pools.app.clone(), auth, crypto(&pools), config);
    let app = router(state);
    let root = ensure_root(&pools).await;
    let ctx = RequestContext::system(Via::System);
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    let org = huxley_orgs::create(
        &mut tx,
        &ctx,
        root,
        huxley_orgs::OrgKind::Provider,
        &unique("Pk"),
    )
    .await
    .unwrap()
    .id;
    tx.commit().await.unwrap();
    let owner = login(&app, &user(&pools, true).await).await;
    Some(Setup {
        pools,
        app,
        org,
        owner,
    })
}

async fn user(pools: &TestPools, owner: bool) -> String {
    user_with_id(pools, owner).await.1
}

async fn user_with_id(pools: &TestPools, owner: bool) -> (Uuid, String) {
    let email = format!("{}@example.com", unique("pk").replace(' ', "."));
    let ctx = RequestContext::system(Via::System);
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    let u = huxley_auth::create_user(&mut tx, &ctx, &email, "Pack User", PASSWORD)
        .await
        .unwrap();
    if owner {
        huxley_authz::grant_root_owner(&mut tx, &ctx, u.id)
            .await
            .unwrap();
    }
    tx.commit().await.unwrap();
    (u.id, email)
}

async fn login(app: &axum::Router, email: &str) -> String {
    let req = Request::post("/api/v1/auth/login")
        .header("x-requested-with", "huxley")
        .header("content-type", "application/json")
        .body(Body::from(
            json!({ "email": email, "password": PASSWORD }).to_string(),
        ))
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let cookie = res.headers()["set-cookie"].to_str().unwrap();
    cookie.split(';').next().unwrap().to_owned()
}

impl Setup {
    async fn call(
        &self,
        cookie: &str,
        method: &str,
        uri: &str,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        let req = Request::builder()
            .method(method)
            .uri(uri)
            .header("x-requested-with", "huxley")
            .header("cookie", cookie);
        let req = match body {
            Some(b) => req
                .header("content-type", "application/json")
                .body(Body::from(b.to_string())),
            None => req.body(Body::empty()),
        }
        .unwrap();
        let res = self.app.clone().oneshot(req).await.unwrap();
        let status = res.status();
        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }
}

#[tokio::test]
async fn datasets_over_http() {
    let Some(s) = setup().await else { return };
    let definition = json!({
        "key": "m365-users", "name": "M365 users",
        "columns": [{ "name": "upn", "type": "text" }, { "name": "licensed", "type": "boolean" }],
        "primaryKey": ["upn"]
    });
    let (status, dataset) = s
        .call(
            &s.owner,
            "POST",
            "/api/v1/datasets",
            Some(json!({ "orgId": s.org, "dataset": definition })),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{dataset}");
    let id = dataset["id"].as_str().unwrap().to_owned();
    let (status, _) = s
        .call(
            &s.owner,
            "POST",
            "/api/v1/datasets",
            Some(json!({ "orgId": s.org, "dataset": definition })),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT);

    let rows = json!({ "orgId": s.org, "rows": [
        { "upn": "ada@x.test", "licensed": true }, { "upn": "bob@x.test", "licensed": false }
    ] });
    let (status, written) = s
        .call(
            &s.owner,
            "POST",
            &format!("/api/v1/datasets/{id}/rows"),
            Some(rows),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{written}");
    assert_eq!(written["written"], 2);
    let (status, bad) = s
        .call(
            &s.owner,
            "POST",
            &format!("/api/v1/datasets/{id}/rows"),
            Some(json!({ "orgId": s.org, "rows": [{ "upn": "c", "licensed": "maybe" }] })),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{bad}");

    let (status, found) = s
        .call(
            &s.owner,
            "POST",
            &format!("/api/v1/datasets/{id}/rows/query"),
            Some(json!({ "filters": { "licensed": true } })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{found}");
    assert_eq!(found["total"], 1);
    assert_eq!(found["rows"][0]["upn"], "ada@x.test");

    // Changing the definition migrates the table.
    let mut changed = definition.clone();
    changed["columns"]
        .as_array_mut()
        .unwrap()
        .push(json!({ "name": "department", "type": "text" }));
    let (status, updated) = s
        .call(
            &s.owner,
            "PUT",
            &format!("/api/v1/datasets/{id}"),
            Some(json!({ "dataset": changed })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{updated}");
    assert_eq!(updated["columns"].as_array().unwrap().len(), 3);

    // Someone without access sees nothing.
    let stranger = login(&s.app, &user(&s.pools, false).await).await;
    let (status, _) = s
        .call(&stranger, "GET", &format!("/api/v1/datasets/{id}"), None)
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (_, list) = s.call(&stranger, "GET", "/api/v1/datasets", None).await;
    assert_eq!(list, json!([]));

    let (status, deleted) = s
        .call(
            &s.owner,
            "POST",
            &format!("/api/v1/datasets/{id}/rows/delete"),
            Some(json!({ "filters": { "upn": "bob@x.test" } })),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(deleted["written"], 1);
    let (status, _) = s
        .call(&s.owner, "DELETE", &format!("/api/v1/datasets/{id}"), None)
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = s
        .call(&s.owner, "GET", &format!("/api/v1/datasets/{id}"), None)
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn syncs_are_checked_scheduled_and_started() {
    let Some(s) = setup().await else { return };
    let definition = json!({
        "key": "users", "name": "Users",
        "columns": [{ "name": "upn", "type": "text" }], "primaryKey": ["upn"]
    });
    let (_, dataset) = s
        .call(
            &s.owner,
            "POST",
            "/api/v1/datasets",
            Some(json!({ "orgId": s.org, "dataset": definition })),
        )
        .await;
    let id = dataset["id"].as_str().unwrap().to_owned();
    let uri = format!("/api/v1/datasets/{id}/sync");

    // Nothing to run yet.
    let (status, _) = s.call(&s.owner, "POST", &format!("{uri}/run"), None).await;
    assert_eq!(status, StatusCode::CONFLICT);

    // Settings are checked against the dataset.
    for (sync, field) in [
        (
            json!({ "action": "nope", "map": { "upn": "userPrincipalName" } }),
            "sync.action",
        ),
        (
            json!({ "action": "msgraph/listUsers", "map": { "other": "x" } }),
            "sync.map.other",
        ),
        (
            json!({ "action": "msgraph/listUsers", "map": { "upn": "userPrincipalName" },
                 "schedule": { "cron": "not cron" } }),
            "sync.schedule",
        ),
    ] {
        let (status, problem) = s
            .call(&s.owner, "PUT", &uri, Some(json!({ "sync": sync })))
            .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{problem}");
        assert!(problem.to_string().contains(field), "{field}: {problem}");
    }

    let sync = json!({ "action": "msgraph/listUsers", "map": { "upn": "userPrincipalName" },
                       "schedule": { "interval": "6h" } });
    let (status, state) = s
        .call(&s.owner, "PUT", &uri, Some(json!({ "sync": sync })))
        .await;
    assert_eq!(status, StatusCode::OK, "{state}");
    assert_eq!(state["sync"]["mode"], "replace");
    assert!(state["nextAt"].is_string(), "{state}");

    // Syncing now starts a run in its organization.
    let (status, started) = s.call(&s.owner, "POST", &format!("{uri}/run"), None).await;
    assert_eq!(status, StatusCode::CREATED, "{started}");
    let run = started["runs"][0].as_str().unwrap();
    let (_, detail) = s
        .call(&s.owner, "GET", &format!("/api/v1/runs/{run}"), None)
        .await;
    assert_eq!(detail["triggerType"], "dataset_sync");
    assert_eq!(detail["orgId"], json!(s.org));

    // Stopping it clears the schedule.
    let (_, state) = s
        .call(&s.owner, "PUT", &uri, Some(json!({ "sync": null })))
        .await;
    assert_eq!(state["sync"], json!(null));
    assert_eq!(state["nextAt"], json!(null));
}
