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
                    trigger: json!({}),
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
