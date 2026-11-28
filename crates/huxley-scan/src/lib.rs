//! Malware scanning of uploaded files (ADR-094).
//!
//! The operator picks a scanner in configuration (`[uploads.scan]`): none,
//! a ClamAV daemon, or OPSWAT MetaDefender (Core on their own servers, or
//! the hosted Cloud). Huxley queues every new upload for a scan and holds
//! it (or not, by configuration) until the scanner calls it clean.
//!
//! Scanners are configured by the operator, not in the app: they're usually
//! on the internal network, so the address isn't something an admin in the
//! browser should be able to point anywhere (no egress policy applies).

mod clamav;
mod metadefender;

use std::time::Duration;

use secrecy::SecretString;
use serde::Deserialize;

pub use clamav::parse_reply as parse_clamav_reply;
pub use metadefender::{Analysis, interpret as interpret_metadefender};

/// Which scanner checks uploads.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Provider {
    /// No scanning (the default): uploads are available at once, unscanned.
    #[default]
    Off,
    Clamav,
    Metadefender,
}

/// What happens to a file that hasn't been called clean yet (just
/// uploaded, or the scanner is unreachable).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WhilePending {
    /// It can't be downloaded until it's scanned clean (the default).
    #[default]
    Hold,
    /// It can be downloaded until a scan finds something. For when the
    /// scanner being down mustn't stop people working.
    Allow,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ScanConfig {
    pub provider: Provider,
    pub while_pending: WhilePending,
    pub clamav: ClamavConfig,
    pub metadefender: MetaDefenderConfig,
}

impl ScanConfig {
    pub fn enabled(&self) -> bool {
        self.provider != Provider::Off
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct ClamavConfig {
    /// `tcp://host:port` (or `host:port`), or `unix:/path/to/clamd.sock`.
    pub address: String,
    pub timeout_seconds: u64,
}

impl Default for ClamavConfig {
    fn default() -> Self {
        Self {
            address: "tcp://localhost:3310".into(),
            timeout_seconds: 120,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct MetaDefenderConfig {
    /// MetaDefender Cloud by default; a Core server is e.g.
    /// `http://metadefender:8008`.
    pub url: String,
    /// The `apikey` header. Cloud needs one; Core may not.
    pub api_key: Option<SecretString>,
    /// Core: the workflow rule to scan with (the `rule` header).
    pub rule: Option<String>,
    /// How long to wait for a result before trying again later.
    pub timeout_seconds: u64,
    /// How often to ask for the result.
    pub poll_milliseconds: u64,
}

impl Default for MetaDefenderConfig {
    fn default() -> Self {
        Self {
            url: "https://api.metadefender.com/v4".into(),
            api_key: None,
            rule: None,
            timeout_seconds: 300,
            poll_milliseconds: 1000,
        }
    }
}

/// A scanner's answer about one file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    Clean,
    /// Not clean: the threats found, or why the scanner wouldn't pass it.
    Infected(String),
}

/// The scan didn't produce a verdict; try again later.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct ScanError(pub String);

/// A configured scanner.
#[derive(Debug, Clone)]
pub enum Scanner {
    Clamav(clamav::Clamav),
    MetaDefender(metadefender::MetaDefender),
}

impl Scanner {
    /// The configured scanner, or `None` with scanning off.
    pub fn from_config(config: &ScanConfig) -> Result<Option<Self>, ScanError> {
        Ok(match config.provider {
            Provider::Off => None,
            Provider::Clamav => Some(Self::Clamav(clamav::Clamav::new(&config.clamav)?)),
            Provider::Metadefender => Some(Self::MetaDefender(metadefender::MetaDefender::new(
                &config.metadefender,
            )?)),
        })
    }

    /// A short name for records: `clamav` or `metadefender`.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Clamav(_) => "clamav",
            Self::MetaDefender(_) => "metadefender",
        }
    }

    pub async fn scan(&self, data: &[u8], filename: &str) -> Result<Verdict, ScanError> {
        match self {
            Self::Clamav(c) => c.scan(data).await,
            Self::MetaDefender(m) => m.scan(data, filename).await,
        }
    }

    /// Checks the scanner answers; its version, if it says.
    pub async fn check(&self) -> Result<String, ScanError> {
        match self {
            Self::Clamav(c) => c.version().await,
            Self::MetaDefender(m) => m.check().await,
        }
    }
}

fn secs(n: u64) -> Duration {
    Duration::from_secs(n.max(1))
}
