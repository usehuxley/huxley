//! Sales orders: from an accepted quote, through stock and purchasing, to hand-offs.

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

const PASSWORD: &str = "sales order api passphrase";

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
    for module in [
        "crm",
        "servicedesk",
        "contracts",
        "inventory",
        "sales",
        "projects",
    ] {
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
                json!({ "orgId": self.provider, "vendor": { "name": name } }),
                StatusCode::CREATED,
            )
            .await;
        v["id"].as_str().unwrap().to_owned()
    }

    /// An accepted quote's sales order.
    async fn sold(&self, lines: Value) -> Value {
        let q = self
            .ok(
                &self.owner,
                "POST",
                "/api/v1/quotes",
                json!({ "orgId": self.acme, "quote": { "title": unique("Office refresh"), "lines": lines } }),
                StatusCode::CREATED,
            )
            .await;
        let id = q["id"].as_str().unwrap();
        let sent = self
            .ok(
                &self.owner,
                "POST",
                &format!("/api/v1/quotes/{id}/send"),
                json!({ "version": q["version"] }),
                StatusCode::OK,
            )
            .await;
        let accepted = self
            .ok(
                &self.owner,
                "POST",
                &format!("/api/v1/quotes/{id}/decision"),
                json!({ "accept": true, "name": "Rosa Park", "version": sent["version"] }),
                StatusCode::OK,
            )
            .await;
        let so = accepted["salesOrderId"]
            .as_str()
            .expect("an order on acceptance")
            .to_owned();
        self.order(&so).await
    }

    async fn order(&self, id: &str) -> Value {
        let (status, o) = self
            .call(
                &self.owner,
                "GET",
                &format!("/api/v1/sales-orders/{id}"),
                None,
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{o}");
        o
    }

    async fn act(&self, o: &Value, what: &str, body: Value) -> (StatusCode, Value) {
        self.call(
            &self.owner,
            "POST",
            &format!("/api/v1/sales-orders/{}/{what}", o["id"].as_str().unwrap()),
            Some(body),
        )
        .await
    }
}

impl Setup {
    /// Submits and receives a purchase order in full.
    async fn arrive(&self, po: &str, vendor: &str, main: &str) {
        let s = self;
        let (_, p) = s
            .call(
                &s.owner,
                "GET",
                &format!("/api/v1/purchase-orders/{po}"),
                None,
            )
            .await;
        assert_eq!(p["vendorId"], json!(vendor));
        s.ok(
            &s.owner,
            "POST",
            &format!("/api/v1/purchase-orders/{po}/step"),
            json!({ "step": "submit", "version": p["version"] }),
            StatusCode::OK,
        )
        .await;
        s.ok(
            &s.owner,
            "POST",
            &format!("/api/v1/purchase-orders/{po}/receive"),
            json!({ "locationId": main, "lines": [{ "lineId": p["lines"][0]["id"] }] }),
            StatusCode::OK,
        )
        .await;
    }
}

