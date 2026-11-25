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

/// The workflows the built-in pack ships.
const EMAIL_REPLIES: &str =
    include_str!("../../huxley-packs/builtin/servicedesk-email/workflows/email-replies.yaml");
const POLL_MAILBOXES: &str =
    include_str!("../../huxley-packs/builtin/servicedesk-email/workflows/poll-mailboxes.yaml");
const EMAIL_TO_TICKET: &str =
    include_str!("../../huxley-packs/builtin/servicedesk-email/workflows/email-to-ticket.yaml");
const NOTIFY_ASSIGNMENT: &str = include_str!(
    "../../huxley-packs/builtin/servicedesk-notifications/workflows/notify-assignment.yaml"
);
const NOTIFY_REPLY: &str = include_str!(
    "../../huxley-packs/builtin/servicedesk-notifications/workflows/notify-reply.yaml"
);
const NOTIFY_SLA: &str =
    include_str!("../../huxley-packs/builtin/servicedesk-notifications/workflows/notify-sla.yaml");
const DISPATCH_TICKETS: &str =
    include_str!("../../huxley-packs/builtin/servicedesk-dispatch/workflows/dispatch-tickets.yaml");

struct Harness {
    _db: IsolatedDb,
    pools: TestPools,
    engine: Engine,
    storage: Storage,
    /// What the engine emailed.
    sent: std::sync::Arc<std::sync::Mutex<Vec<huxley_email::Email>>>,
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
    let (mailer, sent) = huxley_email::Mailer::memory();
    let engine = Engine::with_config(pools.app.clone(), crypto, &EngineConfig::default())
        .with_storage(storage.clone())
        .with_mailer(mailer);
    Some(Harness {
        _db: db,
        pools,
        engine,
        storage,
        sent,
        provider,
        client,
    })
}

impl Harness {
    /// Runs the built-in workflow for an email, at the provider.
    async fn email_to_ticket(&self, email: Uuid) -> (RunStatus, Option<Json>, Option<Json>) {
        self.run(
            EMAIL_TO_TICKET,
            json!({ "email_id": email, "subject": "x" }),
        )
        .await
    }

    /// Runs a workflow document at the provider until it finishes.
    async fn run(&self, yaml: &str, inputs: Json) -> (RunStatus, Option<Json>, Option<Json>) {
        self.run_triggered(yaml, inputs, json!({})).await
    }

    /// Like `run`, as if started by `trigger`.
    async fn run_triggered(
        &self,
        yaml: &str,
        inputs: Json,
        trigger: Json,
    ) -> (RunStatus, Option<Json>, Option<Json>) {
        let document = serde_json::to_value(huxley_hwf::parse_yaml(yaml).unwrap()).unwrap();
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
                    trigger,
                    inputs,
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
        let mailbox = huxley_servicedesk::mail::create(
            &mut tx,
            &ctx,
            self.provider,
            Uuid::now_v7(),
            &input,
            None,
        )
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

/// A test IMAP server (`HUXLEY_TEST_IMAP`: `host:imap_port:smtp_port`, a
/// GreenMail-style server that makes accounts on delivery, with the
/// address as the password).
fn imap_server() -> Option<(String, u16, u16)> {
    let Ok(spec) = std::env::var("HUXLEY_TEST_IMAP") else {
        assert!(
            std::env::var("HUXLEY_REQUIRE_IMAP_TESTS").is_err(),
            "HUXLEY_REQUIRE_IMAP_TESTS is set but HUXLEY_TEST_IMAP isn't"
        );
        return None;
    };
    let mut parts = spec.split(':');
    let host = parts.next()?.to_owned();
    Some((
        host,
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
    ))
}

/// Delivers a message over SMTP.
fn send(host: &str, port: u16, to: &str, id: &str) {
    use std::io::{BufRead, BufReader, Write};
    let stream = std::net::TcpStream::connect((host, port)).unwrap();
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut writer = stream;
    let expect = |reader: &mut BufReader<std::net::TcpStream>| {
        let mut line = String::new();
        loop {
            line.clear();
            reader.read_line(&mut line).unwrap();
            if line.len() < 4 || &line[3..4] != "-" {
                break;
            }
        }
        assert!(line.starts_with('2') || line.starts_with('3'), "{line}");
    };
    expect(&mut reader);
    for command in [
        "EHLO test",
        "MAIL FROM:<ada@acme.example>",
        &format!("RCPT TO:<{to}>"),
        "DATA",
    ] {
        writer
            .write_all(format!("{command}\r\n").as_bytes())
            .unwrap();
        expect(&mut reader);
    }
    let body = format!(
        "From: Ada <ada@acme.example>\r\nTo: {to}\r\nSubject: Help\r\nMessage-ID: <{id}>\r\n\r\nHi\r\n.\r\n"
    );
    writer.write_all(body.as_bytes()).unwrap();
    expect(&mut reader);
    writer.write_all(b"QUIT\r\n").unwrap();
}

#[tokio::test]
async fn poll_mailboxes_fetches_new_mail_and_reports_failures() {
    let Some((host, imap_port, smtp_port)) = imap_server() else {
        return;
    };
    let Some(h) = harness().await else { return };
    let address = format!("t{}@desk.example", Uuid::now_v7().simple());
    // The account exists once it has mail; this first message is already
    // there when the mailbox is added, and unread, so it's fetched too.
    send(&host, smtp_port, &address, &format!("0.{address}"));
    let crypto =
        huxley_crypto::Crypto::new(h.pools.app.clone(), huxley_crypto::testing::key_ring());
    let ctx = RequestContext::system(Via::System);
    let mut made = Vec::new();
    for password in [address.as_str(), "wrong"] {
        let id = Uuid::now_v7();
        let sealed = crypto
            .encrypt(
                h.provider,
                huxley_servicedesk::mail::secret_binding(id),
                password.as_bytes(),
            )
            .await
            .unwrap();
        let input: huxley_servicedesk::mail::MailboxInput = serde_json::from_value(json!({
            "name": "Imap", "address": format!("in{}.{}@x.example", made.len(), Uuid::now_v7().simple()), "kind": "imap",
            "imap": { "host": host, "port": imap_port, "security": "none", "username": address },
        }))
        .unwrap();
        let mut tx = begin_scoped(&h.pools.app, &OrgScope::System).await.unwrap();
        huxley_servicedesk::mail::create(&mut tx, &ctx, h.provider, id, &input, Some(&sealed))
            .await
            .unwrap();
        tx.commit().await.unwrap();
        made.push(id);
    }
    send(&host, smtp_port, &address, &format!("1.{address}"));

    let (status, outputs, error) = h.run(POLL_MAILBOXES, json!({})).await;
    assert_eq!(status, RunStatus::Succeeded, "{error:?}");
    let report = outputs.unwrap()["mailboxes"].clone();
    let good = report
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["mailbox_id"] == json!(made[0]))
        .unwrap();
    let bad = report
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["mailbox_id"] == json!(made[1]))
        .unwrap();
    assert_eq!(
        (good["fetched"].clone(), good["new"].clone()),
        (json!(2), json!(2)),
        "{report}"
    );
    assert!(
        bad["error"]
            .as_str()
            .unwrap()
            .contains("username or password"),
        "{report}"
    );

