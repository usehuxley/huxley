//! The script egress proxy (design doc §6.9). A script container has no
//! network of its own: its only way out is this proxy, which the runner
//! serves on a Unix socket mounted into the container. It speaks HTTP
//! proxying (`CONNECT host:port` for HTTPS, absolute-form requests for
//! plain HTTP) and applies the same SSRF policy as HTTP steps, resolving
//! names itself and connecting only to checked addresses.

use std::sync::Arc;
use std::time::Duration;

use huxley_engine::EgressConfig;
use tokio::io::{AsyncRead, AsyncReadExt as _, AsyncWrite, AsyncWriteExt as _};
use tokio::net::{TcpStream, UnixListener};
use tokio_util::sync::CancellationToken;

/// Largest request head read.
const MAX_HEAD: usize = 16 * 1024;
const HEAD_TIMEOUT: Duration = Duration::from_secs(15);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// Serves the proxy on `listener` until `shutdown`.
pub async fn serve(listener: UnixListener, policy: Arc<EgressConfig>, shutdown: CancellationToken) {
    loop {
        let accepted = tokio::select! {
            () = shutdown.cancelled() => return,
            accepted = listener.accept() => accepted,
        };
        let Ok((stream, _)) = accepted else { continue };
        let policy = policy.clone();
        let stop = shutdown.clone();
        tokio::spawn(async move {
            tokio::select! {
                () = stop.cancelled() => {}
                () = handle(stream, &policy) => {}
            }
        });
    }
}

/// A parsed request head.
#[derive(Debug, PartialEq, Eq)]
enum Target {
    /// `CONNECT host:port`: a tunnel.
    Tunnel { host: String, port: u16 },
    /// `GET http://host:port/path`: forwarded with the head rewritten.
    Forward {
        host: String,
        port: u16,
        head: Vec<u8>,
    },
}

async fn handle<S: AsyncRead + AsyncWrite + Unpin>(mut client: S, policy: &EgressConfig) {
    let Ok(Ok((head, rest))) = tokio::time::timeout(HEAD_TIMEOUT, read_head(&mut client)).await
    else {
        return;
    };
    let target = match parse(&head) {
        Ok(target) => target,
        Err(message) => {
            let _ = refuse(&mut client, "400 Bad Request", message).await;
            return;
        }
    };
    let (host, port) = match &target {
        Target::Tunnel { host, port } | Target::Forward { host, port, .. } => (host.clone(), *port),
    };
    let addrs = match huxley_engine::allowed_addrs(policy, &host, port).await {
        Ok(addrs) => addrs,
        Err(blocked) => {
            let _ = refuse(&mut client, "403 Forbidden", &blocked.0).await;
            return;
        }
    };
    let Some(mut upstream) = connect(&addrs).await else {
        let _ = refuse(&mut client, "502 Bad Gateway", "couldn't connect").await;
        return;
    };
    let ready = match target {
        Target::Tunnel { .. } => client
            .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
            .await
            .is_ok(),
        Target::Forward { head, .. } => upstream.write_all(&head).await.is_ok(),
    };
    if !ready || upstream.write_all(&rest).await.is_err() {
        return;
    }
    let _ = tokio::io::copy_bidirectional(&mut client, &mut upstream).await;
}

async fn connect(addrs: &[std::net::SocketAddr]) -> Option<TcpStream> {
    for addr in addrs {
        if let Ok(Ok(stream)) =
            tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect(addr)).await
        {
            return Some(stream);
        }
    }
    None
}

async fn refuse<S: AsyncWrite + Unpin>(
    client: &mut S,
    status: &str,
    message: &str,
) -> std::io::Result<()> {
    let body = format!("Huxley egress proxy: {message}\n");
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    client.write_all(response.as_bytes()).await?;
    client.shutdown().await
}