#[tokio::test]
async fn an_order_is_filled_from_stock_and_purchase_then_handed_off() {
    let Some(s) = setup().await else { return };
    let vendor_name = unique("Dell Direct");
    let vendor = s.vendor(&vendor_name).await;
    let laptop = s.item_from("LAT-5440", "none", Some(&vendor_name)).await;
    let main = s.location("Main", "warehouse", json!({})).await;
    s.move_stock(json!({ "kind": "receive", "itemId": laptop, "locationId": main, "quantity": 1 }))
        .await;
    let (_, item) = s
        .call(
            &s.owner,
            "GET",
            &format!("/api/v1/inventory/items/{laptop}"),
            None,
        )
        .await;
    let product = item["item"]["productId"].clone();

    let o = s
        .sold(json!([
            { "productId": product, "quantity": 3, "unitPrice": "900" },
            { "name": "Setup", "quantity": 1, "unitPrice": "300" },
            { "name": "Managed laptop", "billing": "monthly", "quantity": 3, "unitPrice": "40" }
        ]))
        .await;
    assert_eq!(o["status"], "open");
    assert_eq!(o["oneTime"], "3000");
    assert_eq!(o["monthly"], "120");
    assert_eq!(o["lines"][0]["itemId"], json!(laptop));

    // One in stock; the other two are bought for the order.
    let (status, got) = s.act(&o, "allocate", json!(null)).await;
    assert_eq!(status, StatusCode::OK, "{got}");
    assert_eq!(got["made"], 1);
    assert_eq!(got["short"], json!(["LAT-5440: 2"]));
    let (_, bought) = s.act(&o, "purchase", json!(null)).await;
    assert_eq!(bought["unmatched"], json!([]));
    let po = bought["orders"][0].as_str().unwrap().to_owned();
    let now = s.order(o["id"].as_str().unwrap()).await;
    assert_eq!(
        (
            now["lines"][0]["allocated"].clone(),
            now["lines"][0]["onOrder"].clone()
        ),
        (json!("1"), json!("2"))
    );
    let (_, again) = s.act(&o, "purchase", json!(null)).await;
    assert_eq!(again["orders"], json!([]));

    // The order arrives and is allocated to the sales order.
    s.arrive(&po, &vendor, &main).await;
    let now = s.order(o["id"].as_str().unwrap()).await;
    assert_eq!(now["lines"][0]["allocated"], "3");

    // Delivered: laptops leave stock, setup is done; the monthly line waits.
    let (status, done) = s
        .act(&o, "fulfil", json!({ "version": now["version"] }))
        .await;
    assert_eq!(status, StatusCode::OK, "{done}");
    assert_eq!(done["lines"][0]["fulfilled"], "3");
    assert_eq!(done["lines"][1]["fulfilled"], "1");
    assert_eq!(done["lines"][2]["fulfilled"], "0");
    assert_eq!(done["status"], "open");
    assert_eq!(s.level(&laptop, &main).await, Value::Null);

    // The recurring line goes to an agreement, which fulfils the order.
    let (status, handed) = s
        .act(&o, "agreement", json!({ "startsOn": "2027-01-01" }))
        .await;
    assert_eq!(status, StatusCode::OK, "{handed}");
    assert_eq!(handed["status"], "fulfilled");
    let agreement = handed["agreementId"].as_str().unwrap();
    let (_, a) = s
        .call(
            &s.owner,
            "GET",
            &format!("/api/v1/agreements/{agreement}"),
            None,
        )
        .await;
    assert_eq!(a["lines"][0]["quantity"], "3");
    assert_eq!(a["status"], "draft");
    assert_eq!(
        s.events(o["id"].as_str().unwrap(), "sales_order.fulfilled")
            .await
            .len(),
        1
    );

    let (status, started) = s
        .act(&o, "project", json!({ "name": "Office refresh rollout" }))
        .await;
    assert_eq!(status, StatusCode::OK, "{started}");
    assert_eq!(started["projectName"], "Office refresh rollout");
    let (status, closed) = s
        .act(&o, "close", json!({ "version": started["version"] }))
        .await;
    assert_eq!(status, StatusCode::OK, "{closed}");
    assert_eq!(closed["status"], "closed");
}

#[tokio::test]
async fn a_cancelled_order_gives_back_what_it_set_aside() {
    let Some(s) = setup().await else { return };
    let switch = s.item("SW-24", "none").await;
    let main = s.location("Main", "warehouse", json!({})).await;
    s.move_stock(json!({ "kind": "receive", "itemId": switch, "locationId": main, "quantity": 5 }))
        .await;
    let (_, item) = s
        .call(
            &s.owner,
            "GET",
            &format!("/api/v1/inventory/items/{switch}"),
            None,
        )
        .await;
    let o = s
        .sold(
            json!([{ "productId": item["item"]["productId"], "quantity": 2, "unitPrice": "500" }]),
        )
        .await;
    s.act(&o, "allocate", json!(null)).await;
    assert_eq!(s.level(&switch, &main).await["available"], "3");
    // Can't close what isn't fulfilled; can cancel it.
    let now = s.order(o["id"].as_str().unwrap()).await;
    let (status, _) = s
        .act(&o, "close", json!({ "version": now["version"] }))
        .await;
    assert_eq!(status, StatusCode::CONFLICT);
    let (status, cancelled) = s
        .act(
            &o,
            "cancel",
            json!({ "version": now["version"], "note": "Client changed their mind" }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{cancelled}");
    assert_eq!(cancelled["status"], "cancelled");
    assert_eq!(s.level(&switch, &main).await["available"], "5");
    // A read-only user sees orders but can't act on them.
    let (_, viewer) = user(&s.pools, Some(("read_only", s.provider))).await;
    let viewer = login(&s.app, &viewer).await;
    let (status, _) = s
        .call(
            &viewer,
            "GET",
            &format!("/api/v1/sales-orders/{}", o["id"].as_str().unwrap()),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = s
        .call(
            &viewer,
            "POST",
            &format!(
                "/api/v1/sales-orders/{}/allocate",
                o["id"].as_str().unwrap()
            ),
            Some(json!(null)),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}
