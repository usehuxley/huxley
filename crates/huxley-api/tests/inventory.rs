//! The inventory API: locations, items, movements, serials and allocations.

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

const PASSWORD: &str = "inventory api passphrase";

struct Setup {
    pools: TestPools,
    app: axum::Router,
    provider: Uuid,
    acme: Uuid,
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
    let ctx = RequestContext::system(Via::System);
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    for module in ["crm", "servicedesk", "contracts", "inventory"] {
        huxley_modules::set_enabled(&mut tx, &ctx, module, true, false)
            .await
            .unwrap();
    }
    tx.commit().await.unwrap();
    let provider = org(&pools, root, huxley_orgs::OrgKind::Provider, "Stock MSP").await;
    let acme = org(&pools, provider, huxley_orgs::OrgKind::Client, "Acme").await;
    let owner = login(&app, &user(&pools, None).await.1).await;
    Some(Setup {
        pools,
        app,
        provider,
        acme,
        owner,
    })
}

/// A user, optionally with a built-in role at an organization.
async fn user(pools: &TestPools, role_at: Option<(&str, Uuid)>) -> (Uuid, String) {
    let email = format!("{}@example.com", unique("stock").replace(' ', "."));
    let ctx = RequestContext::system(Via::System);
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    let u = huxley_auth::create_user(&mut tx, &ctx, &email, "Stock User", PASSWORD)
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

    async fn ok(
        &self,
        cookie: &str,
        method: &str,
        uri: &str,
        body: Value,
        want: StatusCode,
    ) -> Value {
        let (status, v) = self.call(cookie, method, uri, Some(body)).await;
        assert_eq!(status, want, "{method} {uri}: {v}");
        v
    }

    async fn item(&self, sku: &str, tracking: &str) -> String {
        let p = self
            .ok(
                &self.owner,
                "POST",
                "/api/v1/catalog/products",
                json!({ "orgId": self.provider, "product": {
                    "sku": unique(sku).replace(' ', "-"), "name": sku, "kind": "product",
                    "cost": "2", "price": "5", "unit": "each" } }),
                StatusCode::CREATED,
            )
            .await;
        let item = self
            .ok(
                &self.owner,
                "POST",
                "/api/v1/inventory/items",
                json!({ "orgId": self.provider, "item": { "productId": p["id"], "tracking": tracking } }),
                StatusCode::CREATED,
            )
            .await;
        item["id"].as_str().unwrap().to_owned()
    }

    async fn location(&self, name: &str, kind: &str, extra: Value) -> String {
        let mut loc = json!({ "name": unique(name), "kind": kind });
        if let (Some(l), Some(e)) = (loc.as_object_mut(), extra.as_object()) {
            l.extend(e.clone());
        }
        let l = self
            .ok(
                &self.owner,
                "POST",
                "/api/v1/inventory/locations",
                json!({ "orgId": self.provider, "location": loc }),
                StatusCode::CREATED,
            )
            .await;
        l["id"].as_str().unwrap().to_owned()
    }

    async fn move_stock(&self, body: Value) -> (StatusCode, Value) {
        self.call(
            &self.owner,
            "POST",
            "/api/v1/inventory/movements",
            Some(body),
        )
        .await
    }

    async fn level(&self, item: &str, location: &str) -> Value {
        let (_, detail) = self
            .call(
                &self.owner,
                "GET",
                &format!("/api/v1/inventory/items/{item}"),
                None,
            )
            .await;
        detail["levels"]
            .as_array()
            .unwrap()
            .iter()
            .find(|l| l["locationId"] == location)
            .cloned()
            .unwrap_or(Value::Null)
    }

    async fn ticket(&self) -> String {
        let t = self
            .ok(
                &self.owner,
                "POST",
                "/api/v1/tickets",
                json!({ "orgId": self.acme, "title": "Replace the switch" }),
                StatusCode::CREATED,
            )
            .await;
        t["id"].as_str().unwrap().to_owned()
    }

    async fn events(&self, subject: &str, event: &str) -> Vec<Value> {
        let mut tx = begin_scoped(&self.pools.app, &OrgScope::System)
            .await
            .unwrap();
        let rows: Vec<Value> = sqlx::query_scalar(
            "SELECT payload FROM events_outbox WHERE event_type = $1 AND subject_id = $2 ORDER BY occurred_at, id",
        )
        .bind(event)
        .bind(Uuid::parse_str(subject).unwrap())
        .fetch_all(&mut *tx)
        .await
        .unwrap();
        tx.commit().await.unwrap();
        rows
    }
}