/// Reads up to the end of the request head; returns it and anything read
/// past it.
async fn read_head<S: AsyncRead + Unpin>(client: &mut S) -> std::io::Result<(Vec<u8>, Vec<u8>)> {
    let mut buf = Vec::with_capacity(1024);
    let mut chunk = [0u8; 2048];
    loop {
        let n = client.read(&mut chunk).await?;
        if n == 0 {
            return Err(std::io::ErrorKind::UnexpectedEof.into());
        }
        buf.extend_from_slice(&chunk[..n]);
        if let Some(end) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            let rest = buf.split_off(end + 4);
            return Ok((buf, rest));
        }
        if buf.len() > MAX_HEAD {
            return Err(std::io::ErrorKind::InvalidData.into());
        }
    }
}

fn split_host_port(authority: &str, default_port: u16) -> Option<(String, u16)> {
    if let Some(rest) = authority.strip_prefix('[') {
        let (host, after) = rest.split_once(']')?;
        let port = match after.strip_prefix(':') {
            Some(p) => p.parse().ok()?,
            None if after.is_empty() => default_port,
            None => return None,
        };
        return Some((host.to_owned(), port));
    }
    match authority.rsplit_once(':') {
        Some((host, port)) => Some((host.to_owned(), port.parse().ok()?)),
        None => Some((authority.to_owned(), default_port)),
    }
}

fn parse(head: &[u8]) -> Result<Target, &'static str> {
    let text = std::str::from_utf8(head).map_err(|_| "the request isn't text")?;
    let mut lines = text.split("\r\n");
    let first = lines.next().unwrap_or_default();
    let mut parts = first.split(' ');
    let (Some(method), Some(target), Some(version)) = (parts.next(), parts.next(), parts.next())
    else {
        return Err("malformed request line");
    };
    if method == "CONNECT" {
        let (host, port) = split_host_port(target, 443).ok_or("CONNECT needs host:port")?;
        return Ok(Target::Tunnel { host, port });
    }
    let Some(after) = target.strip_prefix("http://") else {
        return Err("only CONNECT (for HTTPS) and http:// URLs");
    };
    let (authority, path) = match after.find('/') {
        Some(i) => (&after[..i], &after[i..]),
        None => (after, "/"),
    };
    let authority = authority.rsplit_once('@').map_or(authority, |(_, a)| a);
    let (host, port) = split_host_port(authority, 80).ok_or("bad host in the URL")?;
    // Origin-form request, one per connection, without proxy headers.
    let mut out = format!("{method} {path} {version}\r\n");
    for line in lines.filter(|l| !l.is_empty()) {
        let name = line
            .split(':')
            .next()
            .unwrap_or_default()
            .to_ascii_lowercase();
        if name.starts_with("proxy-") || name == "connection" || name == "keep-alive" {
            continue;
        }
        out.push_str(line);
        out.push_str("\r\n");
    }
    out.push_str("Connection: close\r\n\r\n");
    Ok(Target::Forward {
        host,
        port,
        head: out.into_bytes(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_are_parsed() {
        assert_eq!(
            parse(b"CONNECT graph.microsoft.com:443 HTTP/1.1\r\nHost: x\r\n\r\n"),
            Ok(Target::Tunnel {
                host: "graph.microsoft.com".into(),
                port: 443
            })
        );
        assert_eq!(
            parse(b"CONNECT [::1]:8443 HTTP/1.1\r\n\r\n"),
            Ok(Target::Tunnel {
                host: "::1".into(),
                port: 8443
            })
        );
        let Ok(Target::Forward { host, port, head }) = parse(
            b"GET http://user@example.com:8080/a?b=1 HTTP/1.1\r\nHost: example.com\r\nProxy-Authorization: x\r\nConnection: keep-alive\r\n\r\n",
        ) else {
            panic!()
        };
        assert_eq!((host.as_str(), port), ("example.com", 8080));
        assert_eq!(
            String::from_utf8(head).unwrap(),
            "GET /a?b=1 HTTP/1.1\r\nHost: example.com\r\nConnection: close\r\n\r\n"
        );
        assert!(parse(b"GET /relative HTTP/1.1\r\n\r\n").is_err());
        assert!(parse(b"GET ftp://x/ HTTP/1.1\r\n\r\n").is_err());
    }
}
