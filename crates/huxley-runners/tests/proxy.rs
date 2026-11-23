//! The script egress proxy over its Unix socket: tunnels and forwarded
//! requests reach allowed hosts only.

use std::sync::Arc;

use huxley_engine::EgressConfig;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::{TcpListener, UnixStream};
use tokio_util::sync::CancellationToken;

/// A server that answers every connection with `hello`, echoing what the
/// request's first line was.
async fn upstream() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        loop {
            let (mut s, _) = listener.accept().await.unwrap();
            tokio::spawn(async move {
                let mut buf = vec![0u8; 4096];
                let n = s.read(&mut buf).await.unwrap_or(0);
                let first = String::from_utf8_lossy(&buf[..n])
                    .lines()
                    .next()
                    .unwrap_or_default()
                    .to_owned();
                let body = format!("hello {first}");
                let reply = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{body}",
                    body.len()
                );
                let _ = s.write_all(reply.as_bytes()).await;
            });
        }
    });
    port
}

fn proxy(policy: EgressConfig) -> (std::path::PathBuf, CancellationToken) {
    let dir = std::env::temp_dir().join(format!("huxley-proxy-test-{}", uuid::Uuid::now_v7()));
    std::fs::create_dir(&dir).unwrap();
    let path = dir.join("proxy.sock");
    let listener = tokio::net::UnixListener::bind(&path).unwrap();
    let stop = CancellationToken::new();
    tokio::spawn(huxley_runners::proxy::serve(
        listener,
        Arc::new(policy),
        stop.clone(),
    ));
    (path, stop)
}

async fn exchange(path: &std::path::Path, request: &str, then: Option<&str>) -> String {
    let mut s = UnixStream::connect(path).await.unwrap();
    s.write_all(request.as_bytes()).await.unwrap();
    if let Some(then) = then {
        // Read the tunnel's 200 first.
        let mut buf = vec![0u8; 1024];
        let n = s.read(&mut buf).await.unwrap();
        assert!(String::from_utf8_lossy(&buf[..n]).starts_with("HTTP/1.1 200"));
        s.write_all(then.as_bytes()).await.unwrap();
    }
    let mut out = String::new();
    let _ = s.read_to_string(&mut out).await;
    out
}

fn local() -> EgressConfig {
    EgressConfig {
        private_hosts: vec!["127.0.0.1".into()],
        ..EgressConfig::default()
    }
}

#[tokio::test]
async fn forwards_plain_http_to_allowed_hosts() {
    let port = upstream().await;
    let (path, _stop) = proxy(local());
    let out = exchange(
        &path,
        &format!("GET http://127.0.0.1:{port}/x?y=1 HTTP/1.1\r\nHost: 127.0.0.1\r\nProxy-Authorization: z\r\n\r\n"),
        None,
    )
    .await;
    assert!(out.contains("hello GET /x?y=1 HTTP/1.1"), "{out}");
}

#[tokio::test]
async fn tunnels_to_allowed_hosts() {
    let port = upstream().await;
    let (path, _stop) = proxy(local());
    let out = exchange(
        &path,
        &format!("CONNECT 127.0.0.1:{port} HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n"),
        Some("PING / HTTP/1.1\r\n\r\n"),
    )
    .await;
    assert!(out.contains("hello PING"), "{out}");
}

#[tokio::test]
async fn private_and_metadata_addresses_are_refused() {
    let port = upstream().await;
    let (path, _stop) = proxy(EgressConfig::default());
    for request in [
        format!("CONNECT 127.0.0.1:{port} HTTP/1.1\r\n\r\n"),
        format!("GET http://localhost:{port}/ HTTP/1.1\r\n\r\n"),
        "GET http://169.254.169.254/latest/meta-data/ HTTP/1.1\r\n\r\n".to_owned(),
        "CONNECT [::1]:443 HTTP/1.1\r\n\r\n".to_owned(),
    ] {
        let out = exchange(&path, &request, None).await;
        assert!(out.starts_with("HTTP/1.1 403"), "{request}: {out}");
        assert!(!out.contains("hello"));
    }
    let out = exchange(&path, "GET ftp://example.com/ HTTP/1.1\r\n\r\n", None).await;
    assert!(out.starts_with("HTTP/1.1 400"), "{out}");
}
