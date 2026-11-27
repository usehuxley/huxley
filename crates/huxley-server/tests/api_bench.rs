//! API benchmark at a reference size (docs/performance.md): an MSP with
//! client organizations, technicians and a history of tickets, under a mix
//! of what technicians do all day. Ignored by default (a measurement, not a
//! check). `scripts/bench.sh` runs it with resource limits:
//!
//! ```sh
//! BENCH_SIZE=small HUXLEY_TEST_DATABASE_URL=… \
//!   cargo test --release -p huxley-server --test api_bench -- --ignored --nocapture
//! ```

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use huxley_core::{RequestContext, Via};
use huxley_db::testing::{TestPools, ensure_root};
use huxley_db::{DatabaseConfig, OrgScope, begin_scoped};
use secrecy::SecretString;
use serde_json::json;
use uuid::Uuid;

use common::{Database, TEST_KEK, huxley_command};

const PASSWORD: &str = "benchmark passphrase";

#[derive(Debug, Clone, Copy)]
struct Size {
    name: &'static str,
    clients: usize,
    technicians: usize,
    tickets: usize,
    api_processes: usize,
    users: usize,
    seconds: u64,
}

fn size() -> Size {
    match std::env::var("BENCH_SIZE").as_deref().unwrap_or("small") {
        "medium" => Size {
            name: "medium",
            clients: 150,
            technicians: 50,
            tickets: 150_000,
            api_processes: 2,
            users: 50,
            seconds: 60,
        },
        "large" => Size {
            name: "large",
            clients: 500,
            technicians: 200,
            tickets: 600_000,
            api_processes: 4,
            users: 150,
            seconds: 60,
        },
        _ => Size {
            name: "small",
            clients: 25,
            technicians: 10,
            tickets: 20_000,
            api_processes: 1,
            users: 10,
            seconds: 60,
        },
    }
}

/// What a technician does, and how often (out of 100).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Op {
    ListOpen,
    ListMine,
    GetTicket,
    ListNotes,
    MyWork,
    AddNote,
    CreateTicket,
}

const MIX: [(Op, usize); 7] = [
    (Op::ListOpen, 25),
    (Op::ListMine, 15),
    (Op::GetTicket, 25),
    (Op::ListNotes, 15),
    (Op::MyWork, 10),
    (Op::AddNote, 7),
    (Op::CreateTicket, 3),
];

impl Op {
    fn name(self) -> &'static str {
        match self {
            Self::ListOpen => "list open tickets",
            Self::ListMine => "list my tickets",
            Self::GetTicket => "open a ticket",
            Self::ListNotes => "read a ticket's notes",
            Self::MyWork => "My work",
            Self::AddNote => "add a note",
            Self::CreateTicket => "create a ticket",
        }
    }
}

/// Seeds the MSP: client orgs, technicians (the Technician role at the
/// provider, inherited), and tickets with two notes each, spread over a
/// year. Returns (provider, client org ids, technician emails, ticket ids).
async fn seed(pools: &TestPools, size: Size) -> (Uuid, Vec<Uuid>, Vec<String>, Vec<Uuid>) {
    let root = ensure_root(pools).await;
    let ctx = RequestContext::system(Via::System);
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    huxley_modules::set_enabled(&mut tx, &ctx, "servicedesk", true, true)
        .await
        .unwrap();
    let provider = huxley_orgs::create(
        &mut tx,
        &ctx,
        root,
        huxley_orgs::OrgKind::Provider,
        "Bench MSP",
    )
    .await
    .unwrap()
    .id;
    let mut clients = Vec::new();
    for n in 0..size.clients {
        clients.push(
            huxley_orgs::create(
                &mut tx,
                &ctx,
                provider,
                huxley_orgs::OrgKind::Client,
                &format!("Client {n:04}"),
            )
            .await
            .unwrap()
            .id,
        );
    }
    let mut emails = Vec::new();
    let mut ids = Vec::new();
    for n in 0..size.technicians {
        let email = format!("tech{n:04}@bench.example");
        let user =
            huxley_auth::create_user(&mut tx, &ctx, &email, &format!("Tech {n:04}"), PASSWORD)
                .await
                .unwrap();
        emails.push(email);
        ids.push(user.id);
    }
    tx.commit().await.unwrap();
    let tickets = seed_history(pools, size, provider, &clients, &ids).await;
    (provider, clients, emails, tickets)
}

