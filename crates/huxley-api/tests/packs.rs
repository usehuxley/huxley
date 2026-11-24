//! Packs over HTTP (ADR-028): upload, review, install, upgrade, conflicts,
//! against a real PostgreSQL.

use std::collections::BTreeMap;

use axum::body::Body;
use http::{Request, StatusCode};
use http_body_util::BodyExt as _;
use huxley_api::{ApiConfig, AppState, router};
use huxley_auth::{AuthConfig, AuthService};
use huxley_core::{RequestContext, Via};
use huxley_crypto::Crypto;
use huxley_db::testing::{TestPools, ensure_root, pools, unique};
use huxley_db::{OrgScope, begin_scoped};
use huxley_packs::{Contents, Manifest, SigningKey};
use serde_json::{Value, json};
use tower::ServiceExt as _;
use uuid::Uuid;

const PASSWORD: &str = "pack api passphrase";

const SPEC: &str = r#"
openapi: 3.0.3
info: { title: Widget Service, version: "1.0" }
servers: [{ url: "https://widgets.example.com" }]
paths:
  /widgets:
    get:
      operationId: listWidgets
      responses: { "200": { description: ok } }
"#;

fn crypto(pools: &TestPools) -> Crypto {
    Crypto::new(pools.app.clone(), huxley_crypto::testing::key_ring())
}

struct Setup {
    pools: TestPools,
    app: axum::Router,
    org: Uuid,
    owner: String,
    key: SigningKey,
}

async fn setup() -> Option<Setup> {
    let pools = pools().await?;
    let key = SigningKey::generate().unwrap();
    let auth = AuthService::new(pools.app.clone(), AuthConfig::default(), crypto(&pools));
    let config = ApiConfig {
        login_attempts_per_minute: 1000,
        ..ApiConfig::default()
    };
    let state = AppState::new(pools.app.clone(), auth, crypto(&pools), config);
    let state = AppState {
        trusted_pack_keys: std::sync::Arc::new(vec![key.public()]),
        ..state
    };
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
        key,
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
    async fn send(
        &self,
        cookie: &str,
        method: &str,
        uri: &str,
        content_type: &str,
        body: Vec<u8>,
    ) -> (StatusCode, Value) {
        let req = Request::builder()
            .method(method)
            .uri(uri)
            .header("x-requested-with", "huxley")
            .header("cookie", cookie)
            .header("content-type", content_type)
            .body(Body::from(body))
            .unwrap();
        let res = self.app.clone().oneshot(req).await.unwrap();
        let status = res.status();
        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        (
            status,
            serde_json::from_slice(&bytes)
                .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into_owned())),
        )
    }

    async fn upload(&self, cookie: &str, pack: Vec<u8>) -> (StatusCode, Value) {
        let uri = format!("/api/v1/packs/uploads?orgId={}", self.org);
        self.send(cookie, "POST", &uri, "application/octet-stream", pack)
            .await
    }

    async fn install(&self, cookie: &str, upload: &Value, version: &str) -> (StatusCode, Value) {
        let uri = format!(
            "/api/v1/packs/uploads/{}/install",
            upload["uploadId"].as_str().unwrap()
        );
        let body = json!({ "orgId": self.org, "version": version })
            .to_string()
            .into_bytes();
        self.send(cookie, "POST", &uri, "application/json", body)
            .await
    }

    async fn get(&self, uri: &str) -> (StatusCode, Value) {
        self.send(&self.owner, "GET", uri, "application/json", Vec::new())
            .await
    }

    fn pack(&self, version: &str, greeting: &str, signed: bool) -> Vec<u8> {
        let manifest = Manifest {
            api_version: "usehuxley.com/v1".into(),
            kind: "Pack".into(),
            key: "contoso/widgets".into(),
            name: "Widgets".into(),
            version: version.into(),
            description: "Widgets, a form and a greeting".into(),
            publisher: "Contoso".into(),
            contents: Contents {
                integrations: vec!["widgets".into()],
                workflows: vec!["greet".into()],
                forms: vec!["greet".into()],
                scripts: vec!["hello".into()],
                templates: vec!["starter".into()],
            },
            files: BTreeMap::new(),
        };
        let workflow = json!({
            "apiVersion": "usehuxley.com/v1",
            "kind": "Workflow",
            "metadata": { "key": "greet", "name": "Greet" },
            "spec": { "steps": [
                { "id": "greet", "type": "transform", "output": greeting },
                { "id": "ping", "type": "http.request",
                  "with": { "method": "GET", "url": "https://hooks.example.net/ping" } }
            ] }
        });
        let template = json!({
            "template": { "key": "starter", "name": "Widget starter", "description": "", "category": "Widgets" },
            "workflow": workflow
        });
        let files: BTreeMap<String, Vec<u8>> = [
            ("integrations/widgets.yaml", SPEC.as_bytes().to_vec()),
            ("workflows/greet.yaml", workflow.to_string().into_bytes()),
            (
                "forms/greet.json",
                json!({ "name": "Greet", "schema": { "type": "object", "properties": {} } })
                    .to_string()
                    .into_bytes(),
            ),
            (
                "scripts/hello.json",
                json!({ "name": "Hello", "language": "python" })
                    .to_string()
                    .into_bytes(),
            ),
            (
                "scripts/hello.py",
                format!("result = {greeting:?}\n").into_bytes(),
            ),
            ("templates/starter.yaml", template.to_string().into_bytes()),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_owned(), v))
        .collect();
        huxley_packs::build(manifest, &files, signed.then_some((&self.key, "Contoso"))).unwrap()
    }
}