    let mut tx = begin_scoped(&h.pools.app, &OrgScope::System).await.unwrap();
    let (emails, events, last_error): (i64, i64, Option<String>) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM sd_inbound_emails WHERE mailbox_id = $1), \
                (SELECT count(*) FROM events_outbox WHERE event_type = 'email.received'), \
                (SELECT last_error FROM sd_mailboxes WHERE id = $2)",
    )
    .bind(made[0])
    .bind(made[1])
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    assert_eq!((emails, events), (2, 2));
    assert!(last_error.unwrap().contains("username or password"));

    // Nothing new: nothing fetched; new mail: just that.
    let (_, outputs, _) = h.run(POLL_MAILBOXES, json!({})).await;
    let report = outputs.unwrap()["mailboxes"].clone();
    let good = report
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["mailbox_id"] == json!(made[0]))
        .unwrap();
    assert_eq!(good["fetched"], 0, "{report}");
    send(&host, smtp_port, &address, &format!("2.{address}"));
    let (_, outputs, _) = h.run(POLL_MAILBOXES, json!({})).await;
    let report = outputs.unwrap()["mailboxes"].clone();
    let good = report
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["mailbox_id"] == json!(made[0]))
        .unwrap();
    assert_eq!(
        (good["fetched"].clone(), good["new"].clone()),
        (json!(1), json!(1)),
        "{report}"
    );
}

/// Ada at the client, and a technician; returns the technician's id.
async fn ada_and_tech(h: &Harness) -> Uuid {
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
    let tech = Uuid::now_v7();
    sqlx::query("INSERT INTO users (id, email, display_name) VALUES ($1, $2, 'Tess Tech')")
        .bind(tech)
        .bind(format!("{}@msp.example", tech.simple()))
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    tech
}

impl Harness {
    async fn number(&self, ticket: Uuid) -> i64 {
        let mut tx = begin_scoped(&self.pools.app, &OrgScope::System)
            .await
            .unwrap();
        let n = sqlx::query_scalar("SELECT number FROM sd_tickets WHERE id = $1")
            .bind(ticket)
            .fetch_one(&mut *tx)
            .await
            .unwrap();
        tx.commit().await.unwrap();
        n
    }

