//! Both scanners against stand-ins speaking their protocols: a fake clamd
//! (INSTREAM) and a mock MetaDefender (the documented REST flow). Neither
//! is the real thing; `docs/attachment-scanning.md` says what's been
//! checked against real scanners.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use huxley_scan::{ClamavConfig, MetaDefenderConfig, Provider, ScanConfig, Scanner, Verdict};
use secrecy::SecretString;
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::TcpListener;

/// The EICAR test file, put together at runtime so this source file isn't
/// itself flagged by antivirus software.
static EICAR_STRING: std::sync::LazyLock<Vec<u8>> = std::sync::LazyLock::new(|| {
    [
        r"X5O!P%@AP[4\PZX54(P^)7CC)7}$",
        "EICAR-STANDARD-ANTIVIRUS-TEST-FILE!$H+H*",
    ]
    .concat()
    .into_bytes()
});

/// Answers INSTREAM like clamd: FOUND for the EICAR test string, else OK.
async fn fake_clamd() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap().to_string();
    tokio::spawn(async move {
        loop {
            let (mut socket, _) = listener.accept().await.unwrap();
            tokio::spawn(async move {
                let mut command = Vec::new();
                loop {
                    let b = socket.read_u8().await.unwrap();
                    if b == 0 {
                        break;
                    }
                    command.push(b);
                }
                let reply: &[u8] = match command.as_slice() {
                    b"zVERSION" => b"ClamAV 1.4.1/27000\0",
                    b"zINSTREAM" => {
                        let mut data = Vec::new();
                        loop {
                            let len = socket.read_u32().await.unwrap() as usize;
                            if len == 0 {
                                break;
                            }
                            let mut chunk = vec![0; len];
                            socket.read_exact(&mut chunk).await.unwrap();
                            data.extend(chunk);
                        }
                        if data
                            .windows(EICAR_STRING.len())
                            .any(|w| w == EICAR_STRING.as_slice())
                        {
                            b"stream: Win.Test.EICAR_HDB-1 FOUND\0"
                        } else {
                            b"stream: OK\0"
                        }
                    }
                    _ => b"UNKNOWN COMMAND\0",
                };
                socket.write_all(reply).await.unwrap();
            });
        }
    });
    address
}

#[tokio::test]
async fn clamav_scans_over_tcp() {
    let address = fake_clamd().await;
    let scanner = Scanner::from_config(&ScanConfig {
        provider: Provider::Clamav,
        clamav: ClamavConfig {
            address: format!("tcp://{address}"),
            timeout_seconds: 5,
        },
        ..ScanConfig::default()
    })
    .unwrap()
    .unwrap();
    assert_eq!(scanner.name(), "clamav");
    assert_eq!(scanner.check().await.unwrap(), "ClamAV 1.4.1/27000");
    assert_eq!(
        scanner.scan(b"quarterly report", "r.txt").await,
        Ok(Verdict::Clean)
    );
    let mut big = vec![b'a'; 3 * 1024 * 1024];
    big.extend_from_slice(&EICAR_STRING);
    assert_eq!(
        scanner.scan(&big, "big.bin").await,
        Ok(Verdict::Infected("Win.Test.EICAR_HDB-1".into()))
    );
}

#[tokio::test]
async fn an_unreachable_clamav_is_an_error_not_a_verdict() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    drop(listener);
    let scanner = Scanner::from_config(&ScanConfig {
        provider: Provider::Clamav,
        clamav: ClamavConfig {
            address: address.to_string(),
            timeout_seconds: 2,
        },
        ..ScanConfig::default()
    })
    .unwrap()
    .unwrap();
    assert!(scanner.scan(b"x", "x").await.is_err());
}

#[derive(Default)]
struct Mock {
    submitted: AtomicUsize,
    polls: AtomicUsize,
}

