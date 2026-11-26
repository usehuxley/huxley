//! The `cmdb` module's API against a real PostgreSQL (design doc §8.5):
//! who may see and change assets and types, relationships and impact.

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

const PASSWORD: &str = "cmdb api passphrase";

struct Setup {
    pools: TestPools,
    app: axum::Router,
    provider: Uuid,
    owner: String,
}

async fn org(pools: &TestPools, parent: Uuid, kind: huxley_orgs::OrgKind, name: &str) -> Uuid {
    let ctx = RequestContext::system(Via::System);
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    let id = huxley_orgs::create(&mut tx, &ctx, parent, kind, &unique(name))
        .await
        .unwrap()
        .id;
    tx.commit().await.unwrap();
    id
}

async fn setup() -> Option<Setup> {
    let pools = pools().await?;
    let crypto = || Crypto::new(pools.app.clone(), huxley_crypto::testing::key_ring());
    let auth = AuthService::new(pools.app.clone(), AuthConfig::default(), crypto());
    let config = ApiConfig {
        login_attempts_per_minute: 1000,
        ..ApiConfig::default()
    };
    let app = router(AppState::new(pools.app.clone(), auth, crypto(), config));
    let root = ensure_root(&pools).await;
    // The module is instance-wide: switch it on (other tests may have too).
    let ctx = RequestContext::system(Via::System);
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    for module in ["crm", "servicedesk", "cmdb"] {
        huxley_modules::set_enabled(&mut tx, &ctx, module, true, false)
            .await
            .unwrap();
    }
    tx.commit().await.unwrap();
    let provider = org(&pools, root, huxley_orgs::OrgKind::Provider, "CMDB MSP").await;
    let owner = login(&app, &user(&pools, None).await.1).await;
    Some(Setup {
        pools,
        app,
        provider,
        owner,
    })
}

/// A user, optionally with a built-in role at an organization.
async fn user(pools: &TestPools, role_at: Option<(&str, Uuid)>) -> (Uuid, String) {
    let email = format!("{}@example.com", unique("cmdb").replace(' ', "."));
    let ctx = RequestContext::system(Via::System);
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    let u = huxley_auth::create_user(&mut tx, &ctx, &email, "Desk User", PASSWORD)
        .await
        .unwrap();
    match role_at {
        None => huxley_authz::grant_root_owner(&mut tx, &ctx, u.id)
            .await
            .unwrap(),
        Some((role, org)) => {
            sqlx::query(
                "INSERT INTO role_assignments (id, user_id, role_id, org_id) \
                 SELECT $1, $2, id, $3 FROM roles WHERE key = $4 AND is_system",
            )
            .bind(Uuid::now_v7())
            .bind(u.id)
            .bind(org)
            .bind(role)
            .execute(&mut *tx)
            .await
            .unwrap();
        }
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

    async fn ok(&self, cookie: &str, method: &str, uri: &str, body: Option<Value>) -> Value {
        let (status, value) = self.call(cookie, method, uri, body).await;
        assert!(status.is_success(), "{method} {uri}: {status} {value}");
        value
    }
}

struct People {
    tech: String,
    reader: String,
    outsider: String,
}

async fn people(s: &Setup) -> People {
    let client = org(
        &s.pools,
        s.provider,
        huxley_orgs::OrgKind::Client,
        "CMDB client",
    )
    .await;
    People {
        tech: login(
            &s.app,
            &user(&s.pools, Some(("technician", s.provider))).await.1,
        )
        .await,
        reader: login(
            &s.app,
            &user(&s.pools, Some(("read_only", s.provider))).await.1,
        )
        .await,
        outsider: login(
            &s.app,
            &user(&s.pools, Some(("technician", client))).await.1,
        )
        .await,
    }
}

fn field(problem: &Value) -> String {
    problem["errors"][0]["pointer"]
        .as_str()
        .unwrap_or_default()
        .to_owned()
}

async fn type_id(s: &Setup, cookie: &str, key: &str) -> String {
    let types = s.ok(cookie, "GET", "/api/v1/asset-types", None).await;
    types
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["key"] == key)
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned()
}