/// The technicians' role, then a year of tickets with notes. Returns a
/// sample of ticket ids to open.
async fn seed_history(
    pools: &TestPools,
    size: Size,
    provider: Uuid,
    clients: &[Uuid],
    ids: &[Uuid],
) -> Vec<Uuid> {
    sqlx::query(
        "INSERT INTO role_assignments (id, user_id, role_id, org_id, inherit) \
         SELECT gen_random_uuid(), u, (SELECT id FROM roles WHERE key = 'technician' AND is_system), $2, true \
           FROM unnest($1::uuid[]) AS u",
    )
    .bind(ids)
    .bind(provider)
    .execute(&pools.owner)
    .await
    .unwrap();
    // Tickets: a fifth open, the rest resolved or closed, like a real desk.
    sqlx::query(
        "WITH s AS (SELECT t.key, st.key AS status, st.id AS status_id, t.id AS type_id \
                      FROM sd_statuses st JOIN sd_ticket_types t ON t.id = st.type_id WHERE t.key = 'incident') \
         INSERT INTO sd_tickets (id, org_id, type_id, status_id, title, description, impact, urgency, priority, \
                                 assignee_user_id, created_at, updated_at, source) \
         SELECT gen_random_uuid(), \
                ($1::uuid[])[1 + (i % array_length($1::uuid[], 1))], \
                (SELECT type_id FROM s LIMIT 1), \
                (SELECT status_id FROM s WHERE status = CASE WHEN i % 10 = 0 THEN 'new' \
                                                         WHEN i % 10 = 1 THEN 'in_progress' \
                                                         WHEN i % 10 = 2 THEN 'waiting_client' \
                                                         WHEN i % 10 < 6 THEN 'resolved' ELSE 'closed' END), \
                'Ticket ' || i || ': printer on floor ' || (i % 12) || ' is offline again', \
                repeat('The user reports the issue happens every morning after login. ', 4), \
                1 + i % 3, 1 + (i / 3) % 3, (ARRAY['p1','p2','p3','p4'])[1 + i % 4], \
                ($2::uuid[])[1 + (i % array_length($2::uuid[], 1))], \
                now() - make_interval(mins => (i::bigint * 525600 / $3)::int), \
                now() - make_interval(mins => (i::bigint * 525600 / $3)::int), 'web' \
           FROM generate_series(1, $3) AS i",
    )
    .bind(clients)
    .bind(ids)
    .bind(i32::try_from(size.tickets).unwrap())
    .execute(&pools.owner)
    .await
    .unwrap();
    // Resolved and closed tickets carry their timestamps, as status changes set them.
    sqlx::query(
        "UPDATE sd_tickets t SET resolved_at = t.created_at + interval '1 day', \
                closed_at = CASE WHEN s.category = 'closed' THEN t.created_at + interval '3 days' END \
           FROM sd_statuses s WHERE s.id = t.status_id AND s.category IN ('resolved', 'closed')",
    )
    .execute(&pools.owner)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO sd_ticket_notes (id, ticket_id, org_id, kind, body, author_user_id, created_at) \
         SELECT gen_random_uuid(), t.id, t.org_id, k.kind, 'Checked the spooler and restarted it; asked the user to confirm.', \
                t.assignee_user_id, t.created_at + interval '1 hour' \
           FROM sd_tickets t CROSS JOIN (VALUES ('public'), ('internal')) AS k(kind)",
    )
    .execute(&pools.owner)
    .await
    .unwrap();
    sqlx::query("ANALYZE").execute(&pools.owner).await.unwrap();
    let sample: Vec<Uuid> =
        sqlx::query_scalar("SELECT id FROM sd_tickets TABLESAMPLE SYSTEM (5) LIMIT 2000")
            .fetch_all(&pools.owner)
            .await
            .unwrap();
    sample
}

async fn sign_in(http: &reqwest::Client, base: &str, email: &str) -> String {
    let res = http
        .post(format!("{base}/api/v1/auth/login"))
        .header("x-requested-with", "huxley")
        .json(&json!({ "email": email, "password": PASSWORD }))
        .send()
        .await
        .unwrap();
    assert!(
        res.status().is_success(),
        "sign-in failed: {}",
        res.status()
    );
    let cookie = res.headers()["set-cookie"].to_str().unwrap();
    cookie.split(';').next().unwrap().to_owned()
}

