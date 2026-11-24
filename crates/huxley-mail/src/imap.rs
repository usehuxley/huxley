//! Fetching mail over IMAP (design doc §8.4): new messages since a cursor,
//! read without marking them, then marked `\Seen`.

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use async_imap::Client;
use futures_util::TryStreamExt as _;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;
use tokio_rustls::rustls::{self, pki_types::ServerName};

/// How the connection is secured.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Security {
    /// TLS from the start (usually port 993).
    Tls,
    /// Plain, upgraded with `STARTTLS` (usually port 143).
    StartTls,
    /// No encryption: only for a server on the same host or network.
    None,
}

/// Where and how to fetch.
#[derive(Clone)]
pub struct Config {
    pub host: String,
    pub port: u16,
    pub security: Security,
    pub username: String,
    pub password: String,
    pub folder: String,
}

impl fmt::Debug for Config {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Config")
            .field("host", &self.host)
            .field("port", &self.port)
            .field("security", &self.security)
            .field("username", &self.username)
            .field("folder", &self.folder)
            .finish_non_exhaustive()
    }
}

/// How far a mailbox has been read: the folder's UIDVALIDITY and the
/// highest UID fetched. A changed UIDVALIDITY starts over.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Cursor {
    pub uid_validity: Option<u32>,
    pub last_uid: u32,
}

#[derive(Debug, Clone)]
pub struct Fetched {
    pub uid: u32,
    pub raw: Vec<u8>,
}

#[derive(Debug, thiserror::Error)]
pub enum ImapError {
    #[error("couldn't connect to {0}: {1}")]
    Connect(String, std::io::Error),
    #[error("TLS: {0}")]
    Tls(String),
    #[error("the server refused the username or password")]
    Login,
    #[error("IMAP: {0}")]
    Protocol(String),
    #[error("the server didn't answer within {0:?}")]
    Timeout(Duration),
}

impl From<async_imap::error::Error> for ImapError {
    fn from(e: async_imap::error::Error) -> Self {
        Self::Protocol(e.to_string())
    }
}

/// A connection, TLS or not.
trait Io: AsyncRead + AsyncWrite + Unpin + Send + fmt::Debug {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send + fmt::Debug> Io for T {}

type Stream = Box<dyn Io>;

fn connector() -> TlsConnector {
    let roots = rustls::RootCertStore::from_iter(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let config = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .expect("ring supports the default protocol versions")
    .with_root_certificates(roots)
    .with_no_client_auth();
    TlsConnector::from(Arc::new(config))
}

/// Wraps a connection in TLS, checking the server's certificate.
async fn tls<S: Io + 'static>(host: &str, stream: S) -> Result<Stream, ImapError> {
    let name = ServerName::try_from(host.to_owned()).map_err(|e| ImapError::Tls(e.to_string()))?;
    let stream = connector()
        .connect(name, stream)
        .await
        .map_err(|e| ImapError::Tls(e.to_string()))?;
    Ok(Box::new(stream))
}

async fn connect(config: &Config) -> Result<async_imap::Session<Stream>, ImapError> {
    let address = format!("{}:{}", config.host, config.port);
    let plain = TcpStream::connect(&address)
        .await
        .map_err(|e| ImapError::Connect(address.clone(), e))?;
    let mut client = match config.security {
        Security::Tls => Client::new(tls(&config.host, plain).await?),
        Security::None | Security::StartTls => Client::new(Box::new(plain) as Stream),
    };
    client
        .read_response()
        .await
        .map_err(|e| ImapError::Protocol(e.to_string()))?
        .ok_or_else(|| ImapError::Protocol("no greeting".into()))?;
    if config.security == Security::StartTls {
        client.run_command_and_check_ok("STARTTLS", None).await?;
        // No greeting follows the upgrade.
        client = Client::new(tls(&config.host, client.into_inner()).await?);
    }
    client
        .login(&config.username, &config.password)
        .await
        .map_err(|_| ImapError::Login)
}

/// The UIDs to fetch from a search's results: oldest first, at most
/// `limit`, and only those after the cursor (`n:*` always includes the
/// highest message, even when it's older than `n`). Also whether more were
/// left out.
fn select(
    found: impl IntoIterator<Item = u32>,
    cursor: Cursor,
    fresh: bool,
    limit: usize,
) -> (Vec<u32>, bool) {
    let mut uids: Vec<u32> = found
        .into_iter()
        .filter(|uid| fresh || *uid > cursor.last_uid)
        .collect();
    uids.sort_unstable();
    let more = uids.len() > limit;
    uids.truncate(limit);
    (uids, more)
}

/// Up to `limit` messages new since `cursor`, oldest first, and the cursor
/// after them. The first time (or after UIDVALIDITY changes) it takes the
/// unread messages, not the folder's whole history. Fetched messages are
/// marked `\Seen`.
pub async fn fetch(
    config: &Config,
    cursor: Cursor,
    limit: usize,
    timeout: Duration,
) -> Result<(Vec<Fetched>, Cursor), ImapError> {
    tokio::time::timeout(timeout, fetch_inner(config, cursor, limit))
        .await
        .map_err(|_| ImapError::Timeout(timeout))?
}

async fn fetch_inner(
    config: &Config,
    cursor: Cursor,
    limit: usize,
) -> Result<(Vec<Fetched>, Cursor), ImapError> {
    let mut session = connect(config).await?;
    let folder = session.select(&config.folder).await?;
    let fresh = cursor.uid_validity.is_none() || cursor.uid_validity != folder.uid_validity;
    let query = if fresh {
        "UNSEEN".to_owned()
    } else {
        format!("UID {}:*", cursor.last_uid.saturating_add(1))
    };
    let found = session.uid_search(&query).await?;
    let (uids, more) = select(found, cursor, fresh, limit);

    let mut fetched = Vec::with_capacity(uids.len());
    if !uids.is_empty() {
        let set = uids
            .iter()
            .map(u32::to_string)
            .collect::<Vec<_>>()
            .join(",");
        let messages: Vec<_> = session
            .uid_fetch(&set, "(UID BODY.PEEK[])")
            .await?
            .try_collect()
            .await?;
        for message in &messages {
            if let (Some(uid), Some(body)) = (message.uid, message.body()) {
                fetched.push(Fetched {
                    uid,
                    raw: body.to_vec(),
                });
            }
        }
        fetched.sort_by_key(|f| f.uid);
        let _: Vec<_> = session
            .uid_store(&set, "+FLAGS (\\Seen)")
            .await?
            .try_collect()
            .await?;
    }
    let newest = fetched.last().map(|f| f.uid);
    let last_uid = if fresh && !more {
        // The first time: everything there now is handled once its unread
        // messages are taken.
        folder
            .uid_next
            .map(|next| next.saturating_sub(1))
            .max(newest)
            .unwrap_or(0)
    } else {
        newest
            .unwrap_or(cursor.last_uid)
            .max(if fresh { 0 } else { cursor.last_uid })
    };
    let _ = session.logout().await;
    Ok((
        fetched,
        Cursor {
            uid_validity: folder.uid_validity,
            last_uid,
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selects_what_is_new() {
        let cursor = Cursor {
            uid_validity: Some(7),
            last_uid: 10,
        };
        // A server answering `11:*` with only its highest (older) message.
        assert_eq!(select([10], cursor, false, 50), (vec![], false));
        assert_eq!(
            select([13, 11, 12, 10], cursor, false, 2),
            (vec![11, 12], true)
        );
        // Fresh: everything unread, oldest first.
        assert_eq!(select([5, 3], cursor, true, 50), (vec![3, 5], false));
    }
}