/// The pack with one file's contents replaced, as an attacker would.
fn rewrite(pack: &[u8], name: &str, content: &[u8]) -> Vec<u8> {
    use std::io::{Read as _, Write as _};
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(pack)).unwrap();
    let mut out = std::io::Cursor::new(Vec::new());
    let mut writer = zip::ZipWriter::new(&mut out);
    for i in 0..archive.len() {
        let mut file = archive.by_index(i).unwrap();
        let entry = file.name().unwrap().into_owned();
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes).unwrap();
        writer
            .start_file(&entry, zip::write::SimpleFileOptions::default())
            .unwrap();
        writer
            .write_all(if entry == name { content } else { &bytes })
            .unwrap();
    }
    writer.finish().unwrap();
    out.into_inner()
}

#[tokio::test]
async fn packs_are_reviewed_installed_and_upgraded() {
    let Some(s) = setup().await else { return };
    // The review: what it adds, where it connects, its scripts, its signer.
    let (status, review) = s.upload(&s.owner, s.pack("1.0.0", "hello", true)).await;
    assert_eq!(status, StatusCode::CREATED, "{review}");
    assert_eq!(review["trust"]["status"], "trusted");
    assert_eq!(review["installedVersion"], Value::Null);
    assert_eq!(
        review["egress"],
        json!(["hooks.example.net", "widgets.example.com"])
    );
    assert_eq!(
        review["scripts"],
        json!([{ "key": "hello", "language": "python" }])
    );
    let actions: Vec<&str> = review["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["action"].as_str().unwrap())
        .collect();
    assert_eq!(actions, ["add"; 5]);

    // Installing needs the version reviewed.
    let (status, _) = s.install(&s.owner, &review, "9.9.9").await;
    assert_eq!(status, StatusCode::CONFLICT);
    let (status, installed) = s.install(&s.owner, &review, "1.0.0").await;
    assert_eq!(status, StatusCode::CREATED, "{installed}");
    let workflow_id = installed["items"]["workflows"]["greet"]
        .as_str()
        .unwrap()
        .to_owned();
    let script_id = installed["items"]["scripts"]["hello"]
        .as_str()
        .unwrap()
        .to_owned();
    // The upload is used up.
    let (status, _) = s.install(&s.owner, &review, "1.0.0").await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Workflows arrive as drafts.
    let (_, workflow) = s.get(&format!("/api/v1/workflows/{workflow_id}")).await;
    assert_eq!(workflow["publishedVersion"], Value::Null, "{workflow}");
    let (_, list) = s.get(&format!("/api/v1/packs?orgId={}", s.org)).await;
    assert_eq!(list[0]["key"], "contoso/widgets");
    assert_eq!(list[0]["trust"], "trusted");

    // An upgrade updates what it installed, in place.
    let (_, review) = s
        .upload(&s.owner, s.pack("1.1.0", "hello again", false))
        .await;
    assert_eq!(review["trust"]["status"], "unsigned");
    assert_eq!(review["installedVersion"], "1.0.0");
    assert!(
        review["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|i| i["action"] == "update"),
        "{review}"
    );
    let (status, upgraded) = s.install(&s.owner, &review, "1.1.0").await;
    assert_eq!(status, StatusCode::CREATED, "{upgraded}");
    assert_eq!(
        upgraded["items"]["workflows"]["greet"],
        workflow_id.as_str()
    );
    assert_eq!(upgraded["items"]["scripts"]["hello"], script_id.as_str());
    let (_, workflow) = s.get(&format!("/api/v1/workflows/{workflow_id}")).await;
    assert!(workflow.to_string().contains("hello again"), "{workflow}");
    let (_, script) = s.get(&format!("/api/v1/scripts/{script_id}")).await;
    assert!(script.to_string().contains("hello again"), "{script}");
    // The pack's template was replaced, not duplicated.
    let (_, templates) = s.get(&format!("/api/v1/templates?orgId={}", s.org)).await;
    let starters = templates
        .as_array()
        .unwrap()
        .iter()
        .filter(|t| t["name"] == "Widget starter")
        .count();
    assert_eq!(starters, 1);

    // Audited.
    let mut tx = begin_scoped(&s.pools.app, &OrgScope::System).await.unwrap();
    let audited: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_events WHERE action = 'pack.installed' AND org_id = $1",
    )
    .bind(s.org)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(audited, 2);
}

#[tokio::test]
async fn conflicts_tampering_and_access() {
    let Some(s) = setup().await else { return };
    // Something of the same key that the pack didn't install is never
    // overwritten.
    let body = json!({ "orgId": s.org,
        "form": { "key": "greet", "name": "Mine", "schema": { "type": "object", "properties": {} } } });
    let (status, created) = s
        .send(
            &s.owner,
            "POST",
            "/api/v1/forms",
            "application/json",
            body.to_string().into_bytes(),
        )
        .await;
    assert!(status.is_success(), "{created}");
    let (_, review) = s.upload(&s.owner, s.pack("1.0.0", "hello", true)).await;
    let form = review["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["kind"] == "forms")
        .unwrap();
    assert_eq!(form["action"], "conflict");
    let (status, problem) = s.install(&s.owner, &review, "1.0.0").await;
    assert_eq!(status, StatusCode::CONFLICT, "{problem}");
    // Nothing was installed.
    let (_, list) = s.get(&format!("/api/v1/packs?orgId={}", s.org)).await;
    assert_eq!(list, json!([]));
    let (_, integrations) = s
        .get(&format!("/api/v1/integrations?orgId={}", s.org))
        .await;
    let org = s.org.to_string();
    assert!(
        !integrations
            .as_array()
            .unwrap()
            .iter()
            .any(|i| i["orgId"] == org.as_str() && i["key"] == "widgets"),
        "the integration wasn't added"
    );

    // A tampered pack is refused at upload.
    let bytes = rewrite(
        &s.pack("1.0.0", "hello", true),
        "scripts/hello.py",
        b"import os\n",
    );
    let (status, problem) = s.upload(&s.owner, bytes).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(
        problem["detail"]
            .as_str()
            .unwrap()
            .contains("scripts/hello.py"),
        "{problem}"
    );

    // Someone without access can't upload, and can't see others' uploads.
    let stranger = login(&s.app, &user(&s.pools, false).await).await;
    let (status, _) = s.upload(&stranger, s.pack("1.0.0", "hello", true)).await;
    assert!(
        matches!(status, StatusCode::FORBIDDEN | StatusCode::NOT_FOUND),
        "{status}"
    );
    let (status, _) = s.install(&stranger, &review, "1.0.0").await;
    assert!(
        matches!(status, StatusCode::FORBIDDEN | StatusCode::NOT_FOUND),
        "{status}"
    );
}

#[tokio::test]
async fn installing_needs_the_permissions_for_what_it_adds() {
    let Some(s) = setup().await else { return };
    // A role that may install packs, but not manage integrations, forms,
    // scripts or workflows.
    let (owner_id, _) = user_with_id(&s.pools, true).await;
    let (installer_id, installer_email) = user_with_id(&s.pools, false).await;
    let ctx = RequestContext::system(Via::System);
    let mut tx = begin_scoped(&s.pools.app, &OrgScope::System).await.unwrap();
    let owner = huxley_authz::load_principal(&mut tx, owner_id)
        .await
        .unwrap();
    let role = huxley_authz::roles::create(
        &mut tx,
        &ctx,
        &owner,
        &huxley_authz::roles::RoleInput {
            key: None,
            name: unique("Pack installer"),
            description: String::new(),
            permissions: vec!["pack:install".into(), "org:read".into()],
        },
    )
    .await
    .unwrap();
    huxley_authz::assign(&mut tx, &ctx, &owner, installer_id, role.id, s.org, false)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let installer = login(&s.app, &installer_email).await;

    // They can upload and review it...
    let (status, review) = s.upload(&installer, s.pack("1.0.0", "hello", true)).await;
    assert_eq!(status, StatusCode::CREATED, "{review}");
    // ...but not install what they couldn't add by hand.
    let (status, _) = s.install(&installer, &review, "1.0.0").await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (_, list) = s.get(&format!("/api/v1/packs?orgId={}", s.org)).await;
    assert_eq!(list, json!([]));
}
