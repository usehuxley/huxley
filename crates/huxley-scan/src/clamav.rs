//! ClamAV: files go to `clamd` with its `INSTREAM` command.

use std::time::Duration;

use clamav_client::tokio as clamd;

use crate::{ClamavConfig, ScanError, Verdict, secs};

#[derive(Debug, Clone)]
enum Address {
    Tcp(String),
    #[cfg(unix)]
    Unix(String),
}

#[derive(Debug, Clone)]
pub struct Clamav {
    address: Address,
    timeout: Duration,
}

impl Clamav {
    pub(crate) fn new(config: &ClamavConfig) -> Result<Self, ScanError> {
        let raw = config.address.trim();
        let address = if let Some(path) = raw.strip_prefix("unix:") {
            #[cfg(unix)]
            {
                Address::Unix(path.trim_start_matches("//").to_owned())
            }
            #[cfg(not(unix))]
            {
                let _ = path;
                return Err(ScanError("unix sockets need a Unix host".into()));
            }
        } else {
            let host = raw.strip_prefix("tcp://").unwrap_or(raw);
            if host.is_empty() || !host.contains(':') {
                return Err(ScanError(format!(
                    "uploads.scan.clamav.address: expected tcp://host:port or unix:/path, got {raw:?}"
                )));
            }
            Address::Tcp(host.to_owned())
        };
        Ok(Self {
            address,
            timeout: secs(config.timeout_seconds),
        })
    }

    async fn timed<F: Future<Output = std::io::Result<Vec<u8>>>>(
        &self,
        what: &str,
        call: F,
    ) -> Result<Vec<u8>, ScanError> {
        match tokio::time::timeout(self.timeout, call).await {
            Ok(Ok(reply)) => Ok(reply),
            Ok(Err(e)) => Err(ScanError(format!("ClamAV {what} failed: {e}"))),
            Err(_) => Err(ScanError(format!("ClamAV didn't answer {what} in time"))),
        }
    }

    pub(crate) async fn scan(&self, data: &[u8]) -> Result<Verdict, ScanError> {
        let reply = match &self.address {
            Address::Tcp(host) => {
                let tcp = clamd::Tcp {
                    host_address: host.as_str(),
                };
                self.timed("a scan", clamd::scan_buffer(data, tcp, None))
                    .await?
            }
            #[cfg(unix)]
            Address::Unix(path) => {
                let socket = clamd::Socket {
                    socket_path: path.as_str(),
                };
                self.timed("a scan", clamd::scan_buffer(data, socket, None))
                    .await?
            }
        };
        parse_reply(&reply)
    }

    pub(crate) async fn version(&self) -> Result<String, ScanError> {
        let reply = match &self.address {
            Address::Tcp(host) => {
                let tcp = clamd::Tcp {
                    host_address: host.as_str(),
                };
                self.timed("a version check", clamd::get_version(tcp))
                    .await?
            }
            #[cfg(unix)]
            Address::Unix(path) => {
                let socket = clamd::Socket {
                    socket_path: path.as_str(),
                };
                self.timed("a version check", clamd::get_version(socket))
                    .await?
            }
        };
        Ok(text(&reply).to_owned())
    }
}

fn text(reply: &[u8]) -> &str {
    std::str::from_utf8(reply)
        .unwrap_or("")
        .trim_end_matches(['\0', '\n', '\r', ' '])
}

/// Reads clamd's answer to `INSTREAM`: `stream: OK`, `stream: <threat>
/// FOUND`, or `<message> ERROR` (e.g. the file is over `StreamMaxLength`).
/// Anything else is an error, never "clean".
pub fn parse_reply(reply: &[u8]) -> Result<Verdict, ScanError> {
    let line = text(reply);
    let body = line.strip_prefix("stream:").map_or(line, str::trim);
    if body == "OK" {
        Ok(Verdict::Clean)
    } else if let Some(threat) = body.strip_suffix(" FOUND") {
        Ok(Verdict::Infected(threat.trim().to_owned()))
    } else if let Some(message) = body.strip_suffix(" ERROR") {
        Err(ScanError(format!(
            "ClamAV couldn't scan it: {}",
            message.trim()
        )))
    } else {
        Err(ScanError(format!(
            "ClamAV gave an answer Huxley doesn't know: {line:?}"
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replies() {
        assert_eq!(parse_reply(b"stream: OK\0"), Ok(Verdict::Clean));
        assert_eq!(
            parse_reply(b"stream: Win.Test.EICAR_HDB-1 FOUND\0"),
            Ok(Verdict::Infected("Win.Test.EICAR_HDB-1".into()))
        );
        assert!(
            parse_reply(b"INSTREAM size limit exceeded. ERROR\0")
                .unwrap_err()
                .0
                .contains("size limit")
        );
        // Never "clean" by accident.
        assert!(parse_reply(b"").is_err());
        assert!(parse_reply(b"stream: OKAY").is_err());
        assert!(parse_reply(b"stream: OK FOUND").is_ok_and(|v| v != Verdict::Clean));
    }

    #[test]
    fn addresses() {
        let at = |a: &str| {
            Clamav::new(&ClamavConfig {
                address: a.into(),
                timeout_seconds: 5,
            })
        };
        assert!(
            matches!(at("tcp://clamav:3310").unwrap().address, Address::Tcp(h) if h == "clamav:3310")
        );
        assert!(matches!(
            at("clamav:3310").unwrap().address,
            Address::Tcp(_)
        ));
        assert!(
            matches!(at("unix:/run/clamd.sock").unwrap().address, Address::Unix(p) if p == "/run/clamd.sock")
        );
        assert!(at("clamav").is_err());
    }
}