fn pick(n: usize) -> Op {
    let mut at = n % 100;
    for (op, weight) in MIX {
        if at < weight {
            return op;
        }
        at -= weight;
    }
    Op::GetTicket
}

#[derive(Default)]
struct Samples {
    by_op: Vec<(Op, Duration)>,
    errors: usize,
}

#[allow(clippy::too_many_arguments)]
async fn user_loop(
    http: reqwest::Client,
    base: String,
    cookie: String,
    tickets: Arc<Vec<Uuid>>,
    clients: Arc<Vec<Uuid>>,
    until: Instant,
    seed: usize,
    counter: Arc<AtomicUsize>,
) -> Samples {
    let mut out = Samples::default();
    let mut n = seed;
    while Instant::now() < until {
        n = n
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        let op = pick(n >> 33);
        let ticket = tickets[(n >> 13) % tickets.len()];
        let request = match op {
            Op::ListOpen => http.get(format!("{base}/api/v1/tickets?limit=50")),
            Op::ListMine => http.get(format!("{base}/api/v1/tickets?assignee=me&limit=50")),
            Op::GetTicket => http.get(format!("{base}/api/v1/tickets/{ticket}")),
            Op::ListNotes => http.get(format!("{base}/api/v1/tickets/{ticket}/notes")),
            Op::MyWork => http.get(format!("{base}/api/v1/my-work")),
            Op::AddNote => http
                .post(format!("{base}/api/v1/tickets/{ticket}/notes"))
                .json(&json!({ "kind": "internal", "body": "Following up with the user this afternoon." })),
            Op::CreateTicket => http.post(format!("{base}/api/v1/tickets")).json(&json!({
                "orgId": clients[(n >> 17) % clients.len()],
                "title": "Outlook keeps asking for the password",
                "description": "Started after the password change this morning.",
            })),
        };
        let started = Instant::now();
        let result = request
            .header("x-requested-with", "huxley")
            .header("cookie", &cookie)
            .send()
            .await;
        let ok = match result {
            Ok(res) => {
                let ok = res.status().is_success();
                let _ = res.bytes().await;
                ok
            }
            Err(_) => false,
        };
        let took = started.elapsed();
        counter.fetch_add(1, Ordering::Relaxed);
        if ok {
            out.by_op.push((op, took));
        } else {
            out.errors += 1;
        }
    }
    out
}

fn percentile(sorted: &[Duration], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::cast_precision_loss
    )]
    let i = (((sorted.len() - 1) as f64) * p).round() as usize;
    sorted[i].as_secs_f64() * 1000.0
}

