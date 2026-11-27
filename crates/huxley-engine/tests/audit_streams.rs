//! SIEM audit streaming (ADR-087): sealed audit records are shipped in
//! chain order, with the stream's secret; a failing destination backs off
//! and, once it's back, gets what it missed without repeats; Splunk gets
//! HEC events.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, Uri};
use axum::routing::post;
use huxley_core::{RequestContext, Via};
use huxley_db::audit::{self, AuditRecord, ChainStatus};
use huxley_db::testing::{ensure_root, isolated};
use huxley_db::{OrgScope, begin_scoped};
use huxley_engine::audit_stream::{StreamInput, StreamKind};
use huxley_engine::{EgressConfig, Engine, EngineConfig};
use serde_json::Value;

/// What the mock destination received: path, headers, body lines.
type Received = Arc<Mutex<Vec<(String, HeaderMap, Vec<Value>)>>>;

#[derive(Clone)]
struct Mock {
    received: Received,
    failing: Arc<AtomicBool>,
}

async fn receive(
    State(mock): State<Mock>,
    uri: Uri,
    headers: HeaderMap,
    body: String,
) -> StatusCode {
    if mock.failing.load(Ordering::SeqCst) {
        return StatusCode::SERVICE_UNAVAILABLE;
    }
    let lines = body
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    mock.received
        .lock()
        .unwrap()
        .push((uri.path().to_owned(), headers, lines));
    StatusCode::OK
}

