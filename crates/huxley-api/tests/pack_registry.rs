//! Pack registries (ADR-093) over HTTP, against a registry served from this
//! test: browsing with installed versions, reviewing and installing from a
//! registry, refusing a download that doesn't match the index, and checking
//! a signed index's key. Against a real PostgreSQL.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::StatusCode as AxumStatus;
use http::{Request, StatusCode};
use http_body_util::BodyExt as _;
use huxley_api::pack_registry::PackRegistries;
use huxley_api::{ApiConfig, AppState, router};
use huxley_auth::{AuthConfig, AuthService};
use huxley_core::{RequestContext, Via};
use huxley_crypto::Crypto;
use huxley_db::testing::{TestPools, ensure_root, pools, unique};
use huxley_db::{OrgScope, begin_scoped};
use huxley_packs::registry::{RegistryConfig, build_index, sign_index};
use huxley_packs::{Contents, Manifest, SigningKey};
use serde_json::{Value, json};
use tower::ServiceExt as _;
use uuid::Uuid;

const PASSWORD: &str = "registry api passphrase";

type Files = Arc<Mutex<HashMap<String, Vec<u8>>>>;

/// A static file server standing in for a registry.
async fn registry_server(files: Files) -> String {
    async fn serve(State(files): State<Files>, Path(path): Path<String>) -> (AxumStatus, Vec<u8>) {
        match files.lock().unwrap().get(&path) {
            Some(bytes) => (AxumStatus::OK, bytes.clone()),
            None => (AxumStatus::NOT_FOUND, Vec::new()),
        }
    }
    let app = axum::Router::new()
        .route("/{*path}", axum::routing::get(serve))
        .with_state(files);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    base
}

fn pack(key: &str, version: &str, signer: &SigningKey) -> Vec<u8> {
    let name = key.split('/').nth(1).unwrap();
    let manifest = Manifest {
        api_version: "usehuxley.io/v1".into(),
        kind: "Pack".into(),
        key: key.into(),
        name: format!("{name} {version}"),
        version: version.into(),
        description: "A form for greeting".into(),
        publisher: "Contoso".into(),
        contents: Contents {
            forms: vec![format!("{name}-form")],
            ..Contents::default()
        },
        files: BTreeMap::new(),
    };
    let files: BTreeMap<String, Vec<u8>> = [(
        format!("forms/{name}-form.json"),
        json!({ "name": format!("Greet {version}"), "schema": { "type": "object", "properties": {} } })
            .to_string()
            .into_bytes(),
    )]
    .into_iter()
    .collect();
    huxley_packs::build(manifest, &files, Some((signer, "Contoso"))).unwrap()
}

struct Setup {
    app: axum::Router,
    org: Uuid,
    cookie: String,
}