/// Starts the size's API processes and waits until each is ready.
async fn start_api(
    url: &str,
    size: Size,
) -> (Vec<std::process::Child>, Vec<String>, reqwest::Client) {
    let mut processes = Vec::new();
    let mut bases = Vec::new();
    for n in 0..size.api_processes {
        let port = 18_200 + n;
        processes.push(
            huxley_command(&["serve", "--roles", "api"])
                .env("HUXLEY_DATABASE__URL", url)
                .env("HUXLEY_CRYPTO__KEK", TEST_KEK)
                .env("HUXLEY_HTTP__BIND", format!("127.0.0.1:{port}"))
                .env("HUXLEY_HTTP__COOKIE_SECURE", "false")
                .env("HUXLEY_HTTP__LOGIN_ATTEMPTS_PER_MINUTE", "100000")
                .env(
                    "HUXLEY_STORAGE__URL",
                    std::env::temp_dir().join(format!("huxley-bench-{port}")),
                )
                .env("RUST_LOG", "warn")
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .expect("the huxley binary starts"),
        );
        bases.push(format!("http://127.0.0.1:{port}"));
    }
    let http = reqwest::Client::builder()
        .pool_max_idle_per_host(size.users)
        .build()
        .unwrap();
    for base in &bases {
        for _ in 0..100 {
            if http
                .get(format!("{base}/readyz"))
                .send()
                .await
                .is_ok_and(|r| r.status().is_success())
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }
    (processes, bases, http)
}

/// Every signed-in user works for `length`; their samples, together.
async fn phase(
    http: &reqwest::Client,
    cookies: &[(String, String)],
    tickets: &Arc<Vec<Uuid>>,
    clients: &Arc<Vec<Uuid>>,
    length: Duration,
    seed: usize,
) -> Samples {
    let until = Instant::now() + length;
    let counter = Arc::new(AtomicUsize::new(0));
    let mut tasks = Vec::new();
    for (u, (base, cookie)) in cookies.iter().enumerate() {
        tasks.push(tokio::spawn(user_loop(
            http.clone(),
            base.clone(),
            cookie.clone(),
            tickets.clone(),
            clients.clone(),
            until,
            u * seed + 1,
            counter.clone(),
        )));
    }
    let mut all = Samples::default();
    for t in tasks {
        let s = t.await.unwrap();
        all.by_op.extend(s.by_op);
        all.errors += s.errors;
    }
    all
}

/// Prints the results as the table docs/performance.md uses.
fn report(size: Size, all: &Samples, elapsed: f64) {
    let mut totals: Vec<Duration> = all.by_op.iter().map(|(_, d)| *d).collect();
    totals.sort();
    #[allow(clippy::cast_precision_loss)]
    let rate = totals.len() as f64 / elapsed;
    println!(
        "\n{} ({} clients, {} technicians, {} tickets; {} API process(es), {} concurrent users, {}s)",
        size.name,
        size.clients,
        size.technicians,
        size.tickets,
        size.api_processes,
        size.users,
        size.seconds
    );
    println!(
        "| all requests | {rate:.0} req/s | p50 {:.1} ms | p95 {:.1} ms | p99 {:.1} ms | {} errors |",
        percentile(&totals, 0.5),
        percentile(&totals, 0.95),
        percentile(&totals, 0.99),
        all.errors
    );
    for (op, _) in MIX {
        let mut d: Vec<Duration> = all
            .by_op
            .iter()
            .filter(|(o, _)| *o == op)
            .map(|(_, d)| *d)
            .collect();
        d.sort();
        println!(
            "| {} | {} | p50 {:.1} ms | p95 {:.1} ms | p99 {:.1} ms |",
            op.name(),
            d.len(),
            percentile(&d, 0.5),
            percentile(&d, 0.95),
            percentile(&d, 0.99)
        );
    }
}

#[tokio::test]
#[ignore = "a measurement; run on demand (scripts/bench.sh)"]
async fn api_under_a_technicians_workload() {
    let size = size();
    let Some(db) = Database::create("apibench").await else {
        return;
    };
    let config = DatabaseConfig {
        url: SecretString::from(db.url.clone()),
        max_connections: 8,
        acquire_timeout_secs: 30,
        statement_timeout_ms: 600_000,
        read_url: None,
        direct_url: None,
        pooled: false,
    };
    let owner = huxley_db::connect_owner(&config).unwrap();
    huxley_db::migrate(&owner).await.unwrap();
    let pools = TestPools {
        owner,
        app: huxley_db::connect(&config).unwrap(),
    };
    let seeding = Instant::now();
    let (_, clients, emails, tickets) = seed(&pools, size).await;
    println!(
        "seeded {} tickets in {:.0}s",
        size.tickets,
        seeding.elapsed().as_secs_f64()
    );

    let (processes, bases, http) = start_api(&db.url, size).await;
    let tickets = Arc::new(tickets);
    let clients = Arc::new(clients);
    let mut cookies = Vec::new();
    for u in 0..size.users {
        let base = &bases[u % bases.len()];
        cookies.push((
            base.clone(),
            sign_in(&http, base, &emails[u % emails.len()]).await,
        ));
    }
    // Warm up, then measure.
    let run = |seconds: u64, seed: usize| {
        phase(
            &http,
            &cookies,
            &tickets,
            &clients,
            Duration::from_secs(seconds),
            seed,
        )
    };
    run(10, 0).await;
    let started = Instant::now();
    let all = run(size.seconds, 7919).await;
    let elapsed = started.elapsed().as_secs_f64();
    for mut p in processes {
        let _ = p.kill();
        let _ = p.wait();
    }
    report(size, &all, elapsed);
    assert_eq!(all.errors, 0, "requests failed");
    pools.owner.close().await;
    pools.app.close().await;
    // BENCH_KEEP keeps the database, to look at query plans afterwards.
    if std::env::var_os("BENCH_KEEP").is_none() {
        db.drop().await;
    } else {
        println!("kept the database: {}", db.url);
    }
}
