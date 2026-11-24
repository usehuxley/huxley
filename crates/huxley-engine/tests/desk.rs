//! The service desk steps in real runs (ADR-038): the built-in *Email to
//! ticket* workflow turns a received email into a ticket with its
//! attachments, once, and only where the module is on.

use std::time::{Duration, Instant};

use huxley_core::{RequestContext, Via};
use huxley_db::testing::{IsolatedDb, TestPools, ensure_root, isolated, unique};
use huxley_db::{OrgScope, begin_scoped};
use huxley_engine::{Engine, EngineConfig, RunStatus, StartRun};
use huxley_storage::Storage;
use serde_json::{Value as Json, json};
use uuid::Uuid;

/// The workflow the built-in pack ships.
const EMAIL_TO_TICKET: &str =
    include_str!("../../huxley-packs/builtin/servicedesk-email/workflows/email-to-ticket.yaml");

struct Harness {
    _db: IsolatedDb,
    pools: TestPools,
    engine: Engine,
    storage: Storage,
    provider: Uuid,
    client: Uuid,
}

async fn org(pools: &TestPools, parent: Uuid, kind: &str) -> Uuid {
    let id = Uuid::now_v7();
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    sqlx::query(
        "INSERT INTO organizations (id, parent_id, kind, name, path) VALUES ($1, $2, $3::org_kind, $4, '')",
    )
    .bind(id)
    .bind(parent)
    .bind(kind)
    .bind(unique("desk"))
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    id
}

async fn harness() -> Option<Harness> {
    let db = isolated("desk").await?;
    let pools = db.pools.clone();
    let root = ensure_root(&pools).await;
    let provider = org(&pools, root, "provider").await;
    let client = org(&pools, provider, "client").await;
    let ctx = RequestContext::system(Via::System);
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    for module in ["crm", "servicedesk"] {
        huxley_modules::set_enabled(&mut tx, &ctx, module, true, false)
            .await
            .unwrap();
    }
    tx.commit().await.unwrap();
    let crypto = huxley_crypto::Crypto::new(pools.app.clone(), huxley_crypto::testing::key_ring());
    let storage = Storage::memory();
    let engine = Engine::with_config(pools.app.clone(), crypto, &EngineConfig::default())
        .with_storage(storage.clone());
    Some(Harness {
        _db: db,
        pools,
        engine,
        storage,
        provider,
        client,
    })
}