#[tokio::test]
async fn stock_is_received_moved_allocated_and_used() {
    let Some(s) = setup().await else { return };
    let cable = s.item("CAT6-3M", "none").await;
    let main = s.location("Main", "warehouse", json!({})).await;
    let van = s.location("Van", "vehicle", json!({})).await;
    s.ok(
        &s.owner,
        "PUT",
        &format!("/api/v1/inventory/items/{cable}/reorder/{main}"),
        json!({ "reorderPoint": "5", "reorderQty": "20" }),
        StatusCode::NO_CONTENT,
    )
    .await;
    // Nothing on hand yet, so it's already low.
    assert_eq!(s.events(&cable, "stock.low").await.len(), 1);

    let (status, m) = s
        .move_stock(
            json!({ "kind": "receive", "itemId": cable, "locationId": main, "quantity": "10",
            "unitCost": "2.5", "reference": "PO-1" }),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{m}");
    assert!(m["toLocationName"].as_str().unwrap().starts_with("Main"));
    assert_eq!(s.level(&cable, &main).await["onHand"], "10");
    assert_eq!(s.level(&cable, &main).await["low"], false);

    let (status, _) = s
        .move_stock(
            json!({ "kind": "transfer", "itemId": cable, "locationId": main, "toLocationId": van,
            "quantity": 4 }),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(s.level(&cable, &van).await["onHand"], "4");

    // Allocated to a ticket, then used: it leaves stock, and Main dips low.
    let ticket = s.ticket().await;
    let a = s
        .ok(
            &s.owner,
            "POST",
            "/api/v1/inventory/allocations",
            json!({ "itemId": cable, "locationId": main, "quantity": "2", "refType": "ticket", "refId": ticket }),
            StatusCode::CREATED,
        )
        .await;
    assert_eq!(a["status"], "allocated");
    assert_eq!(a["refOrgId"], json!(s.acme));
    let level = s.level(&cable, &main).await;
    assert_eq!(
        (level["onHand"].clone(), level["available"].clone()),
        (json!("6"), json!("4"))
    );
    assert_eq!(level["low"], true);
    assert_eq!(s.events(&cable, "stock.low").await.len(), 2);
    // Can't move what's allocated.
    let (status, err) = s
        .move_stock(
            json!({ "kind": "transfer", "itemId": cable, "locationId": main, "toLocationId": van,
            "quantity": 5 }),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{err}");
    let id = a["id"].as_str().unwrap();
    let used = s
        .ok(
            &s.owner,
            "POST",
            &format!("/api/v1/inventory/allocations/{id}/consume"),
            json!(null),
            StatusCode::OK,
        )
        .await;
    assert_eq!(used["status"], "consumed");
    assert_eq!(s.level(&cable, &main).await["onHand"], "4");
    let consumed = s.events(&cable, "inventory.consumed").await;
    assert_eq!(consumed[0]["unitCost"], "2.5");
    assert_eq!(consumed[0]["refId"], json!(ticket));
    let (status, _) = s
        .call(
            &s.owner,
            "POST",
            &format!("/api/v1/inventory/allocations/{id}/release"),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT);

    let (_, on_ticket) = s
        .call(
            &s.owner,
            "GET",
            &format!("/api/v1/inventory/allocations?refType=ticket&refId={ticket}"),
            None,
        )
        .await;
    assert_eq!(on_ticket.as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn serial_numbers_are_followed_one_by_one() {
    let Some(s) = setup().await else { return };
    let laptop = s.item("LAPTOP-14", "serial").await;
    let main = s.location("Main", "warehouse", json!({})).await;
    let shelf = s
        .location("Shelf A", "bin", json!({ "parentId": main }))
        .await;
    let serial = |n: &str| format!("{laptop}-{n}");

    let (status, m) = s
        .move_stock(
            json!({ "kind": "receive", "itemId": laptop, "locationId": main,
            "serials": [serial("A"), serial("B"), serial("C")] }),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{m}");
    assert_eq!(m["quantity"], "3");
    let (status, _) = s
        .move_stock(json!({ "kind": "receive", "itemId": laptop, "locationId": main, "serials": [serial("A")] }))
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);

    // Moved to a bin, allocated to a ticket, used; one goes back to the vendor.
    s.move_stock(
        json!({ "kind": "transfer", "itemId": laptop, "locationId": main, "toLocationId": shelf,
        "serials": [serial("B")] }),
    )
    .await;
    let ticket = s.ticket().await;
    let (status, a) = s
        .call(
            &s.owner,
            "POST",
            "/api/v1/inventory/allocations",
            Some(
                json!({ "itemId": laptop, "locationId": main, "serials": [serial("B")],
                "refType": "ticket", "refId": ticket }),
            ),
        )
        .await;
    assert_eq!(
        status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "B is on the shelf: {a}"
    );
    let a = s
        .ok(
            &s.owner,
            "POST",
            "/api/v1/inventory/allocations",
            json!({ "itemId": laptop, "locationId": shelf, "serials": [serial("B")],
                "refType": "ticket", "refId": ticket, "consume": true }),
            StatusCode::CREATED,
        )
        .await;
    assert_eq!(a["status"], "consumed");
    let (status, _) = s
        .move_stock(
            json!({ "kind": "rma", "itemId": laptop, "locationId": main, "serials": [serial("C")],
            "reference": "RMA-7" }),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);

    let (_, units) = s
        .call(
            &s.owner,
            "GET",
            &format!("/api/v1/inventory/units?itemId={laptop}"),
            None,
        )
        .await;
    let status_of = |n: &str| {
        units
            .as_array()
            .unwrap()
            .iter()
            .find(|u| u["serial"] == serial(n))
            .unwrap()["status"]
            .clone()
    };
    assert_eq!(status_of("A"), "in_stock");
    assert_eq!(status_of("B"), "consumed");
    assert_eq!(status_of("C"), "rma");
    let (_, found) = s
        .call(
            &s.owner,
            "GET",
            &format!("/api/v1/inventory/units?serial={}", serial("B")),
            None,
        )
        .await;
    assert_eq!(found[0]["refId"], json!(ticket));
    // Back from the job into stock.
    let (status, _) = s
        .move_stock(json!({ "kind": "return", "itemId": laptop, "locationId": main, "serials": [serial("B")] }))
        .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(s.level(&laptop, &main).await["onHand"], "2");
}

#[tokio::test]
async fn technicians_take_parts_and_only_configurers_keep_locations() {
    let Some(s) = setup().await else { return };
    let (tech_id, tech) = user(&s.pools, Some(("technician", s.provider))).await;
    let tech = login(&s.app, &tech).await;
    let (_, viewer) = user(&s.pools, Some(("read_only", s.provider))).await;
    let viewer = login(&s.app, &viewer).await;
    let screws = s.item("SCREW-M3", "lot").await;
    let van = s
        .location("Tech van", "vehicle", json!({ "assignedUserId": tech_id }))
        .await;

    // Lot items name the lot.
    let (status, _) = s
        .move_stock(
            json!({ "kind": "receive", "itemId": screws, "locationId": van, "quantity": 100 }),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    s.ok(
        &s.owner,
        "POST",
        "/api/v1/inventory/movements",
        json!({ "kind": "receive", "itemId": screws, "locationId": van, "quantity": 100, "lot": "L-2026-11" }),
        StatusCode::CREATED,
    )
    .await;
    let lots = s.level(&screws, &van).await["lots"].clone();
    assert_eq!(lots[0]["lot"], "L-2026-11");

    let ticket = s.ticket().await;
    let take = json!({ "itemId": screws, "locationId": van, "quantity": 4, "lot": "L-2026-11",
        "refType": "ticket", "refId": ticket, "consume": true });
    s.ok(
        &tech,
        "POST",
        "/api/v1/inventory/allocations",
        take.clone(),
        StatusCode::CREATED,
    )
    .await;
    let (status, _) = s
        .call(&viewer, "POST", "/api/v1/inventory/allocations", Some(take))
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, items) = s
        .call(&viewer, "GET", "/api/v1/inventory/items", None)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        items
            .as_array()
            .unwrap()
            .iter()
            .any(|i| i["id"] == screws.as_str())
    );
    let (status, _) = s
        .call(
            &tech,
            "POST",
            "/api/v1/inventory/locations",
            Some(json!({ "orgId": s.provider, "location": { "name": unique("Mine"), "kind": "vehicle" } })),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // A used location stays, turned off; an unused one goes.
    let (status, _) = s
        .call(
            &s.owner,
            "DELETE",
            &format!("/api/v1/inventory/locations/{van}"),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT);
    let spare = s.location("Spare", "warehouse", json!({})).await;
    let (status, _) = s
        .call(
            &s.owner,
            "DELETE",
            &format!("/api/v1/inventory/locations/{spare}"),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn adjustments_say_why_and_the_ledger_keeps_everything() {
    let Some(s) = setup().await else { return };
    let cable = s.item("CAT6-1M", "none").await;
    let van = s.location("Van", "vehicle", json!({})).await;
    let (status, _) = s
        .move_stock(json!({ "kind": "receive", "itemId": cable, "locationId": van, "quantity": 4 }))
        .await;
    assert_eq!(status, StatusCode::CREATED);
    // Adjustments say why; the ledger has it all.
    let (status, _) = s
        .move_stock(
            json!({ "kind": "adjust", "itemId": cable, "locationId": van, "quantity": "-1" }),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    let (status, _) = s
        .move_stock(
            json!({ "kind": "adjust", "itemId": cable, "locationId": van, "quantity": "-1",
            "note": "Counted 3" }),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    let (_, ledger) = s
        .call(
            &s.owner,
            "GET",
            &format!("/api/v1/inventory/movements?itemId={cable}"),
            None,
        )
        .await;
    let kinds: Vec<&str> = ledger
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["kind"].as_str().unwrap())
        .collect();
    assert_eq!(kinds, ["adjust", "receive"]);
    assert_eq!(s.level(&cable, &van).await["onHand"], "3");
}

#[tokio::test]
async fn serial_items_name_each_unit() {
    let Some(s) = setup().await else { return };
    let laptop = s.item("LAPTOP-13", "serial").await;
    let main = s.location("Main", "warehouse", json!({})).await;
    let serial = |n: &str| format!("{laptop}-{n}");
    // Serial items name their units; the count must match.
    let (status, _) = s
        .move_stock(
            json!({ "kind": "receive", "itemId": laptop, "locationId": main, "quantity": 2 }),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    let (status, _) = s
        .move_stock(
            json!({ "kind": "receive", "itemId": laptop, "locationId": main, "quantity": 3,
            "serials": [serial("A"), serial("B")] }),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
}