#[tokio::test]
async fn technicians_keep_assets_readers_read_and_others_nothing() {
    let Some(s) = setup().await else { return };
    let p = people(&s).await;
    let server = type_id(&s, &p.tech, "server").await;
    let body = json!({ "orgId": s.provider, "typeId": server, "name": "DC01", "attributes": { "memory_gb": "lots" } });
    let (status, problem) = s.call(&p.tech, "POST", "/api/v1/assets", Some(body)).await;
    assert_eq!(
        (status, field(&problem).as_str()),
        (StatusCode::UNPROCESSABLE_ENTITY, "/attributes/memory_gb")
    );
    let body = json!({ "orgId": s.provider, "typeId": server, "name": "DC01", "serialNumber": "SN-9", "attributes": { "memory_gb": 32 } });
    let a = s
        .ok(&p.tech, "POST", "/api/v1/assets", Some(body.clone()))
        .await;
    let id = a["id"].as_str().unwrap().to_owned();
    assert_eq!(
        (a["canWrite"].as_bool(), a["typeKey"].as_str()),
        (Some(true), Some("server"))
    );

    // Readers see it, can't change it.
    let seen = s
        .ok(&p.reader, "GET", &format!("/api/v1/assets/{id}"), None)
        .await;
    assert_eq!(seen["canWrite"], false);
    let mut change = body.clone();
    change["version"] = a["version"].clone();
    change["status"] = json!("retired");
    let (status, _) = s
        .call(
            &p.reader,
            "PUT",
            &format!("/api/v1/assets/{id}"),
            Some(change.clone()),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let a = s
        .ok(
            &p.tech,
            "PUT",
            &format!("/api/v1/assets/{id}"),
            Some(change),
        )
        .await;
    assert_eq!(a["status"], "retired");
    let listed = s.ok(&p.reader, "GET", "/api/v1/assets?q=SN-9", None).await;
    assert!(
        listed["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|i| i["id"] == id.as_str())
    );
    let in_use = s
        .ok(&p.reader, "GET", "/api/v1/assets?q=SN-9&inUse=true", None)
        .await;
    assert!(
        in_use["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|i| i["id"] != id.as_str())
    );

    // Another client's technician can't see it.
    let (status, _) = s
        .call(&p.outsider, "GET", &format!("/api/v1/assets/{id}"), None)
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Types are kept by those who may configure the CMDB.
    let t = json!({ "name": unique("Door controller"), "fields": [{ "key": "zone", "label": "Zone", "fieldType": "text" }] });
    let (status, _) = s
        .call(&p.tech, "POST", "/api/v1/asset-types", Some(t.clone()))
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    s.ok(&s.owner, "POST", "/api/v1/asset-types", Some(t)).await;
}

#[tokio::test]
async fn relationships_drive_impact() {
    let Some(s) = setup().await else { return };
    let p = people(&s).await;
    let server = type_id(&s, &p.tech, "server").await;
    let mut ids = Vec::new();
    for name in ["Host", "VM", "App"] {
        let a = s
            .ok(
                &p.tech,
                "POST",
                "/api/v1/assets",
                Some(json!({ "orgId": s.provider, "typeId": server, "name": name })),
            )
            .await;
        ids.push(a["id"].as_str().unwrap().to_owned());
    }
    let (host, vm, app) = (&ids[0], &ids[1], &ids[2]);
    s.ok(
        &p.tech,
        "POST",
        &format!("/api/v1/assets/{host}/relationships"),
        Some(json!({ "kind": "hosts", "toId": vm })),
    )
    .await;
    s.ok(
        &p.tech,
        "POST",
        &format!("/api/v1/assets/{app}/relationships"),
        Some(json!({ "kind": "runs_on", "toId": vm })),
    )
    .await;
    let (status, _) = s
        .call(
            &p.reader,
            "POST",
            &format!("/api/v1/assets/{app}/relationships"),
            Some(json!({ "kind": "depends_on", "toId": host })),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let impact = s
        .ok(
            &p.reader,
            "GET",
            &format!("/api/v1/assets/{host}/impact"),
            None,
        )
        .await;
    let names: Vec<&str> = impact
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["VM", "App"]);
    let vm_view = s
        .ok(&p.tech, "GET", &format!("/api/v1/assets/{vm}"), None)
        .await;
    assert_eq!(vm_view["relationships"].as_array().unwrap().len(), 2);
    let edge = vm_view["relationships"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["kind"] == "runs_on")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let (status, _) = s
        .call(
            &p.tech,
            "DELETE",
            &format!("/api/v1/asset-relationships/{edge}"),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let impact = s
        .ok(
            &p.tech,
            "GET",
            &format!("/api/v1/assets/{host}/impact"),
            None,
        )
        .await;
    assert_eq!(impact.as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn assets_are_linked_to_tickets_and_mapped() {
    let Some(s) = setup().await else { return };
    let p = people(&s).await;
    let server = type_id(&s, &p.tech, "server").await;
    let mut ids = Vec::new();
    for name in ["Host", "VM"] {
        let a = s
            .ok(
                &p.tech,
                "POST",
                "/api/v1/assets",
                Some(json!({ "orgId": s.provider, "typeId": server, "name": name })),
            )
            .await;
        ids.push(a["id"].as_str().unwrap().to_owned());
    }
    s.ok(
        &p.tech,
        "POST",
        &format!("/api/v1/assets/{}/relationships", ids[0]),
        Some(json!({ "kind": "hosts", "toId": ids[1] })),
    )
    .await;
    let map = s
        .ok(
            &p.reader,
            "GET",
            &format!("/api/v1/assets/{}/map", ids[1]),
            None,
        )
        .await;
    assert_eq!(
        (
            map["nodes"].as_array().unwrap().len(),
            map["edges"].as_array().unwrap().len()
        ),
        (2, 1)
    );

    let ticket = s
        .ok(
            &p.tech,
            "POST",
            "/api/v1/tickets",
            Some(json!({ "orgId": s.provider, "title": "Host is down" })),
        )
        .await;
    let t = ticket["id"].as_str().unwrap().to_owned();
    let uri = format!("/api/v1/tickets/{t}/assets");
    let (status, _) = s
        .call(&p.reader, "POST", &uri, Some(json!({ "assetId": ids[0] })))
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    s.ok(&p.tech, "POST", &uri, Some(json!({ "assetId": ids[0] })))
        .await;
    let on = s.ok(&p.reader, "GET", &uri, None).await;
    assert_eq!(
        (on["linked"][0]["id"].as_str(), on["canLink"].as_bool()),
        (Some(ids[0].as_str()), Some(false))
    );
    let history = s
        .ok(
            &p.reader,
            "GET",
            &format!("/api/v1/assets/{}/tickets", ids[0]),
            None,
        )
        .await;
    assert_eq!(history[0]["id"], t.as_str());
    let (status, _) = s.call(&p.outsider, "GET", &uri, None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = s
        .call(&p.tech, "DELETE", &format!("{uri}/{}", ids[0]), None)
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn mappings_turn_dataset_rows_into_assets() {
    let Some(s) = setup().await else { return };
    let p = people(&s).await;
    let key = unique("rmm")
        .to_lowercase()
        .replace(|c: char| !c.is_ascii_alphanumeric(), "-");
    let dataset = s
        .ok(
            &s.owner,
            "POST",
            "/api/v1/datasets",
            Some(json!({ "orgId": s.provider, "dataset": {
                "key": key, "name": "RMM devices", "primaryKey": ["device_id"],
                "columns": [{ "name": "device_id", "type": "text" }, { "name": "hostname", "type": "text" }],
            } })),
        )
        .await;
    let ds = dataset["id"].as_str().unwrap().to_owned();
    s.ok(
        &s.owner,
        "POST",
        &format!("/api/v1/datasets/{ds}/rows"),
        Some(json!({ "orgId": s.provider, "rows": [{ "device_id": "d1", "hostname": "PC-ONE" }, { "device_id": "d2", "hostname": "PC-TWO" }] })),
    )
    .await;
    let ws = type_id(&s, &p.tech, "workstation").await;
    let rule = json!({
        "datasetId": ds, "name": "RMM", "source": "rmm", "typeId": ws,
        "matchKeys": [{ "field": "external_id", "column": "device_id" }],
        "fields": { "name": "hostname", "hostname": "hostname", "external_id": "device_id" },
    });
    // Only those who configure the CMDB keep mappings.
    let (status, _) = s
        .call(
            &p.tech,
            "POST",
            "/api/v1/asset-mappings",
            Some(rule.clone()),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let m = s
        .ok(&s.owner, "POST", "/api/v1/asset-mappings", Some(rule))
        .await;
    let id = m["id"].as_str().unwrap().to_owned();
    // A preview keeps nothing; a run makes the assets, once.
    let preview = s
        .ok(
            &s.owner,
            "POST",
            &format!("/api/v1/asset-mappings/{id}/run?preview=true"),
            None,
        )
        .await;
    assert_eq!(
        (preview["created"].as_u64(), preview["preview"].as_bool()),
        (Some(2), Some(true))
    );
    let found = s.ok(&p.tech, "GET", "/api/v1/assets?q=PC-ONE", None).await;
    assert_eq!(found["total"], 0);
    let run = s
        .ok(
            &s.owner,
            "POST",
            &format!("/api/v1/asset-mappings/{id}/run"),
            None,
        )
        .await;
    assert_eq!(run["created"], 2);
    let again = s
        .ok(
            &s.owner,
            "POST",
            &format!("/api/v1/asset-mappings/{id}/run"),
            None,
        )
        .await;
    assert_eq!(
        (again["created"].as_u64(), again["unchanged"].as_u64()),
        (Some(0), Some(2))
    );
    let found = s.ok(&p.tech, "GET", "/api/v1/assets?q=PC-ONE", None).await;
    assert_eq!(found["items"][0]["externalIds"]["rmm"], "d1");
    assert_eq!(found["items"][0]["fieldSources"]["hostname"], "rmm");
    let listed = s.ok(&s.owner, "GET", "/api/v1/asset-mappings", None).await;
    assert!(
        listed
            .as_array()
            .unwrap()
            .iter()
            .any(|m| m["id"] == id.as_str() && m["lastResult"]["unchanged"] == 2)
    );
}
