//! Email intake against a real PostgreSQL (ADR-038): mailboxes, posting
//! messages, and turning them into tickets and replies (threading, sender
//! matching, automatic mail and loops).

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

const PASSWORD: &str = "mail api passphrase";

struct Setup {
    pools: TestPools,
    app: axum::Router,
    provider: Uuid,
    acme: Uuid,
    globex: Uuid,
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
    for module in ["crm", "servicedesk"] {
        huxley_modules::set_enabled(&mut tx, &ctx, module, true, false)
            .await
            .unwrap();
    }
    tx.commit().await.unwrap();
    let provider = org(&pools, root, huxley_orgs::OrgKind::Provider, "Mail MSP").await;
    let acme = org(&pools, provider, huxley_orgs::OrgKind::Client, "Acme").await;
    let globex = org(&pools, provider, huxley_orgs::OrgKind::Client, "Globex").await;
    let owner = login(&app, &user(&pools, None).await.1).await;
    Some(Setup {
        pools,
        app,
        provider,
        acme,
        globex,
        owner,
    })
}

/// A user, optionally with a built-in role at an organization.
async fn user(pools: &TestPools, role_at: Option<(&str, Uuid)>) -> (Uuid, String) {
    let email = format!("{}@example.com", unique("desk").replace(' ', "."));
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

/// A mailbox at the provider, `<name>@<unique>.example`.
async fn mailbox(s: &Setup, extra: Value) -> Value {
    let mut body = json!({ "orgId": s.provider, "name": "Help", "address": format!("help@{}.example", Uuid::now_v7().simple()) });
    if let (Some(b), Some(e)) = (body.as_object_mut(), extra.as_object()) {
        b.extend(e.clone());
    }
    s.ok(&s.owner, "POST", "/api/v1/mailboxes", Some(body))
        .await
}

/// Posts a raw message to a mailbox.
async fn post(s: &Setup, mailbox: &Value, raw: &str) -> (StatusCode, Value) {
    let req = Request::post(format!(
        "/api/v1/mailboxes/{}/messages",
        mailbox["id"].as_str().unwrap()
    ))
    .header("x-requested-with", "huxley")
    .header("cookie", &s.owner)
    .header("content-type", "message/rfc822")
    .body(Body::from(raw.replace('\n', "\r\n")))
    .unwrap();
    let res = s.app.clone().oneshot(req).await.unwrap();
    let status = res.status();
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

fn message(from: &str, subject: &str, id: &str, extra: &str, body: &str) -> String {
    format!(
        "From: {from}\nTo: help@msp.example\nSubject: {subject}\nMessage-ID: <{id}>\n{extra}\n{body}\n"
    )
}

/// Turns an email into ticket work, as the step does, for the provider.
async fn process(s: &Setup, email: &Value) -> huxley_servicedesk::mail::Outcome {
    let ctx = RequestContext::system(Via::Workflow);
    let mut tx = begin_scoped(&s.pools.app, &OrgScope::System).await.unwrap();
    let id = Uuid::parse_str(email["emailId"].as_str().unwrap()).unwrap();
    let outcome = huxley_servicedesk::mail::to_ticket(&mut tx, &ctx, id, s.provider)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    outcome
}

async fn ticket(s: &Setup, id: Uuid) -> Value {
    s.ok(&s.owner, "GET", &format!("/api/v1/tickets/{id}"), None)
        .await
}

#[tokio::test]
async fn a_message_becomes_a_ticket_and_replies_thread_onto_it() {
    let Some(s) = setup().await else { return };
    let tag = Uuid::now_v7().simple().to_string();
    let ada = format!("ada.{tag}@acme.example");
    s.ok(
        &s.owner,
        "POST",
        &format!("/api/v1/orgs/{}/contacts", s.acme),
        Some(json!({ "firstName": "Ada", "lastName": "Lovelace", "email": ada })),
    )
    .await;
    let mb = mailbox(&s, json!({ "ticketType": "service_request" })).await;

    // Posted once (twice is the same message).
    let raw = message(
        &format!("Ada <{ada}>"),
        "Need a laptop",
        &format!("one.{tag}@acme.example"),
        "",
        "Mine broke.\n-- \nAda",
    );
    let (status, first) = post(&s, &mb, &raw).await;
    assert_eq!(
        (status, first["duplicate"].clone()),
        (StatusCode::ACCEPTED, json!(false)),
        "{first}"
    );
    let (_, again) = post(&s, &mb, &raw).await;
    assert_eq!(
        (again["emailId"].clone(), again["duplicate"].clone()),
        (first["emailId"].clone(), json!(true))
    );

    // A ticket for Ada at Acme: subject as title, signature stripped.
    let outcome = process(&s, &first).await;
    assert_eq!(
        (outcome.outcome.as_str(), outcome.created),
        ("ticket_created", true)
    );
    let t = ticket(&s, outcome.ticket_id.unwrap()).await;
    assert_eq!(t["orgId"], json!(s.acme));
    assert_eq!(
        (t["title"].as_str(), t["description"].as_str()),
        (Some("Need a laptop"), Some("Mine broke."))
    );
    assert_eq!(
        (t["source"].as_str(), t["typeKey"].as_str()),
        (Some("email"), Some("service_request"))
    );
    assert_eq!(t["requesterName"], "Ada Lovelace");
    // Processing it again changes nothing.
    let same = process(&s, &first).await;
    assert_eq!((same.ticket_id, same.created), (outcome.ticket_id, false));

    // A reply by In-Reply-To, quote stripped: a public note from Ada (not a first response).
    let reply = message(
        &ada,
        "Re: Need a laptop",
        &format!("two.{tag}@acme.example"),
        &format!("In-Reply-To: <one.{tag}@acme.example>\n"),
        "Any news?\n\nOn Mon, Ada wrote:\n> Mine broke.",
    );
    let (_, r) = post(&s, &mb, &reply).await;
    let out = process(&s, &r).await;
    assert_eq!(
        (out.outcome.as_str(), out.ticket_id),
        ("note_added", outcome.ticket_id)
    );
    let url = format!("/api/v1/tickets/{}", outcome.ticket_id.unwrap());
    let notes = s.ok(&s.owner, "GET", &format!("{url}/notes"), None).await;
    assert_eq!(notes[0]["body"], "Any news?");
    assert_eq!(
        (
            notes[0]["authorContactName"].as_str(),
            notes[0]["authorEmail"].as_str()
        ),
        (Some("Ada Lovelace"), Some(ada.as_str()))
    );
    assert!(ticket(&s, outcome.ticket_id.unwrap()).await["firstResponseAt"].is_null());
}

#[tokio::test]
async fn unknown_senders_follow_the_mailbox_policy() {
    let Some(s) = setup().await else { return };
    let tag = Uuid::now_v7().simple().to_string();
    let domain = format!("{tag}.globex.example");
    // A known colleague at Globex: a stranger from the same domain lands there.
    s.ok(
        &s.owner,
        "POST",
        &format!("/api/v1/orgs/{}/contacts", s.globex),
        Some(json!({ "firstName": "Hank", "email": format!("hank@{domain}") })),
    )
    .await;
    let mb = mailbox(&s, json!({})).await;
    let (_, e) = post(
        &s,
        &mb,
        &message(
            &format!("Marge Bouvier <marge@{domain}>"),
            "Hi",
            &format!("m.{tag}@x"),
            "",
            "Help",
        ),
    )
    .await;
    let out = process(&s, &e).await;
    let t = ticket(&s, out.ticket_id.unwrap()).await;
    assert_eq!(
        (t["orgId"].clone(), t["requesterName"].as_str()),
        (json!(s.globex), Some("Marge Bouvier"))
    );

    // Personal mail with nowhere else to go: the unknown-sender organization, as a new contact.
    let mb2 = mailbox(&s, json!({ "unknownOrgId": s.acme })).await;
    let (_, e) = post(
        &s,
        &mb2,
        &message(
            &format!("bart.{tag}@gmail.com"),
            "",
            &format!("b.{tag}@x"),
            "",
            "Eat my shorts",
        ),
    )
    .await;
    let t = ticket(&s, process(&s, &e).await.ticket_id.unwrap()).await;
    assert_eq!(
        (t["orgId"].clone(), t["title"].as_str()),
        (json!(s.acme), Some("(no subject)"))
    );
    assert_eq!(t["requesterEmail"], format!("bart.{tag}@gmail.com"));

    // No requester, or ignored.
    let none = mailbox(&s, json!({ "unknownSender": "no_requester" })).await;
    let (_, e) = post(
        &s,
        &none,
        &message(
            &format!("lisa.{tag}@gmail.com"),
            "Sax",
            &format!("l.{tag}@x"),
            "",
            "Hi",
        ),
    )
    .await;
    let t = ticket(&s, process(&s, &e).await.ticket_id.unwrap()).await;
    assert!(t["requesterId"].is_null());
    assert_eq!(t["orgId"], json!(s.provider));
    let ignore = mailbox(&s, json!({ "unknownSender": "ignore" })).await;
    let (_, e) = post(
        &s,
        &ignore,
        &message(
            &format!("moe.{tag}@gmail.com"),
            "Hi",
            &format!("o.{tag}@x"),
            "",
            "Hi",
        ),
    )
    .await;
    let out = process(&s, &e).await;
    assert_eq!(
        (out.outcome.as_str(), out.ticket_id),
        ("ignored_unknown_sender", None)
    );
}

#[tokio::test]
async fn automatic_mail_and_loops_are_skipped() {
    let Some(s) = setup().await else { return };
    let tag = Uuid::now_v7().simple().to_string();
    let mb = mailbox(&s, json!({})).await;
    let other = mailbox(&s, json!({})).await;
    let cases = [
        (
            message(
                "someone@corp.example",
                "Out of office",
                &format!("a.{tag}@x"),
                "Auto-Submitted: auto-replied\n",
                "Away",
            ),
            "ignored_automatic",
        ),
        (
            message(
                "MAILER-DAEMON@mx.example",
                "Undeliverable",
                &format!("b.{tag}@x"),
                "",
                "Failed",
            ),
            "ignored_automatic",
        ),
        (
            message(
                other["address"].as_str().unwrap(),
                "Ping",
                &format!("c.{tag}@x"),
                "",
                "Loop",
            ),
            "ignored_loop",
        ),
    ];
    for (raw, expected) in cases {
        let (_, e) = post(&s, &mb, &raw).await;
        let out = process(&s, &e).await;
        assert_eq!(
            (out.outcome.as_str(), out.ticket_id),
            (expected, None),
            "{raw}"
        );
    }
    // A flood from one sender.
    let flood = format!("flood.{tag}@corp.example");
    let mut latest = Value::Null;
    for i in 0..22 {
        latest = post(
            &s,
            &mb,
            &message(&flood, "Again", &format!("f{i}.{tag}@x"), "", "Again"),
        )
        .await
        .1;
    }
    assert_eq!(process(&s, &latest).await.outcome, "ignored_loop");
    // The mailbox's list shows what became of each.
    let list = s
        .ok(
            &s.owner,
            "GET",
            &format!(
                "/api/v1/mailboxes/{}/emails?limit=5",
                mb["id"].as_str().unwrap()
            ),
            None,
        )
        .await;
    assert_eq!(list.as_array().unwrap().len(), 5);
    assert_eq!(list[0]["outcome"], "ignored_loop");
}

#[tokio::test]
async fn mailboxes_are_checked_and_guarded() {
    let Some(s) = setup().await else { return };
    for (body, pointer) in [
        (
            json!({ "orgId": s.provider, "name": "x", "address": "not-an-address" }),
            "/address",
        ),
        (
            json!({ "orgId": s.provider, "name": "x", "address": "a@b.example", "kind": "pigeon" }),
            "/kind",
        ),
        (
            json!({ "orgId": s.provider, "name": "x", "address": "a@b.example", "unknownSender": "shrug" }),
            "/unknownSender",
        ),
        (
            json!({ "orgId": s.provider, "name": "x", "address": "a@b.example", "ticketType": "nope" }),
            "/ticketType",
        ),
    ] {
        let (status, problem) = s
            .call(&s.owner, "POST", "/api/v1/mailboxes", Some(body.clone()))
            .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body} {problem}");
        assert_eq!(problem["errors"][0]["pointer"], pointer, "{body}");
    }
    let mb = mailbox(&s, json!({})).await;
    let (status, problem) = s
        .call(&s.owner, "POST", "/api/v1/mailboxes",
              Some(json!({ "orgId": s.provider, "name": "Dup", "address": mb["address"].as_str().unwrap().to_uppercase() })))
        .await;
    assert_eq!(
        (status, problem["errors"][0]["pointer"].as_str()),
        (StatusCode::UNPROCESSABLE_ENTITY, Some("/address"))
    );

    // Not a message; a mailbox that's off.
    let (status, _) = post(&s, &mb, "").await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    let url = format!("/api/v1/mailboxes/{}", mb["id"].as_str().unwrap());
    let mut body = mb.clone();
    for key in [
        "id",
        "orgId",
        "lastCheckedAt",
        "hasPassword",
        "lastError",
        "createdAt",
    ] {
        body.as_object_mut().unwrap().remove(key);
    }
    body["enabled"] = json!(false);
    s.ok(&s.owner, "PUT", &url, Some(body)).await;
    assert_eq!(
        post(&s, &mb, &message("a@b.example", "x", "off@x", "", "x"))
            .await
            .0,
        StatusCode::CONFLICT
    );

    // Technicians can't manage mailboxes or post to them.
    let tech = login(
        &s.app,
        &user(&s.pools, Some(("technician", s.provider))).await.1,
    )
    .await;
    assert_eq!(
        s.call(&tech, "GET", "/api/v1/mailboxes", None).await.0,
        StatusCode::FORBIDDEN
    );
    let req = Request::post(format!("{url}/messages"))
        .header("x-requested-with", "huxley")
        .header("cookie", &tech)
        .body(Body::from("From: a@b.example\r\n\r\nx"))
        .unwrap();
    assert_eq!(
        s.app.clone().oneshot(req).await.unwrap().status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        s.call(&s.owner, "DELETE", &url, None).await.0,
        StatusCode::NO_CONTENT
    );
}

#[tokio::test]
async fn replies_reopen_resolved_tickets_and_closed_ones_start_over() {
    let Some(s) = setup().await else { return };
    let tag = Uuid::now_v7().simple().to_string();
    let ada = format!("ada.{tag}@acme.example");
    s.ok(
        &s.owner,
        "POST",
        &format!("/api/v1/orgs/{}/contacts", s.acme),
        Some(json!({ "firstName": "Ada", "email": ada })),
    )
    .await;
    let mb = mailbox(&s, json!({})).await;
    let (_, first) = post(
        &s,
        &mb,
        &message(
            &ada,
            "Need a laptop",
            &format!("one.{tag}@acme.example"),
            "",
            "Mine broke.",
        ),
    )
    .await;
    let outcome = process(&s, &first).await;
    let url = format!("/api/v1/tickets/{}", outcome.ticket_id.unwrap());
    // Resolved, then a reply by subject token reopens it.
    let t = ticket(&s, outcome.ticket_id.unwrap()).await;
    s.ok(&s.owner, "POST", &format!("{url}/status"),
         Some(json!({ "status": "resolved", "resolution": "New laptop sent.", "version": t["version"] }))).await;
    let number = t["number"].as_i64().unwrap();
    let thanks = message(
        &ada,
        &format!("Re: Need a laptop [HX#{number}]"),
        &format!("three.{tag}@acme.example"),
        "",
        "It doesn't turn on.",
    );
    let (_, th) = post(&s, &mb, &thanks).await;
    assert_eq!(process(&s, &th).await.outcome, "ticket_reopened");
    assert_eq!(
        ticket(&s, outcome.ticket_id.unwrap()).await["statusCategory"],
        "open"
    );

    // Closed: a reply opens a new ticket instead.
    let t = ticket(&s, outcome.ticket_id.unwrap()).await;
    s.ok(
        &s.owner,
        "POST",
        &format!("{url}/status"),
        Some(json!({ "status": "closed", "version": t["version"] })),
    )
    .await;
    let late = message(
        &ada,
        &format!("Re: Need a laptop [HX#{number}]"),
        &format!("four.{tag}@acme.example"),
        "",
        "Hello?",
    );
    let (_, l) = post(&s, &mb, &late).await;
    let out = process(&s, &l).await;
    assert_eq!(out.outcome, "ticket_created");
    assert_ne!(out.ticket_id, outcome.ticket_id);
}

#[tokio::test]
async fn imap_mailboxes_keep_their_password_sealed() {
    let Some(s) = setup().await else { return };
    let address = format!("imap@{}.example", Uuid::now_v7().simple());
    let (status, problem) = s
        .call(
            &s.owner,
            "POST",
            "/api/v1/mailboxes",
            Some(
                json!({ "orgId": s.provider, "name": "Imap", "address": address, "kind": "imap" }),
            ),
        )
        .await;
    assert_eq!(
        (status, problem["errors"][0]["pointer"].as_str()),
        (StatusCode::UNPROCESSABLE_ENTITY, Some("/imap"))
    );
    let mb = s
        .ok(
            &s.owner,
            "POST",
            "/api/v1/mailboxes",
            Some(json!({
            "orgId": s.provider, "name": "Imap", "address": address, "kind": "imap",
            "imap": { "host": " Mail.Example.COM ", "username": "help" }, "password": "hunter2" })),
        )
        .await;
    // Defaults filled in; the password is never shown.
    assert_eq!(
        mb["imap"],
        json!({ "host": "mail.example.com", "port": 993, "security": "tls", "username": "help", "folder": "INBOX" })
    );
    assert_eq!(mb["hasPassword"], true);
    assert!(!mb.to_string().contains("hunter2"));
    let mut tx = begin_scoped(&s.pools.app, &OrgScope::System).await.unwrap();
    let id = Uuid::parse_str(mb["id"].as_str().unwrap()).unwrap();
    let sealed: Vec<u8> = sqlx::query_scalar("SELECT secret FROM sd_mailboxes WHERE id = $1")
        .bind(id)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert!(!sealed.windows(7).any(|w| w == b"hunter2"));
    let crypto =
        huxley_crypto::Crypto::new(s.pools.app.clone(), huxley_crypto::testing::key_ring());
    let plain = crypto
        .decrypt(huxley_servicedesk::mail::secret_binding(id), &sealed)
        .await
        .unwrap();
    assert_eq!(&plain[..], b"hunter2");

    // Saving without a password keeps it.
    let url = format!("/api/v1/mailboxes/{id}");
    let saved = s
        .ok(
            &s.owner,
            "PUT",
            &url,
            Some(json!({
            "name": "Renamed", "address": address, "kind": "imap", "version": mb["version"],
            "imap": { "host": "mail.example.com", "username": "help", "security": "start_tls" } })),
        )
        .await;
    assert_eq!(
        (saved["hasPassword"].clone(), saved["imap"]["port"].clone()),
        (json!(true), json!(143))
    );
}