impl Harness {
    /// Runs the built-in workflow for an email, at the provider.
    async fn email_to_ticket(&self, email: Uuid) -> (RunStatus, Option<Json>, Option<Json>) {
        let document =
            serde_json::to_value(huxley_hwf::parse_yaml(EMAIL_TO_TICKET).unwrap()).unwrap();
        let mut tx = begin_scoped(&self.pools.app, &OrgScope::System)
            .await
            .unwrap();
        let id = self
            .engine
            .start_run(
                &mut tx,
                StartRun {
                    org_id: self.provider,
                    workflow_id: None,
                    version: None,
                    document: Some(document),
                    trigger_type: "event".into(),
                    trigger: json!({}),
                    inputs: json!({ "email_id": email, "subject": "x" }),
                    actor_id: None,
                },
            )
            .await
            .unwrap();
        tx.commit().await.unwrap();
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            self.engine.drain("desk-test", 50).await.unwrap();
            let (status, outputs, error): (RunStatus, Option<Json>, Option<Json>) =
                sqlx::query_as("SELECT status, outputs, error FROM runs WHERE id = $1")
                    .bind(id)
                    .fetch_one(&self.pools.owner)
                    .await
                    .unwrap();
            if status.is_finished() {
                return (status, outputs, error);
            }
            assert!(Instant::now() < deadline, "run didn't finish");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    /// Delivers a raw message to a new mailbox at the provider.
    async fn receive(&self, raw: &str) -> Uuid {
        let ctx = RequestContext::system(Via::System);
        let mut tx = begin_scoped(&self.pools.app, &OrgScope::System)
            .await
            .unwrap();
        let input: huxley_servicedesk::mail::MailboxInput = serde_json::from_value(json!({
            "name": "Help", "address": format!("help@{}.example", Uuid::now_v7().simple())
        }))
        .unwrap();
        let mailbox = huxley_servicedesk::mail::create(&mut tx, &ctx, self.provider, &input)
            .await
            .unwrap();
        let raw = raw.replace('\n', "\r\n");
        let parsed = huxley_mail::parse(raw.as_bytes()).unwrap();
        let id = Uuid::now_v7();
        self.storage
            .put(
                &huxley_servicedesk::mail::raw_key(self.provider, id),
                raw.clone().into(),
            )
            .await
            .unwrap();
        let (id, _) = huxley_servicedesk::mail::store(&mut tx, &mailbox, id, &parsed, raw.len())
            .await
            .unwrap();
        tx.commit().await.unwrap();
        id
    }
}

#[tokio::test]
async fn the_built_in_workflow_opens_a_ticket_with_its_attachments() {
    let Some(h) = harness().await else { return };
    let ctx = RequestContext::system(Via::System);
    let mut tx = begin_scoped(&h.pools.app, &OrgScope::System).await.unwrap();
    let contact = huxley_crm::contacts::ContactInput {
        first_name: "Ada".into(),
        email: Some("ada@acme.example".into()),
        ..Default::default()
    };
    huxley_crm::contacts::create(&mut tx, &ctx, h.client, &contact)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    let email = h
        .receive(
            "From: Ada <ada@acme.example>\nSubject: Logs attached\nMessage-ID: <logs@acme.example>\n\
MIME-Version: 1.0\nContent-Type: multipart/mixed; boundary=XX\n\n\
--XX\nContent-Type: text/plain\n\nSee the log.\n\
--XX\nContent-Type: text/plain; name=log.txt\nContent-Disposition: attachment; filename=log.txt\n\
Content-Transfer-Encoding: base64\n\naGVsbG8=\n--XX--\n",
        )
        .await;
    let (status, outputs, error) = h.email_to_ticket(email).await;
    assert_eq!(status, RunStatus::Succeeded, "{error:?}");
    let outputs = outputs.unwrap();
    assert_eq!(outputs["outcome"], "ticket_created");
    let ticket = Uuid::parse_str(outputs["ticket_id"].as_str().unwrap()).unwrap();

    let mut tx = begin_scoped(&h.pools.app, &OrgScope::System).await.unwrap();
    let files = huxley_entity::attachments::of_entity(&mut tx, huxley_entity::TICKET, ticket)
        .await
        .unwrap();
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].filename, "log.txt");
    let content = huxley_entity::attachments::content(&h.storage, &files[0])
        .await
        .unwrap();
    assert_eq!(&content[..], b"hello");
    let org: Uuid = sqlx::query_scalar("SELECT org_id FROM sd_tickets WHERE id = $1")
        .bind(ticket)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert_eq!(org, h.client);
    tx.commit().await.unwrap();

    // Again (say, the event delivered twice): the same result, nothing added.
    let (status, outputs, _) = h.email_to_ticket(email).await;
    assert_eq!(status, RunStatus::Succeeded);
    assert_eq!(outputs.unwrap()["ticket_id"], json!(ticket));
    let mut tx = begin_scoped(&h.pools.app, &OrgScope::System).await.unwrap();
    let (tickets, files): (i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM sd_tickets), \
                (SELECT count(*) FROM attachments WHERE entity_type = 'ticket')",
    )
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    assert_eq!((tickets, files), (1, 1));
}

#[tokio::test]
async fn the_step_needs_the_module() {
    let Some(h) = harness().await else { return };
    let email = h
        .receive("From: b@b.example\nSubject: Hi\nMessage-ID: <hi@b.example>\n\nHello\n")
        .await;
    let ctx = RequestContext::system(Via::System);
    let mut tx = begin_scoped(&h.pools.app, &OrgScope::System).await.unwrap();
    huxley_modules::set_for_org(&mut tx, &ctx, h.provider, "servicedesk", false)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let (status, _, error) = h.email_to_ticket(email).await;
    assert_eq!(status, RunStatus::Failed);
    assert!(error.unwrap().to_string().contains("module_disabled"));
}