/// The documented flow: `POST /file` → `data_id`; `GET /file/{id}` until 100%.
async fn mock_metadefender(mock: Arc<Mock>) -> String {
    async fn submit(
        State(mock): State<Arc<Mock>>,
        headers: HeaderMap,
        body: axum::body::Bytes,
    ) -> Result<Json<Value>, StatusCode> {
        if headers.get("apikey").and_then(|v| v.to_str().ok()) != Some("test-key") {
            return Err(StatusCode::UNAUTHORIZED);
        }
        mock.submitted.fetch_add(1, Ordering::SeqCst);
        let infected = body
            .windows(EICAR_STRING.len())
            .any(|w| w == EICAR_STRING.as_slice());
        let name = headers
            .get("filename")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        let id = if name == "limited.txt" {
            "limited"
        } else if infected {
            "bad"
        } else {
            "good"
        };
        Ok(Json(json!({ "data_id": id })))
    }
    async fn result(State(mock): State<Arc<Mock>>, Path(id): Path<String>) -> Json<Value> {
        // Scanning on the first ask, done on the second.
        let n = mock.polls.fetch_add(1, Ordering::SeqCst);
        if n % 2 == 0 {
            return Json(json!({
                "data_id": id,
                "process_info": { "progress_percentage": 30 },
                "scan_results": { "scan_all_result_i": 255 }
            }));
        }
        Json(match id.as_str() {
            "bad" => json!({
                "data_id": id,
                "process_info": { "progress_percentage": 100, "result": "Blocked" },
                "scan_results": { "scan_all_result_i": 1, "scan_all_result_a": "Infected" }
            }),
            "limited" => json!({
                "data_id": id,
                "process_info": { "progress_percentage": 100 },
                "scan_results": { "scan_all_result_i": 253 }
            }),
            _ => json!({
                "data_id": id,
                "process_info": { "progress_percentage": 100, "result": "Allowed" },
                "scan_results": { "scan_all_result_i": 0, "scan_all_result_a": "No Threat Detected" }
            }),
        })
    }
    async fn version() -> Json<Value> {
        Json(json!({ "version": "5.24.2", "product_id": "MDCore" }))
    }
    let app = axum::Router::new()
        .route("/file", axum::routing::post(submit))
        .route("/file/{id}", axum::routing::get(result))
        .route("/version", axum::routing::get(version))
        .with_state(mock);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    url
}

fn metadefender(url: &str, key: &str) -> Scanner {
    Scanner::from_config(&ScanConfig {
        provider: Provider::Metadefender,
        metadefender: MetaDefenderConfig {
            url: url.into(),
            api_key: Some(SecretString::from(key.to_owned())),
            rule: None,
            timeout_seconds: 10,
            poll_milliseconds: 100,
        },
        ..ScanConfig::default()
    })
    .unwrap()
    .unwrap()
}

#[tokio::test]
async fn metadefender_submits_and_polls_for_the_verdict() {
    let mock = Arc::new(Mock::default());
    let url = mock_metadefender(mock.clone()).await;
    let scanner = metadefender(&url, "test-key");
    assert_eq!(scanner.check().await.unwrap(), "MetaDefender 5.24.2");
    assert_eq!(scanner.scan(b"fine", "fine.txt").await, Ok(Verdict::Clean));
    assert!(matches!(
        scanner.scan(&EICAR_STRING, "eicar.com").await,
        Ok(Verdict::Infected(reason)) if reason.contains("result 1")
    ));
    // A rate-limited scan is tried again later, not passed.
    assert!(scanner.scan(b"fine", "limited.txt").await.is_err());
    assert_eq!(mock.submitted.load(Ordering::SeqCst), 3);
    assert!(mock.polls.load(Ordering::SeqCst) >= 6, "it waited for 100%");

    // A refused key is an error, and says so.
    let refused = metadefender(&url, "wrong")
        .scan(b"x", "x")
        .await
        .unwrap_err();
    assert!(refused.0.contains("API key"), "{refused}");
}

/// Against a real clamd when `HUXLEY_TEST_CLAMAV` names one (e.g.
/// `tcp://127.0.0.1:3310`); skipped otherwise.
#[tokio::test]
async fn real_clamav_finds_the_eicar_test_file() {
    let Ok(address) = std::env::var("HUXLEY_TEST_CLAMAV") else {
        return;
    };
    let scanner = Scanner::from_config(&ScanConfig {
        provider: Provider::Clamav,
        clamav: ClamavConfig {
            address,
            timeout_seconds: 60,
        },
        ..ScanConfig::default()
    })
    .unwrap()
    .unwrap();
    assert!(scanner.check().await.unwrap().starts_with("ClamAV "));
    assert_eq!(
        scanner.scan(b"quarterly report", "r.txt").await,
        Ok(Verdict::Clean)
    );
    assert!(matches!(
        scanner.scan(&EICAR_STRING, "eicar.com").await,
        Ok(Verdict::Infected(name)) if name.contains("Eicar") || name.contains("EICAR")
    ));
}
