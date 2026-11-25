//! The `knowledge` module's API against a real PostgreSQL (design doc
//! §8.5): who may write, publish and read articles, and what readers see.

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

const PASSWORD: &str = "knowledge api passphrase";

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
    for module in ["crm", "servicedesk", "knowledge"] {
        huxley_modules::set_enabled(&mut tx, &ctx, module, true, false)
            .await
            .unwrap();
    }
    tx.commit().await.unwrap();
    let provider = org(&pools, root, huxley_orgs::OrgKind::Provider, "KB MSP").await;
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
    let email = format!("{}@example.com", unique("kb").replace(' ', "."));
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

fn field(problem: &Value) -> String {
    problem["errors"][0]["pointer"]
        .as_str()
        .unwrap_or_default()
        .to_owned()
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
        "KB client",
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

/// An article the technician wrote and the owner published: its id.
async fn published(s: &Setup, p: &People, title: &str, body: &str) -> String {
    let a = s
        .ok(
            &p.tech,
            "POST",
            "/api/v1/kb/articles",
            Some(json!({ "orgId": s.provider, "title": title, "body": body })),
        )
        .await;
    let id = a["id"].as_str().unwrap().to_owned();
    s.ok(
        &s.owner,
        "POST",
        &format!("/api/v1/kb/articles/{id}/publish"),
        None,
    )
    .await;
    id
}

#[tokio::test]
async fn writers_write_and_publishers_review() {
    let Some(s) = setup().await else { return };
    let p = people(&s).await;
    // A technician writes a draft at the MSP; it's theirs to change, not publish.
    let body = json!({ "orgId": s.provider, "title": "Reset MFA", "body": "1. Open **Entra**", "tags": ["mfa"] });
    let a = s
        .ok(&p.tech, "POST", "/api/v1/kb/articles", Some(body))
        .await;
    let id = a["id"].as_str().unwrap().to_owned();
    assert_eq!(
        (
            a["status"].as_str(),
            a["canWrite"].as_bool(),
            a["canPublish"].as_bool()
        ),
        (Some("draft"), Some(true), Some(false))
    );
    assert!(
        a["bodyHtml"]
            .as_str()
            .unwrap()
            .contains("<strong>Entra</strong>")
    );
    let (status, _) = s
        .call(
            &p.tech,
            "POST",
            &format!("/api/v1/kb/articles/{id}/publish"),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    // Readers can't write, and see nothing until it's published.
    let (status, _) = s
        .call(
            &p.reader,
            "POST",
            "/api/v1/kb/articles",
            Some(json!({ "orgId": s.provider, "title": "x" })),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = s
        .call(&p.reader, "GET", &format!("/api/v1/kb/articles/{id}"), None)
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Submitted, rejected with a reason, fixed, resubmitted and approved.
    s.ok(
        &p.tech,
        "POST",
        &format!("/api/v1/kb/articles/{id}/submit"),
        None,
    )
    .await;
    let uri = format!("/api/v1/kb/articles/{id}/reject");
    let (status, problem) = s.call(&s.owner, "POST", &uri, Some(json!({}))).await;
    assert_eq!(
        (status, field(&problem).as_str()),
        (StatusCode::UNPROCESSABLE_ENTITY, "/comment")
    );
    s.ok(
        &s.owner,
        "POST",
        &uri,
        Some(json!({ "comment": "Add PowerShell" })),
    )
    .await;
    let a = s
        .ok(&p.tech, "GET", &format!("/api/v1/kb/articles/{id}"), None)
        .await;
    let changes = json!({ "version": a["version"], "body": "1. Open **Entra**\n2. `Reset-Mfa`" });
    s.ok(
        &p.tech,
        "PATCH",
        &format!("/api/v1/kb/articles/{id}"),
        Some(changes),
    )
    .await;
    s.ok(
        &p.tech,
        "POST",
        &format!("/api/v1/kb/articles/{id}/submit"),
        None,
    )
    .await;
    let a = s
        .ok(
            &s.owner,
            "POST",
            &format!("/api/v1/kb/articles/{id}/publish"),
            Some(json!({ "comment": "Thanks" })),
        )
        .await;
    assert_eq!(
        (a["status"].as_str(), a["publishedRevision"].as_i64()),
        (Some("published"), Some(2))
    );
}

#[tokio::test]
async fn readers_see_what_is_published() {
    let Some(s) = setup().await else { return };
    let p = people(&s).await;
    let word = unique("glacierproxy").replace(['-', ' '], "");
    let id = published(&s, &p, "Proxy settings", &format!("Set the {word} address")).await;
    let a = s
        .ok(&p.tech, "GET", &format!("/api/v1/kb/articles/{id}"), None)
        .await;
    // A new draft: readers keep the published revision, without history.
    let changes = json!({ "version": a["version"], "title": "Proxy settings (draft)" });
    let a = s
        .ok(
            &p.tech,
            "PATCH",
            &format!("/api/v1/kb/articles/{id}"),
            Some(changes),
        )
        .await;
    let seen = s
        .ok(&p.reader, "GET", &format!("/api/v1/kb/articles/{id}"), None)
        .await;
    assert_eq!(
        (seen["title"].as_str(), seen["revision"].as_i64()),
        (Some("Proxy settings"), Some(1))
    );
    assert_eq!(seen["activity"], json!([]));
    let found = s
        .ok(
            &p.reader,
            "GET",
            &format!("/api/v1/kb/articles?q={word}"),
            None,
        )
        .await;
    assert_eq!(found["items"][0]["title"], "Proxy settings");
    let (status, _) = s
        .call(
            &p.reader,
            "GET",
            &format!("/api/v1/kb/articles/{id}/revisions"),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let revisions = s
        .ok(
            &p.tech,
            "GET",
            &format!("/api/v1/kb/articles/{id}/revisions"),
            None,
        )
        .await;
    assert_eq!(revisions.as_array().unwrap().len(), 2);
    // Only a publisher may change who sees a published article.
    let changes = json!({ "version": a["version"], "visibility": "public" });
    let (status, _) = s
        .call(
            &p.tech,
            "PATCH",
            &format!("/api/v1/kb/articles/{id}"),
            Some(changes.clone()),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    s.ok(
        &s.owner,
        "PATCH",
        &format!("/api/v1/kb/articles/{id}"),
        Some(changes),
    )
    .await;
    // A client's technician doesn't see the MSP's articles.
    let (status, _) = s
        .call(
            &p.outsider,
            "GET",
            &format!("/api/v1/kb/articles/{id}"),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let theirs = s
        .ok(
            &p.outsider,
            "GET",
            &format!("/api/v1/kb/articles?q={word}"),
            None,
        )
        .await;
    assert_eq!(theirs["total"], 0);
}

#[tokio::test]
async fn categories_are_kept_by_publishers_and_previews_are_safe() {
    let Some(s) = setup().await else { return };
    let p = people(&s).await;
    let category = json!({ "orgId": s.provider, "category": { "name": unique("Identity") } });
    let (status, _) = s
        .call(
            &p.tech,
            "POST",
            "/api/v1/kb/categories",
            Some(category.clone()),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let made = s
        .ok(&s.owner, "POST", "/api/v1/kb/categories", Some(category))
        .await;
    let listed = s.ok(&p.reader, "GET", "/api/v1/kb/categories", None).await;
    assert!(
        listed
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c["id"] == made["id"])
    );
    let preview = s
        .ok(
            &p.tech,
            "POST",
            "/api/v1/kb/preview",
            Some(json!({ "body": "**hi** <script>x</script>" })),
        )
        .await;
    assert_eq!(
        preview["html"].as_str().unwrap().trim(),
        "<p><strong>hi</strong> &lt;script&gt;x&lt;/script&gt;</p>"
    );
    let (status, _) = s
        .call(
            &p.reader,
            "POST",
            "/api/v1/kb/preview",
            Some(json!({ "body": "x" })),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn articles_are_linked_to_tickets_and_suggested() {
    let Some(s) = setup().await else { return };
    let p = people(&s).await;
    let word = unique("wombatscanner").replace(['-', ' '], "");
    let fix = published(
        &s,
        &p,
        &format!("Fixing the {word}"),
        &format!("Restart the {word} service."),
    )
    .await;
    let ticket = s
        .ok(
            &p.tech,
            "POST",
            "/api/v1/tickets",
            Some(json!({ "orgId": s.provider, "title": format!("{word} not scanning"), "description": format!("The {word} is stuck") })),
        )
        .await;
    let t = ticket["id"].as_str().unwrap().to_owned();
    let uri = format!("/api/v1/kb/tickets/{t}/articles");

    // Suggested by its words, before it's linked.
    let k = s.ok(&p.tech, "GET", &uri, None).await;
    assert_eq!(k["suggested"][0]["id"], fix.as_str(), "{k}");
    assert_eq!(
        (
            k["linked"].as_array().unwrap().len(),
            k["canLink"].as_bool()
        ),
        (0, Some(true))
    );
    // Readers can't link, but see what's linked.
    let (status, _) = s
        .call(&p.reader, "POST", &uri, Some(json!({ "articleId": fix })))
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    s.ok(&p.tech, "POST", &uri, Some(json!({ "articleId": fix })))
        .await;
    let k = s.ok(&p.reader, "GET", &uri, None).await;
    assert_eq!(
        (k["linked"][0]["id"].as_str(), k["canLink"].as_bool()),
        (Some(fix.as_str()), Some(false))
    );
    assert_eq!(k["suggested"], json!([]));
    // The article lists the ticket for writers.
    let used = s
        .ok(
            &p.tech,
            "GET",
            &format!("/api/v1/kb/articles/{fix}/tickets"),
            None,
        )
        .await;
    assert_eq!(used[0]["id"], t.as_str());
    // A client's technician can't see the MSP's ticket.
    let (status, _) = s.call(&p.outsider, "GET", &uri, None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    // Unlinked.
    let (status, _) = s
        .call(&p.tech, "DELETE", &format!("{uri}/{fix}"), None)
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let k = s.ok(&p.tech, "GET", &uri, None).await;
    assert_eq!(k["linked"], json!([]));
}