async fn mock() -> (Mock, String) {
    let mock = Mock {
        received: Arc::default(),
        failing: Arc::default(),
    };
    let app = Router::new()
        .route("/ingest", post(receive))
        .route("/services/collector/event", post(receive))
        .with_state(mock.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://localhost:{}", listener.local_addr().unwrap().port());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (mock, base)
}

async fn record(pools: &huxley_db::testing::TestPools, action: &str) {
    let ctx = RequestContext::system(Via::System);
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    audit::record(&mut tx, &ctx, AuditRecord::new(action))
        .await
        .unwrap();
    tx.commit().await.unwrap();
}

fn seqs(received: &Received, at: usize) -> Vec<i64> {
    received.lock().unwrap()[at]
        .2
        .iter()
        .map(|r| {
            r["seq"]
                .as_i64()
                .or_else(|| r["event"]["seq"].as_i64())
                .unwrap()
        })
        .collect()
}

fn input(kind: StreamKind, url: String, secret: &str, from_start: bool) -> StreamInput {
    StreamInput {
        name: "SIEM".into(),
        kind,
        url,
        header_name: Some("X-Api-Key".into()),
        secret: Some(secret.into()),
        enabled: true,
        from_start,
    }
}

fn engine(pools: &huxley_db::testing::TestPools) -> Engine {
    let crypto = huxley_crypto::Crypto::new(pools.app.clone(), huxley_crypto::testing::key_ring());
    let config = EngineConfig {
        egress: EgressConfig {
            private_hosts: vec!["localhost".into()],
            ..EgressConfig::default()
        },
        ..EngineConfig::default()
    };
    Engine::with_config(pools.app.clone(), crypto, &config)
}

#[tokio::test]
async fn audit_records_are_shipped_in_order_and_resume_after_failures() {
    let Some(db) = isolated("audit_streams").await else {
        return;
    };
    let pools = db.pools.clone();
    ensure_root(&pools).await;
    let engine = engine(&pools);
    let ctx = RequestContext::system(Via::System);
    let (mock, base) = mock().await;

    // A new stream starts from now: only what's recorded after it is sent.
    let stream = engine
        .create_audit_stream(
            &ctx,
            &input(StreamKind::Webhook, format!("{base}/ingest"), "k1", false),
        )
        .await
        .unwrap();
    assert!(stream.has_secret);
    record(&pools, "test.one").await;
    record(&pools, "test.two").await;
    assert!(engine.ship_audit_streams().await.unwrap() >= 2);
    let first = seqs(&mock.received, 0);
    assert!(first.windows(2).all(|w| w[1] == w[0] + 1), "{first:?}");
    {
        let received = mock.received.lock().unwrap();
        assert_eq!(received[0].1["x-api-key"], "k1");
        let actions: Vec<&str> = received[0]
            .2
            .iter()
            .map(|r| r["action"].as_str().unwrap())
            .collect();
        assert!(actions.ends_with(&["test.one", "test.two"]), "{actions:?}");
        assert!(
            !actions.contains(&"org.created"),
            "started from now: {actions:?}"
        );
        assert_eq!(received[0].2[0]["hash"].as_str().unwrap().len(), 64);
    }

    // A failing destination backs off and keeps its place.
    mock.failing.store(true, Ordering::SeqCst);
    record(&pools, "test.three").await;
    assert_eq!(engine.ship_audit_streams().await.unwrap(), 0);
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    let failed = huxley_engine::audit_stream::get(&mut tx, stream.id)
        .await
        .unwrap()
        .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(failed.failures, 1);
    assert!(
        failed.last_error.as_deref().unwrap().contains("503"),
        "{:?}",
        failed.last_error
    );
    assert!(failed.behind >= 1);
    // Backing off: not tried again yet.
    mock.failing.store(false, Ordering::SeqCst);
    assert_eq!(engine.ship_audit_streams().await.unwrap(), 0);
    // Saving the stream tries it again: it gets what it missed, once.
    engine
        .update_audit_stream(
            &ctx,
            stream.id,
            &StreamInput {
                secret: None,
                ..input(StreamKind::Webhook, format!("{base}/ingest"), "", false)
            },
        )
        .await
        .unwrap()
        .unwrap();
    assert!(engine.ship_audit_streams().await.unwrap() >= 1);
    let resumed = seqs(&mock.received, 1);
    assert_eq!(resumed[0], first.last().unwrap() + 1, "no gap, no repeat");
    {
        let received = mock.received.lock().unwrap();
        assert_eq!(received[1].1["x-api-key"], "k1", "the secret stays");
        assert!(received[1].2.iter().any(|r| r["action"] == "test.three"));
    }
}

#[tokio::test]
async fn splunk_gets_hec_events_and_a_test_event_stays_out_of_the_chain() {
    let Some(db) = isolated("audit_streams_splunk").await else {
        return;
    };
    let pools = db.pools.clone();
    ensure_root(&pools).await;
    let engine = engine(&pools);
    let ctx = RequestContext::system(Via::System);
    let (mock, base) = mock().await;
    record(&pools, "test.one").await;
    record(&pools, "test.two").await;
    // Splunk, from the start: HEC events with the Splunk token.
    let splunk = engine
        .create_audit_stream(
            &ctx,
            &input(StreamKind::SplunkHec, base.clone(), "hec-token", true),
        )
        .await
        .unwrap();
    assert!(engine.ship_audit_streams().await.unwrap() >= 3);
    {
        let received = mock.received.lock().unwrap();
        let (path, headers, events) = received.last().unwrap();
        assert_eq!(path, "/services/collector/event");
        assert_eq!(headers["authorization"], "Splunk hec-token");
        assert_eq!(events[0]["event"]["seq"], 1);
        assert_eq!(events[0]["sourcetype"], "huxley:audit");
        assert!(events[0]["time"].is_i64());
    }
    // A test event reaches the destination without joining the chain.
    let shipped_before = mock.received.lock().unwrap().len();
    assert_eq!(
        engine.test_audit_stream(splunk.id).await.unwrap(),
        Some(Ok(()))
    );
    assert_eq!(mock.received.lock().unwrap().len(), shipped_before + 1);
    assert!(matches!(
        audit::verify_chain(&pools.owner).await.unwrap(),
        ChainStatus::Intact { .. }
    ));
}
