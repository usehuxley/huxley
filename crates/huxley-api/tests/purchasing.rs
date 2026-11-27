//! Purchasing: vendors, purchase orders, approval, receiving and reordering.

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

const PASSWORD: &str = "purchasing api passphrase";

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
        self.item_from(sku, tracking, None).await
    }

    async fn item_from(&self, sku: &str, tracking: &str, vendor: Option<&str>) -> String {
        let p = self
            .ok(
                &self.owner,
                "POST",
                "/api/v1/catalog/products",
                json!({ "orgId": self.provider, "product": {
                    "sku": unique(sku).replace(' ', "-"), "name": sku, "kind": "product",
                    "cost": "3", "price": "5", "unit": "each", "vendor": vendor } }),
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

impl Setup {
    async fn vendor(&self, name: &str) -> String {
        let v = self
            .ok(
                &self.owner,
                "POST",
                "/api/v1/vendors",
                json!({ "orgId": self.provider, "vendor": { "name": name, "email": "orders@example.com" } }),
                StatusCode::CREATED,
            )
            .await;
        v["id"].as_str().unwrap().to_owned()
    }

    async fn order(&self, body: Value) -> Value {
        self.ok(
            &self.owner,
            "POST",
            "/api/v1/purchase-orders",
            json!({ "orgId": self.provider, "order": body }),
            StatusCode::CREATED,
        )
        .await
    }

    async fn step(
        &self,
        cookie: &str,
        order: &Value,
        step: &str,
        comment: Option<&str>,
    ) -> (StatusCode, Value) {
        let (_, now) = self
            .call(
                cookie,
                "GET",
                &format!("/api/v1/purchase-orders/{}", order["id"].as_str().unwrap()),
                None,
            )
            .await;
        self.call(
            cookie,
            "POST",
            &format!(
                "/api/v1/purchase-orders/{}/step",
                order["id"].as_str().unwrap()
            ),
            Some(json!({ "step": step, "version": now["version"], "comment": comment })),
        )
        .await
    }

    async fn receive(&self, order: &Value, body: Value) -> (StatusCode, Value) {
        self.call(
            &self.owner,
            "POST",
            &format!(
                "/api/v1/purchase-orders/{}/receive",
                order["id"].as_str().unwrap()
            ),
            Some(body),
        )
        .await
    }
}

#[tokio::test]
async fn an_order_over_the_threshold_is_approved_ordered_and_received_in_parts() {
    let Some(s) = setup().await else { return };
    let vendor = s.vendor(&unique("Ingram")).await;
    let drives = s.item("SSD-1TB", "none").await;
    let main = s.location("Main", "warehouse", json!({})).await;
    s.ok(
        &s.owner,
        "PUT",
        "/api/v1/purchase-orders/settings",
        json!({ "orgId": s.provider, "approvalAbove": "40" }),
        StatusCode::OK,
    )
    .await;
    let o = s
        .order(json!({ "vendorId": vendor, "shipTo": main, "lines": [
            { "itemId": drives, "quantity": 10 },
            { "name": "Shipping", "quantity": 1, "unitCost": "15" }
        ] }))
        .await;
    // The item's catalog cost filled in.
    assert_eq!(o["lines"][0]["unitCost"], "3");
    assert_eq!(o["total"], "45");
    assert_eq!(o["status"], "draft");

    // Over 40: it waits for approval, which technicians can't give.
    let (status, sub) = s.step(&s.owner, &o, "submit", None).await;
    assert_eq!(status, StatusCode::OK, "{sub}");
    assert_eq!(sub["status"], "submitted");
    let (_, tech) = user(&s.pools, Some(("technician", s.provider))).await;
    let tech = login(&s.app, &tech).await;
    let (status, _) = s.step(&tech, &o, "approve", None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = s.step(&s.owner, &o, "reject", None).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    let (_, ok) = s.step(&s.owner, &o, "approve", None).await;
    assert_eq!(ok["status"], "approved");
    let (_, sent) = s.step(&s.owner, &o, "order", Some("SO-99")).await;
    assert_eq!(sent["status"], "ordered");
    assert_eq!(sent["vendorReference"], "SO-99");
    // Sent orders don't change.
    let (status, _) = s
        .call(
            &s.owner,
            "PUT",
            &format!("/api/v1/purchase-orders/{}", o["id"].as_str().unwrap()),
            Some(json!({ "vendorId": vendor, "version": sent["version"] })),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT);

    // Four drives arrive, then the rest and the shipping line.
    let line = |n: usize| o["lines"][n]["id"].clone();
    let (status, part) = s
        .receive(
            &o,
            json!({ "lines": [{ "lineId": line(0), "quantity": 4 }] }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{part}");
    assert_eq!(part["status"], "partially_received");
    assert_eq!(s.level(&drives, &main).await["onHand"], "4");
    let (status, _) = s
        .receive(
            &o,
            json!({ "lines": [{ "lineId": line(0), "quantity": 7 }] }),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    let (_, done) = s
        .receive(
            &o,
            json!({ "lines": [{ "lineId": line(0) }, { "lineId": line(1) }], "reference": "DN-1" }),
        )
        .await;
    assert_eq!(done["status"], "received");
    assert_eq!(s.level(&drives, &main).await["onHand"], "10");
    let (_, moves) = s
        .call(
            &s.owner,
            "GET",
            &format!("/api/v1/inventory/movements?itemId={drives}"),
            None,
        )
        .await;
    assert_eq!(moves[0]["refType"], "purchase_order");
    assert_eq!(moves[0]["reference"], "DN-1");
    assert_eq!(moves[0]["unitCost"], "3");
    let (status, _) = s.step(&s.owner, &o, "cancel", None).await;
    assert_eq!(status, StatusCode::CONFLICT);
    let (_, receipts) = s
        .call(
            &s.owner,
            "GET",
            &format!(
                "/api/v1/purchase-orders/{}/receipts",
                o["id"].as_str().unwrap()
            ),
            None,
        )
        .await;
    assert_eq!(receipts.as_array().unwrap().len(), 3);
    let id = o["id"].as_str().unwrap();
    assert_eq!(s.events(id, "purchase_order.received").await.len(), 1);
}

#[tokio::test]
async fn parts_bought_for_a_ticket_are_allocated_on_arrival() {
    let Some(s) = setup().await else { return };
    let vendor = s.vendor(&unique("TD Synnex")).await;
    let dock = s.item("DOCK-USB-C", "serial").await;
    let main = s.location("Main", "warehouse", json!({})).await;
    let ticket = s.ticket().await;
    let o = s
        .order(json!({ "vendorId": vendor, "shipTo": main, "lines": [
            { "itemId": dock, "quantity": 1, "unitCost": "120", "forRefType": "ticket", "forRefId": ticket }
        ] }))
        .await;
    // Under any threshold: approved straight away.
    let (_, ok) = s.step(&s.owner, &o, "submit", None).await;
    assert_eq!(ok["status"], "approved");
    let serial = format!("{dock}-1");
    let (status, got) = s
        .receive(
            &o,
            json!({ "lines": [{ "lineId": o["lines"][0]["id"], "serials": [serial] }] }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{got}");
    assert_eq!(got["status"], "received");
    let (_, parts) = s
        .call(
            &s.owner,
            "GET",
            &format!("/api/v1/inventory/allocations?refType=ticket&refId={ticket}"),
            None,
        )
        .await;
    assert_eq!(parts[0]["serials"], json!([serial]));
    assert_eq!(parts[0]["status"], "allocated");

    // A draft can be deleted; a vendor with orders can't be.
    let draft = s
        .order(json!({ "vendorId": vendor, "lines": [{ "name": "Cable", "quantity": 2 }] }))
        .await;
    let (status, _) = s
        .call(
            &s.owner,
            "DELETE",
            &format!("/api/v1/purchase-orders/{}", draft["id"].as_str().unwrap()),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = s
        .call(
            &s.owner,
            "DELETE",
            &format!("/api/v1/vendors/{vendor}"),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT);
}

#[tokio::test]
async fn low_stock_is_reordered_from_its_vendor() {
    let Some(s) = setup().await else { return };
    let name = unique("Pax8");
    let vendor = s.vendor(&name).await;
    let toner = s.item_from("TONER-K", "none", Some(&name)).await;
    let main = s.location("Main", "warehouse", json!({})).await;
    s.move_stock(json!({ "kind": "receive", "itemId": toner, "locationId": main, "quantity": 2 }))
        .await;
    s.ok(
        &s.owner,
        "PUT",
        &format!("/api/v1/inventory/items/{toner}/reorder/{main}"),
        json!({ "reorderPoint": "3", "reorderQty": "12" }),
        StatusCode::NO_CONTENT,
    )
    .await;
    let (_, suggested) = s
        .call(
            &s.owner,
            "GET",
            &format!("/api/v1/purchase-orders/suggestions?orgId={}", s.provider),
            None,
        )
        .await;
    let mine = suggested
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["itemId"] == toner.as_str())
        .cloned()
        .unwrap();
    assert_eq!(mine["quantity"], "12");
    assert_eq!(mine["vendorId"], json!(vendor));

    let made = s
        .ok(
            &s.owner,
            "POST",
            "/api/v1/purchase-orders/reorder",
            json!({ "orgId": s.provider }),
            StatusCode::OK,
        )
        .await;
    let po = made
        .as_array()
        .unwrap()
        .iter()
        .find(|o| o["vendorId"] == vendor.as_str())
        .cloned()
        .unwrap();
    assert_eq!(po["lines"][0]["quantity"], "12");
    assert_eq!(po["shipTo"], json!(main));
    // On order now, so it isn't suggested again.
    let (_, again) = s
        .call(
            &s.owner,
            "GET",
            &format!("/api/v1/purchase-orders/suggestions?orgId={}", s.provider),
            None,
        )
        .await;
    assert!(
        !again
            .as_array()
            .unwrap()
            .iter()
            .any(|x| x["itemId"] == toner.as_str())
    );
}