    /// Delivers a raw message to the mailbox that received `earlier`.
    async fn deliver_like(&self, earlier: Uuid, raw: &str) -> Uuid {
        let mut tx = begin_scoped(&self.pools.app, &OrgScope::System)
            .await
            .unwrap();
        let mailbox_id: Uuid =
            sqlx::query_scalar("SELECT mailbox_id FROM sd_inbound_emails WHERE id = $1")
                .bind(earlier)
                .fetch_one(&mut *tx)
                .await
                .unwrap();
        let mailbox = huxley_servicedesk::mail::mailbox(&mut tx, mailbox_id)
            .await
            .unwrap();
        let raw = raw.replace('\n', "\r\n");
        let parsed = huxley_mail::parse(raw.as_bytes()).unwrap();
        let id = Uuid::now_v7();
        self.storage
            .put(
                &huxley_servicedesk::mail::raw_key(mailbox.org_id, id),
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
async fn public_notes_are_emailed_and_answers_thread_back() {
    let Some(h) = harness().await else { return };
    let tech = ada_and_tech(&h).await;
    let email = h
        .receive("From: Ada <ada@acme.example>\nSubject: VPN down\nMessage-ID: <vpn@acme.example>\n\nIt's down.\n")
        .await;
    let (_, outputs, _) = h.email_to_ticket(email).await;
    let ticket = Uuid::parse_str(outputs.unwrap()["ticket_id"].as_str().unwrap()).unwrap();

    // A public and an internal note from the technician.
    let user_ctx = RequestContext::system(Via::Ui).with_actor(huxley_core::Actor::User(tech));
    let mut tx = begin_scoped(&h.pools.app, &OrgScope::System).await.unwrap();
    let mut add = async |kind: &str, body: &str| {
        let note = huxley_servicedesk::tickets::NewNote {
            kind: kind.into(),
            body: body.into(),
        };
        huxley_servicedesk::tickets::add_note(&mut tx, &user_ctx, ticket, &note)
            .await
            .unwrap()
            .id
    };
    let public = add("public", "Restarting it now.").await;
    let internal = add("internal", "Firewall again?").await;
    tx.commit().await.unwrap();

    let (status, outputs, error) = h
        .run(EMAIL_REPLIES, json!({ "change": { "note": public } }))
        .await;
    assert_eq!(status, RunStatus::Succeeded, "{error:?}");
    assert_eq!(outputs.unwrap()["sent"], true);
    let sent = h.sent.lock().unwrap().clone();
    assert_eq!(sent.len(), 1);
    let mail = &sent[0];
    assert_eq!(mail.to, "ada@acme.example");
    let number = h.number(ticket).await;
    assert_eq!(mail.subject, format!("Re: VPN down [HX#{number}]"));
    assert!(
        mail.text
            .starts_with("Restarting it now.\n\nTess Tech\nHelp"),
        "{}",
        mail.text
    );
    let reply = mail.reply.clone().unwrap();
    assert_eq!(reply.in_reply_to, ["vpn@acme.example"]);
    assert!(reply.from_address.starts_with("help@"));
    assert_eq!(
        reply.message_id,
        format!(
            "hx.{public}@{}",
            reply.from_address.split('@').nth(1).unwrap()
        )
    );

    // Internal notes and repeats aren't emailed.
    for note in [internal, public] {
        let (_, outputs, _) = h
            .run(EMAIL_REPLIES, json!({ "change": { "note": note } }))
            .await;
        assert_eq!(outputs.unwrap()["sent"], false);
    }
    assert_eq!(h.sent.lock().unwrap().len(), 1);

    // Ada answers our message (no token in the subject): it threads back.
    let answer = format!(
        "From: ada@acme.example\nSubject: Re: thanks\nMessage-ID: <ans@acme.example>\nIn-Reply-To: <{}>\n\nWorks now.\n",
        reply.message_id
    );
    let id = h.deliver_like(email, &answer).await;
    let (_, outputs, _) = h.email_to_ticket(id).await;
    let outputs = outputs.unwrap();
    assert_eq!(
        (outputs["outcome"].as_str(), outputs["ticket_id"].clone()),
        (Some("note_added"), json!(ticket))
    );
}

#[tokio::test]
async fn email_replies_only_starts_for_public_notes_written_in_huxley() {
    let Some(h) = harness().await else { return };
    let document = huxley_hwf::parse_yaml(EMAIL_REPLIES).unwrap();
    let Some(huxley_hwf::Trigger::Event(trigger)) = document.spec.triggers.first() else {
        panic!("an event trigger");
    };
    assert_eq!(trigger.event, "ticket.note_added");
    let filter = trigger.filter.as_deref().unwrap();
    let passes = |change: Json| {
        let event = json!({ "payload": { "reference": "T-1", "change": change } });
        let scope = huxley_expr::Scope::new().with("event", &event);
        h.engine.expr().eval_condition(filter, &scope).unwrap()
    };
    assert!(passes(json!({ "kind": "public", "note": "n1" })));
    assert!(!passes(json!({ "kind": "internal", "note": "n1" })));
    assert!(!passes(
        json!({ "kind": "public", "note": "n1", "email": "e1", "from": "ada@acme.example" })
    ));
}

/// A workflow document around `spec` (the `spec:` body).
fn workflow(spec: &str) -> String {
    format!(
        "apiVersion: usehuxley.io/v1\nkind: Workflow\nmetadata: {{ key: t, name: T }}\nspec:\n{spec}"
    )
}

#[tokio::test]
async fn ticket_steps_create_find_update_and_note() {
    let Some(h) = harness().await else { return };
    let (status, outputs, error) = h
        .run(
            &workflow(
                r#"  inputs: { type: object, properties: { client: { type: string } } }
  steps:
    - id: open
      type: core.ticket.create
      with: { org_id: "{{ inputs.client }}", title: "Disk full on FS01", impact: 1, urgency: 1 }
    - id: note
      type: core.ticket.note
      with: { ticket_id: "{{ steps.open.output.ticket.id }}", body: "Cleared temp files." }
    - id: found
      type: core.ticket.find
      with: { search: "Disk full on FS01" }
    - id: progress
      type: core.ticket.update
      with: { ticket_id: "{{ steps.open.output.ticket.id }}", status: in_progress, urgency: 3 }
    - id: resolve
      type: core.ticket.update
      with: { ticket_id: "{{ steps.open.output.ticket.id }}", status: resolved, resolution: "Expanded the volume." }
    - id: read
      type: core.ticket.get
      with: { ticket_id: "{{ steps.open.output.ticket.id }}", notes: true }
  outputs:
    source: "{{ steps.open.output.ticket.source }}"
    priority: "{{ steps.open.output.ticket.priority }}"
    found: "{{ steps.found.output.total }}"
    retargeted: "{{ steps.progress.output.ticket.priority }}"
    status: "{{ steps.read.output.ticket.statusKey }}"
    notes: "{{ steps.read.output.notes | map(attribute='kind') | list }}"
"#,
            ),
            json!({ "client": h.client }),
        )
        .await;
    assert_eq!(status, RunStatus::Succeeded, "{error:?}");
    assert_eq!(
        outputs.unwrap(),
        json!({
            "source": "workflow",
            "priority": "p1",
            "found": 1,
            "retargeted": "p3",
            "status": "resolved",
            "notes": ["internal", "resolution"],
        })
    );
}

#[tokio::test]
async fn ticket_steps_stay_in_their_organization_and_refuse_secrets() {
    let Some(h) = harness().await else { return };
    // A ticket at another provider, out of this run's reach.
    let other = org(&h.pools, ensure_root(&h.pools).await, "provider").await;
    let ctx = RequestContext::system(Via::System);
    let mut tx = begin_scoped(&h.pools.app, &OrgScope::System).await.unwrap();
    let elsewhere = huxley_servicedesk::tickets::create(
        &mut tx,
        &ctx,
        &huxley_servicedesk::tickets::NewTicket {
            org_id: other,
            title: "Theirs".into(),
            ..Default::default()
        },
        "T",
        None,
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    let (status, _, error) = h
        .run(
            &workflow(&format!(
                "  steps:\n    - id: peek\n      type: core.ticket.get\n      with: {{ ticket_id: \"{}\" }}\n",
                elsewhere.id
            )),
            json!({}),
        )
        .await;
    assert_eq!(status, RunStatus::Failed);
    assert_eq!(error.unwrap()["kind"], "not_found");

    let (status, _, error) = h
        .run(
            &workflow(
                "  steps:\n    - id: leak\n      type: core.ticket.create\n      with: { title: \"{{ secrets.generate() }}\" }\n",
            ),
            json!({}),
        )
        .await;
    assert_eq!(status, RunStatus::Failed);
    assert!(error.unwrap().to_string().contains("secrets"));
}

async fn enable(h: &Harness, module: &str) {
    let ctx = RequestContext::system(Via::System);
    let mut tx = begin_scoped(&h.pools.app, &OrgScope::System).await.unwrap();
    huxley_modules::set_enabled(&mut tx, &ctx, module, true, false)
        .await
        .unwrap();
    tx.commit().await.unwrap();
}

/// Runs SQL in system scope.
async fn exec(h: &Harness, sql: &str, binds: &[Uuid]) {
    let mut tx = begin_scoped(&h.pools.app, &OrgScope::System).await.unwrap();
    let mut q = sqlx::query(sqlx::AssertSqlSafe(sql.to_owned()));
    for b in binds {
        q = q.bind(*b);
    }
    q.execute(&mut *tx).await.unwrap();
    tx.commit().await.unwrap();
}

async fn person(h: &Harness, name: &str) -> Uuid {
    let id = Uuid::now_v7();
    let mut tx = begin_scoped(&h.pools.app, &OrgScope::System).await.unwrap();
    sqlx::query("INSERT INTO users (id, email, display_name) VALUES ($1, $2, $3)")
        .bind(id)
        .bind(format!("{}@msp.example", id.simple()))
        .bind(name)
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    id
}

/// A queue at the provider with `dispatch`, and these people (and a team
/// of `team`) as members.
async fn queue(h: &Harness, dispatch: &str, people: &[Uuid], team: &[Uuid]) -> Uuid {
    let ctx = RequestContext::system(Via::System);
    let mut tx = begin_scoped(&h.pools.app, &OrgScope::System).await.unwrap();
    let team_id = Uuid::now_v7();
    sqlx::query("INSERT INTO teams (id, name) VALUES ($1, $2)")
        .bind(team_id)
        .bind(unique("Team"))
        .execute(&mut *tx)
        .await
        .unwrap();
    for user in team {
        sqlx::query("INSERT INTO team_members (team_id, user_id) VALUES ($1, $2)")
            .bind(team_id)
            .bind(user)
            .execute(&mut *tx)
            .await
            .unwrap();
    }
    let input: huxley_servicedesk::queues::QueueInput = serde_json::from_value(json!({
        "name": unique("Queue"),
        "dispatch": dispatch,
        "userIds": people,
        "teamIds": if team.is_empty() { json!([]) } else { json!([team_id]) },
    }))
    .unwrap();
    let q = huxley_servicedesk::queues::create(&mut tx, &ctx, h.provider, &input)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    q.id
}

/// A new ticket at the client, in `queue`.
async fn ticket_in(h: &Harness, queue: Option<Uuid>) -> Uuid {
    let ctx = RequestContext::system(Via::System);
    let mut tx = begin_scoped(&h.pools.app, &OrgScope::System).await.unwrap();
    let t = huxley_servicedesk::tickets::create(
        &mut tx,
        &ctx,
        &huxley_servicedesk::tickets::NewTicket {
            org_id: h.client,
            title: "Printer jammed".into(),
            queue_id: queue,
            ..Default::default()
        },
        "T",
        None,
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    t.id
}

const DISPATCH_STEP: &str = r#"  inputs: { type: object, properties: { ticket: { type: string }, strategy: {} } }
  steps:
    - id: d
      type: core.ticket.dispatch
      with: { ticket_id: "{{ inputs.ticket }}", strategy: "{{ inputs.strategy }}" }
  outputs:
    result: "{{ steps.d.output }}"
"#;

impl Harness {
    /// Dispatches a ticket; the outcome.
    async fn dispatch(&self, ticket: Uuid, strategy: Option<&str>) -> Json {
        let (status, outputs, error) = self
            .run(
                &workflow(DISPATCH_STEP),
                json!({ "ticket": ticket, "strategy": strategy }),
            )
            .await;
        assert_eq!(status, RunStatus::Succeeded, "{error:?}");
        outputs.unwrap()["result"].clone()
    }

    /// Who got it, by name.
    async fn assignee(&self, ticket: Uuid, strategy: Option<&str>) -> Json {
        let out = self.dispatch(ticket, strategy).await;
        assert_eq!(out["reason"], "dispatched", "{out}");
        out["assignee"]["name"].clone()
    }
}

#[tokio::test]
async fn dispatch_takes_turns_or_follows_load() {
    let Some(h) = harness().await else { return };
    let (amy, ben, cy) = (
        person(&h, "Amy").await,
        person(&h, "Ben").await,
        person(&h, "Cy").await,
    );
    // Cy is a member through a team.
    let q = queue(&h, "round_robin", &[amy, ben], &[cy]).await;
    let mut got = Vec::new();
    for _ in 0..4 {
        got.push(h.assignee(ticket_in(&h, Some(q)).await, None).await);
    }
    assert_eq!(got, [json!("Amy"), json!("Ben"), json!("Cy"), json!("Amy")]);

    // Ben gets two more by hand (not his turn taken). By load: Ben has 3,
    // Amy 2, Cy 1, though Ben is next in turn.
    for _ in 0..2 {
        let extra = ticket_in(&h, None).await;
        exec(
            &h,
            "UPDATE sd_tickets SET assignee_user_id = $2 WHERE id = $1",
            &[extra, ben],
        )
        .await;
    }
    let t = ticket_in(&h, Some(q)).await;
    assert_eq!(h.assignee(t, Some("load")).await, "Cy");
    // Without workforce, skills falls back to load: Amy and Cy have 2, and
    // Amy has waited longer.
    assert_eq!(
        h.assignee(ticket_in(&h, Some(q)).await, Some("skills"))
            .await,
        "Amy"
    );

    // Already assigned, no queue, a manual queue: nobody is chosen.
    assert_eq!(h.dispatch(t, None).await["reason"], "assigned");
    assert_eq!(
        h.dispatch(ticket_in(&h, None).await, None).await["reason"],
        "no_queue"
    );
    let manual = queue(&h, "manual", &[amy], &[]).await;
    let out = h.dispatch(ticket_in(&h, Some(manual)).await, None).await;
    assert_eq!(
        (out["reason"].as_str(), out["assigned"].as_bool()),
        (Some("manual"), Some(false))
    );
    // An empty queue: nobody available.
    let empty = queue(&h, "load", &[], &[]).await;
    assert_eq!(
        h.dispatch(ticket_in(&h, Some(empty)).await, None).await["reason"],
        "nobody_available"
    );
}

#[tokio::test]
async fn dispatch_by_skills_skips_people_who_cant_take_it() {
    let Some(h) = harness().await else { return };
    enable(&h, "workforce").await;
    let (amy, ben, cy, dee) = (
        person(&h, "Amy").await,
        person(&h, "Ben").await,
        person(&h, "Cy").await,
        person(&h, "Dee").await,
    );
    let q = queue(&h, "skills", &[amy, ben, cy, dee], &[]).await;
    let (skill, cert) = (Uuid::now_v7(), Uuid::now_v7());
    exec(
        &h,
        "INSERT INTO wf_skills (id, name) VALUES ($1, 'Printers')",
        &[skill],
    )
    .await;
    exec(
        &h,
        "INSERT INTO wf_certifications (id, vendor, name) VALUES ($1, 'HP', 'Printer tech')",
        &[cert],
    )
    .await;
    exec(
        &h,
        "INSERT INTO wf_requirements (id, target_type, target_id, skill_id, min_level) VALUES (gen_random_uuid(), 'queue', $1, $2, 3)",
        &[q, skill],
    )
    .await;
    exec(
        &h,
        "INSERT INTO wf_requirements (id, target_type, target_id, certification_id) VALUES (gen_random_uuid(), 'queue', $1, $2)",
        &[q, cert],
    )
    .await;
    // Amy: certified, a beginner. Ben: certified expert, off sick today.
    // Cy: an expert without the certification. Dee: certified, proficient.
    for (who, level, certified) in [
        (amy, 1, true),
        (ben, 4, true),
        (cy, 4, false),
        (dee, 3, true),
    ] {
        exec(
            &h,
            &format!(
                "INSERT INTO wf_user_skills (user_id, skill_id, level) VALUES ($1, $2, {level})"
            ),
            &[who, skill],
        )
        .await;
        if certified {
            exec(
                &h,
                "INSERT INTO wf_user_certifications (id, user_id, certification_id, earned_on) VALUES (gen_random_uuid(), $1, $2, current_date - 10)",
                &[who, cert],
            )
            .await;
        }
    }
    exec(
        &h,
        "INSERT INTO wf_time_off (id, user_id, starts_on, ends_on, kind, status) \
         VALUES (gen_random_uuid(), $1, current_date - 1, current_date + 1, 'sick', 'approved')",
        &[ben],
    )
    .await;
    let out = h.dispatch(ticket_in(&h, Some(q)).await, None).await;
    assert_eq!(out["assignee"]["name"], "Dee", "{out}");
    let skipped: Vec<(Json, Json)> = out["considered"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| (c["name"].clone(), c["skipped"]["reason"].clone()))
        .collect();
    assert!(
        skipped.contains(&(json!("Ben"), json!("away"))),
        "{skipped:?}"
    );
    assert!(
        skipped.contains(&(json!("Cy"), json!("unqualified"))),
        "{skipped:?}"
    );
    // Dee off shift now (no hours on any day but an impossible minute): Amy.
    exec(
        &h,
        r#"INSERT INTO wf_schedules (user_id, timezone, hours) VALUES ($1, 'UTC', '{"mon": [["00:00", "00:01"]]}')"#,
        &[dee],
    )
    .await;
    assert_eq!(h.assignee(ticket_in(&h, Some(q)).await, None).await, "Amy");
    // Dee is back (no hours: any time). Round-robin still skips Ben and Cy,
    // and Dee has waited longer than Amy.
    exec(&h, "DELETE FROM wf_schedules WHERE user_id = $1", &[dee]).await;
    let mut got = Vec::new();
    for _ in 0..2 {
        got.push(
            h.assignee(ticket_in(&h, Some(q)).await, Some("round_robin"))
                .await,
        );
    }
    assert_eq!(got, [json!("Dee"), json!("Amy")]);
}

#[tokio::test]
async fn the_dispatch_workflow_assigns_new_and_moved_tickets() {
    let Some(h) = harness().await else { return };
    let document = huxley_hwf::parse_yaml(DISPATCH_TICKETS).unwrap();
    let filters: Vec<(String, String)> = document
        .spec
        .triggers
        .iter()
        .map(|t| match t {
            huxley_hwf::Trigger::Event(e) => (e.event.clone(), e.filter.clone().unwrap()),
            _ => panic!("event triggers"),
        })
        .collect();
    let passes = |event: &str, payload: Json| {
        let (_, filter) = filters.iter().find(|(e, _)| e == event).unwrap();
        let event = json!({ "payload": payload });
        let scope = huxley_expr::Scope::new().with("event", &event);
        h.engine.expr().eval_condition(filter, &scope).unwrap()
    };
    let q = json!(Uuid::now_v7());
    assert!(passes(
        "ticket.created",
        json!({ "queueId": q, "assigneeUserId": null, "change": {} })
    ));
    assert!(!passes(
        "ticket.created",
        json!({ "queueId": null, "assigneeUserId": null, "change": {} })
    ));
    assert!(!passes(
        "ticket.created",
        json!({ "queueId": q, "assigneeUserId": q, "change": {} })
    ));
    let moved = json!({ "queue": { "from": "A", "to": "B" } });
    assert!(passes(
        "ticket.updated",
        json!({ "queueId": q, "assigneeUserId": null, "change": moved })
    ));
    assert!(!passes(
        "ticket.updated",
        json!({ "queueId": q, "assigneeUserId": null, "change": { "title": {} } })
    ));

    let amy = person(&h, "Amy").await;
    let queue = queue(&h, "round_robin", &[amy], &[]).await;
    let ticket = ticket_in(&h, Some(queue)).await;
    let (status, outputs, error) = h
        .run_triggered(
            DISPATCH_TICKETS,
            json!({}),
            json!({ "type": "ticket.created", "subject_type": "ticket", "subject_id": ticket }),
        )
        .await;
    assert_eq!(status, RunStatus::Succeeded, "{error:?}");
    let outputs = outputs.unwrap();
    assert_eq!(
        (
            outputs["assigned"].clone(),
            outputs["assignee"]["name"].clone()
        ),
        (json!(true), json!("Amy"))
    );
}

/// Each event trigger's filter in a workflow, by event.
fn filters(yaml: &str) -> Vec<(String, String)> {
    huxley_hwf::parse_yaml(yaml)
        .unwrap()
        .spec
        .triggers
        .iter()
        .map(|t| match t {
            huxley_hwf::Trigger::Event(e) => (e.event.clone(), e.filter.clone().unwrap()),
            _ => panic!("event triggers"),
        })
        .collect()
}

impl Harness {
    fn passes(&self, yaml: &str, event: &str, payload: &Json) -> bool {
        let found = filters(yaml);
        let (_, filter) = found.iter().find(|(e, _)| e == event).unwrap();
        let event = json!({ "type": event, "payload": payload });
        let scope = huxley_expr::Scope::new().with("event", &event);
        self.engine.expr().eval_condition(filter, &scope).unwrap()
    }

    /// Runs a notification workflow for an event about `ticket`; what each
    /// person was sent: (user, kind, title).
    async fn notified(
        &self,
        yaml: &str,
        event: &str,
        ticket: Uuid,
        payload: Json,
    ) -> Vec<(Uuid, String, String)> {
        let (status, _, error) = self
            .run_triggered(
                yaml,
                payload,
                json!({ "type": event, "subject_type": "ticket", "subject_id": ticket }),
            )
            .await;
        assert_eq!(status, RunStatus::Succeeded, "{error:?}");
        sqlx::query_as(
            "SELECT user_id, kind, title FROM notifications WHERE link = $1 ORDER BY created_at, user_id",
        )
        .bind(format!("/tickets/{ticket}"))
        .fetch_all(&self.pools.owner)
        .await
        .unwrap()
    }
}

#[tokio::test]
async fn assignment_notifies_the_assignee_or_their_team_but_not_whoever_did_it() {
    let Some(h) = harness().await else { return };
    let (amy, ben) = (person(&h, "Amy").await, person(&h, "Ben").await);
    let ticket = ticket_in(&h, None).await;
    let assigned = |user: Option<Uuid>, team: Option<Uuid>, actor: Option<Uuid>, change: Json| {
        json!({ "reference": "T-1", "title": "Printer jammed", "assigneeUserId": user,
                "assigneeTeamId": team, "actorId": actor, "change": change })
    };
    // Which changes start it.
    let who = json!({ "assignee": { "from": null, "to": "Amy" } });
    assert!(h.passes(
        NOTIFY_ASSIGNMENT,
        "ticket.assigned",
        &assigned(Some(amy), None, None, who.clone())
    ));
    assert!(!h.passes(
        NOTIFY_ASSIGNMENT,
        "ticket.assigned",
        &assigned(None, None, None, who.clone())
    ));
    let team = Uuid::now_v7();
    let to_team = json!({ "team": { "from": null, "to": "Desk" } });
    assert!(h.passes(
        NOTIFY_ASSIGNMENT,
        "ticket.updated",
        &assigned(None, Some(team), None, to_team.clone())
    ));
    // A team change while a person has it: they've been told already.
    assert!(!h.passes(
        NOTIFY_ASSIGNMENT,
        "ticket.updated",
        &assigned(Some(amy), Some(team), None, to_team.clone())
    ));
    assert!(!h.passes(
        NOTIFY_ASSIGNMENT,
        "ticket.updated",
        &assigned(Some(amy), None, None, json!({ "title": {} }))
    ));
    assert!(h.passes(
        NOTIFY_ASSIGNMENT,
        "ticket.created",
        &assigned(Some(amy), None, None, json!({}))
    ));

    // Dispatch (no actor) assigns Amy: she's told.
    let sent = h
        .notified(
            NOTIFY_ASSIGNMENT,
            "ticket.assigned",
            ticket,
            assigned(Some(amy), None, None, who.clone()),
        )
        .await;
    assert_eq!(
        sent,
        [(amy, "ticket.assigned".into(), "T-1 assigned to you".into())]
    );
    // Amy takes another herself: nobody is told.
    let other = ticket_in(&h, None).await;
    let nobody: Vec<(Uuid, String, String)> = Vec::new();
    assert_eq!(
        h.notified(
            NOTIFY_ASSIGNMENT,
            "ticket.assigned",
            other,
            assigned(Some(amy), None, Some(amy), who.clone())
        )
        .await,
        nobody
    );
    // A team: everyone in it, except whoever assigned it.
    exec(
        &h,
        "INSERT INTO teams (id, name) VALUES ($1, 'Desk ' || $1::text)",
        &[team],
    )
    .await;
    for member in [amy, ben] {
        exec(
            &h,
            "INSERT INTO team_members (team_id, user_id) VALUES ($1, $2)",
            &[team, member],
        )
        .await;
    }
    let third = ticket_in(&h, None).await;
    let sent = h
        .notified(
            NOTIFY_ASSIGNMENT,
            "ticket.updated",
            third,
            assigned(None, Some(team), Some(amy), to_team),
        )
        .await;
    assert_eq!(
        sent,
        [(
            ben,
            "ticket.assigned".into(),
            "T-1 assigned to your team".into()
        )]
    );
}

#[tokio::test]
async fn replies_and_sla_alerts_notify_the_assignee() {
    let Some(h) = harness().await else { return };
    let amy = person(&h, "Amy").await;
    let reply = |user: Option<Uuid>, change: Json| {
        json!({ "reference": "T-2", "title": "VPN drops", "assigneeUserId": user,
                "assigneeTeamId": null, "actorId": null, "change": change })
    };
    let emailed =
        json!({ "kind": "public", "note": "n1", "email": "e1", "from": "ada@acme.example" });
    assert!(h.passes(
        NOTIFY_REPLY,
        "ticket.note_added",
        &reply(Some(amy), emailed.clone())
    ));
    // Written in Huxley, internal, or nobody to tell: no.
    assert!(!h.passes(
        NOTIFY_REPLY,
        "ticket.note_added",
        &reply(Some(amy), json!({ "kind": "public", "note": "n1" }))
    ));
    assert!(!h.passes(
        NOTIFY_REPLY,
        "ticket.note_added",
        &reply(
            Some(amy),
            json!({ "kind": "internal", "note": "n1", "email": "e1" })
        )
    ));
    assert!(!h.passes(
        NOTIFY_REPLY,
        "ticket.note_added",
        &reply(None, emailed.clone())
    ));
    let ticket = ticket_in(&h, None).await;
    let sent = h
        .notified(
            NOTIFY_REPLY,
            "ticket.note_added",
            ticket,
            reply(Some(amy), emailed),
        )
        .await;
    assert_eq!(
        sent,
        [(
            amy,
            "ticket.replied".into(),
            "T-2: the requester replied".into()
        )]
    );

    let alert = |clock: &str| {
        json!({ "reference": "T-3", "title": "Server down", "priority": "p1", "clock": clock,
                "assigneeUserId": amy, "assigneeTeamId": null })
    };
    assert!(!h.passes(
        NOTIFY_SLA,
        "ticket.sla_warning",
        &json!({ "assigneeUserId": null, "assigneeTeamId": null })
    ));
    let late = ticket_in(&h, None).await;
    h.notified(NOTIFY_SLA, "ticket.sla_warning", late, alert("response"))
        .await;
    let sent = h
        .notified(NOTIFY_SLA, "ticket.sla_breached", late, alert("resolution"))
        .await;
    let titles: Vec<&str> = sent.iter().map(|(_, _, t)| t.as_str()).collect();
    assert_eq!(
        titles,
        [
            "T-3 is close to its first response target",
            "T-3 missed its resolution target"
        ]
    );
    assert!(sent.iter().all(|(u, k, _)| *u == amy && k == "ticket.sla"));
}

#[tokio::test]
async fn notify_checks_its_kind_and_exclusions() {
    let Some(h) = harness().await else { return };
    let amy = person(&h, "Amy").await;
    for (with, problem) in [
        (
            format!("{{ to: \"user:{amy}\", title: Hi, kind: security.password_changed }}"),
            "kind workflows can send",
        ),
        (
            format!("{{ to: \"user:{amy}\", title: Hi, exclude: [\"nobody\"] }}"),
            "isn't a user id",
        ),
    ] {
        let (status, _, error) = h
            .run(
                &workflow(&format!(
                    "  steps:\n    - id: n\n      type: core.notify\n      with: {with}\n"
                )),
                json!({}),
            )
            .await;
        assert_eq!(status, RunStatus::Failed, "{with}");
        assert!(error.unwrap().to_string().contains(problem), "{with}");
    }
}

#[tokio::test]
async fn replies_go_out_through_the_mailboxs_own_smtp_server() {
    let Some((host, imap_port, smtp_port)) = imap_server() else {
        return;
    };
    let Some(h) = harness().await else { return };
    let tech = ada_and_tech(&h).await;
    let email = h
        .receive("From: Ada <ada@acme.example>\nSubject: Printer\nMessage-ID: <printer@acme.example>\n\nJammed.\n")
        .await;
    let (_, outputs, _) = h.email_to_ticket(email).await;
    let ticket = Uuid::parse_str(outputs.unwrap()["ticket_id"].as_str().unwrap()).unwrap();
    // The mailbox sends through its own server (GreenMail, no login).
    let ctx = RequestContext::system(Via::System);
    let mut tx = begin_scoped(&h.pools.app, &OrgScope::System).await.unwrap();
    let mailbox_id: Uuid =
        sqlx::query_scalar("SELECT mailbox_id FROM sd_inbound_emails WHERE id = $1")
            .bind(email)
            .fetch_one(&mut *tx)
            .await
            .unwrap();
    let mailbox = huxley_servicedesk::mail::mailbox(&mut tx, mailbox_id)
        .await
        .unwrap();
    let input: huxley_servicedesk::mail::MailboxInput = serde_json::from_value(json!({
        "name": mailbox.name, "address": mailbox.address, "kind": "forward", "version": mailbox.version,
        "smtp": { "host": host, "port": smtp_port, "security": "none" },
    }))
    .unwrap();
    let mailbox = huxley_servicedesk::mail::update(&mut tx, &ctx, mailbox_id, &input, None)
        .await
        .unwrap();
    assert_eq!(mailbox.smtp.as_ref().and_then(|s| s.port), Some(smtp_port));
    let user_ctx = RequestContext::system(Via::Ui).with_actor(huxley_core::Actor::User(tech));
    let note = huxley_servicedesk::tickets::add_note(
        &mut tx,
        &user_ctx,
        ticket,
        &huxley_servicedesk::tickets::NewNote {
            kind: "public".into(),
            body: "Cleared it.".into(),
        },
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();

    let (status, outputs, error) = h
        .run(EMAIL_REPLIES, json!({ "change": { "note": note.id } }))
        .await;
    assert_eq!(status, RunStatus::Succeeded, "{error:?}");
    assert_eq!(outputs.unwrap()["sent"], true);
    // Not through the instance's mailer…
    assert!(h.sent.lock().unwrap().is_empty());
    // …but delivered by the mailbox's server, from the mailbox, threaded.
    let token = format!("[HX#{}]", h.number(ticket).await);
    let config = huxley_mail::imap::Config {
        host,
        port: imap_port,
        security: huxley_mail::imap::Security::None,
        username: "ada@acme.example".into(),
        password: "ada@acme.example".into(),
        folder: "INBOX".into(),
    };
    let deadline = Instant::now() + Duration::from_secs(10);
    let raw = loop {
        let (messages, _) = huxley_mail::imap::fetch(
            &config,
            huxley_mail::imap::Cursor::default(),
            200,
            Duration::from_secs(10),
        )
        .await
        .unwrap();
        if let Some(m) = messages
            .iter()
            .map(|m| String::from_utf8_lossy(&m.raw).into_owned())
            .find(|raw| raw.contains(&token))
        {
            break m;
        }
        assert!(Instant::now() < deadline, "the reply never arrived");
        tokio::time::sleep(Duration::from_millis(200)).await;
    };
    assert!(
        raw.contains(&format!("From: Help <{}>", mailbox.address)),
        "{raw}"
    );
    assert!(raw.contains("In-Reply-To: <printer@acme.example>"), "{raw}");
    assert!(raw.contains("Cleared it."), "{raw}");
}