async fn setup(pools: &TestPools, registries: Vec<RegistryConfig>, trusted: Vec<String>) -> Setup {
    let crypto = Crypto::new(pools.app.clone(), huxley_crypto::testing::key_ring());
    let auth = AuthService::new(pools.app.clone(), AuthConfig::default(), crypto.clone());
    let config = ApiConfig {
        login_attempts_per_minute: 1000,
        ..ApiConfig::default()
    };
    let state = AppState::new(pools.app.clone(), auth, crypto, config);
    let state = AppState {
        trusted_pack_keys: Arc::new(trusted),
        pack_registries: Arc::new(PackRegistries::new(registries)),
        ..state
    };
    let app = router(state);
    let root = ensure_root(pools).await;
    let ctx = RequestContext::system(Via::System);
    let email = format!("{}@example.com", unique("reg").replace(' ', "."));
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    let org = huxley_orgs::create(
        &mut tx,
        &ctx,
        root,
        huxley_orgs::OrgKind::Provider,
        &unique("Reg"),
    )
    .await
    .unwrap()
    .id;
    let user = huxley_auth::create_user(&mut tx, &ctx, &email, "Registry User", PASSWORD)
        .await
        .unwrap();
    huxley_authz::grant_root_owner(&mut tx, &ctx, user.id)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let res = app
        .clone()
        .oneshot(
            Request::post("/api/v1/auth/login")
                .header("x-requested-with", "huxley")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({ "email": email, "password": PASSWORD }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let cookie = res.headers()["set-cookie"]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    Setup { app, org, cookie }
}

impl Setup {
    async fn call(&self, method: &str, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
        let req = Request::builder()
            .method(method)
            .uri(uri)
            .header("x-requested-with", "huxley")
            .header("cookie", &self.cookie)
            .header("content-type", "application/json")
            .body(body.map_or_else(Body::empty, |b| Body::from(b.to_string())))
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
async fn packs_are_browsed_reviewed_and_installed_from_a_registry() {
    let Some(pools) = pools().await else { return };
    let signer = SigningKey::generate().unwrap();
    let files: Files = Arc::default();
    let base = registry_server(files.clone()).await;
    let key = format!(
        "contoso/{}",
        unique("greet").replace(' ', "-").to_lowercase()
    );
    let packs = vec![
        (
            "greet-1.9.0.hxpack".to_owned(),
            pack(&key, "1.9.0", &signer),
        ),
        (
            "greet-1.10.0.hxpack".to_owned(),
            pack(&key, "1.10.0", &signer),
        ),
    ];
    let index = serde_json::to_vec(&build_index("Contoso packs", &base, &packs).unwrap()).unwrap();
    {
        let mut f = files.lock().unwrap();
        f.insert("index.json".into(), index);
        for (name, bytes) in &packs {
            f.insert(name.clone(), bytes.clone());
        }
    }
    let s = setup(
        &pools,
        vec![RegistryConfig {
            url: format!("{base}/index.json"),
            key: None,
        }],
        vec![signer.public()],
    )
    .await;

    let (status, listing) = s
        .call(
            "GET",
            &format!("/api/v1/packs/registry?orgId={}", s.org),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{listing}");
    assert_eq!(listing[0]["name"], "Contoso packs");
    let entry = &listing[0]["packs"][0];
    assert_eq!(entry["key"], key);
    assert_eq!(entry["versions"][0]["version"], "1.10.0", "newest first");
    assert_eq!(entry["trusted"], true);
    assert!(entry["installed"].is_null());

    // Review, then install, as an upload.
    let review_body = json!({ "registry": 0, "key": key, "version": "1.10.0" });
    let (status, review) = s
        .call(
            "POST",
            &format!("/api/v1/packs/registry/review?orgId={}", s.org),
            Some(review_body),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{review}");
    assert_eq!(review["trust"]["status"], "trusted", "{review}");
    let upload = review["uploadId"].as_str().unwrap();
    let (status, installed) = s
        .call(
            "POST",
            &format!("/api/v1/packs/uploads/{upload}/install"),
            Some(json!({ "orgId": s.org, "version": "1.10.0" })),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{installed}");
    let (_, listing) = s
        .call(
            "GET",
            &format!("/api/v1/packs/registry?orgId={}", s.org),
            None,
        )
        .await;
    assert_eq!(listing[0]["packs"][0]["installed"], "1.10.0");

    // A download that doesn't match the index is refused.
    files.lock().unwrap().insert(
        "greet-1.9.0.hxpack".into(),
        pack(&key, "1.9.0", &SigningKey::generate().unwrap()),
    );
    let (status, problem) = s
        .call(
            "POST",
            &format!("/api/v1/packs/registry/review?orgId={}", s.org),
            Some(json!({ "registry": 0, "key": key, "version": "1.9.0" })),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_GATEWAY, "{problem}");
    assert!(
        problem["detail"].as_str().unwrap().contains("SHA-256"),
        "{problem}"
    );
    let (status, _) = s
        .call(
            "POST",
            &format!("/api/v1/packs/registry/review?orgId={}", s.org),
            Some(json!({ "registry": 0, "key": key, "version": "9.9.9" })),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_signed_index_must_carry_the_registry_key() {
    let Some(pools) = pools().await else { return };
    let (signer, registry_key, other) = (
        SigningKey::generate().unwrap(),
        SigningKey::generate().unwrap(),
        SigningKey::generate().unwrap(),
    );
    let files: Files = Arc::default();
    let base = registry_server(files.clone()).await;
    let packs = vec![(
        "hello-1.0.0.hxpack".to_owned(),
        pack("contoso/hello", "1.0.0", &signer),
    )];
    let index = serde_json::to_vec(&build_index("Signed packs", &base, &packs).unwrap()).unwrap();
    {
        let mut f = files.lock().unwrap();
        f.insert(
            "index.json.sig".into(),
            sign_index(&index, &registry_key, "Signed packs").unwrap(),
        );
        f.insert("index.json".into(), index);
    }
    let url = format!("{base}/index.json");
    let s = setup(
        &pools,
        vec![
            RegistryConfig {
                url: url.clone(),
                key: Some(registry_key.public()),
            },
            RegistryConfig {
                url,
                key: Some(other.public()),
            },
        ],
        vec![],
    )
    .await;
    let (_, listing) = s
        .call(
            "GET",
            &format!("/api/v1/packs/registry?orgId={}", s.org),
            None,
        )
        .await;
    assert_eq!(listing[0]["signed"], true);
    assert!(listing[0]["error"].is_null(), "{listing}");
    assert_eq!(listing[0]["packs"][0]["key"], "contoso/hello");
    assert_eq!(
        listing[0]["packs"][0]["trusted"], false,
        "signed by a key this instance doesn't trust"
    );
    assert!(
        listing[1]["error"].as_str().unwrap().contains("signature"),
        "the wrong key is refused: {listing}"
    );
    assert_eq!(listing[1]["packs"], json!([]));
}
