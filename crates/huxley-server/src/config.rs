//! Configuration loading.
//!
//! Sources, lowest to highest precedence:
//! 1. Built-in defaults
//! 2. TOML file (`--config`, `HUXLEY_CONFIG`, or `./huxley.toml` if present)
//! 3. Environment variables: `HUXLEY_<SECTION>__<KEY>`, e.g. `HUXLEY_DATABASE__URL`
//! 4. File-backed environment variables: `HUXLEY_<SECTION>__<KEY>_FILE` holds a
//!    path whose contents become the value. This is how Docker and Kubernetes
//!    secrets are passed without putting them in the environment.

use std::net::SocketAddr;
use std::path::Path;

use anyhow::{Context as _, anyhow};
use figment::Figment;
use figment::providers::{Env, Format as _, Serialized, Toml};
use huxley_db::DatabaseConfig;
use huxley_telemetry::TelemetryConfig;
use serde::Deserialize;

const ENV_PREFIX: &str = "HUXLEY_";
const FILE_SUFFIX: &str = "_FILE";

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub http: HttpConfig,
    /// Optional here so commands can report a clear error when it is missing.
    pub database: Option<DatabaseConfig>,
    #[serde(default)]
    pub telemetry: TelemetryConfig,
    /// Apply migrations at startup. Convenient for single-node installs; larger
    /// deployments run `huxley migrate` as a separate step before rollout.
    #[serde(default)]
    pub migrate_on_start: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct HttpConfig {
    pub bind: SocketAddr,
    /// Seconds to wait for in-flight requests and jobs on shutdown.
    pub shutdown_grace_secs: u64,
}

impl Default for HttpConfig {
    fn default() -> Self {
        Self {
            bind: SocketAddr::from(([0, 0, 0, 0], 8080)),
            shutdown_grace_secs: 30,
        }
    }
}

impl Config {
    pub fn load(path: Option<&Path>) -> anyhow::Result<Self> {
        Self::from_figment(&figment(path, std::env::vars())?)
    }

    fn from_figment(figment: &Figment) -> anyhow::Result<Self> {
        figment.extract().context("invalid configuration")
    }

    pub fn database(&self) -> anyhow::Result<&DatabaseConfig> {
        self.database.as_ref().ok_or_else(|| {
            anyhow!(
                "database is not configured: set HUXLEY_DATABASE__URL (or _FILE) or [database] url"
            )
        })
    }
}

fn figment(
    path: Option<&Path>,
    env: impl Iterator<Item = (String, String)>,
) -> anyhow::Result<Figment> {
    let mut figment = Figment::new();

    match path {
        Some(path) => {
            if !path.exists() {
                return Err(anyhow!("config file not found: {}", path.display()));
            }
            figment = figment.merge(Toml::file_exact(path));
        }
        None => figment = figment.merge(Toml::file("huxley.toml")),
    }

    figment = figment.merge(
        Env::prefixed(ENV_PREFIX)
            .split("__")
            .ignore(&["config", "roles"])
            .filter(|key| !key.as_str().ends_with("_file")),
    );

    for (name, file) in env {
        let Some(key) = name
            .strip_prefix(ENV_PREFIX)
            .and_then(|k| k.strip_suffix(FILE_SUFFIX))
        else {
            continue;
        };
        let contents = std::fs::read_to_string(&file)
            .with_context(|| format!("{name}: failed to read {file}"))?;
        let path = key.to_lowercase().replace("__", ".");
        figment = figment.merge(Serialized::default(&path, contents.trim_end()));
    }

    Ok(figment)
}

#[cfg(test)]
#[allow(clippy::result_large_err)] // figment::Jail closures return figment::Error
mod tests {
    use super::*;
    use secrecy::ExposeSecret as _;

    #[test]
    fn defaults_apply_without_any_sources() {
        figment::Jail::expect_with(|_jail| {
            let config = Config::from_figment(&figment(None, std::iter::empty()).unwrap()).unwrap();
            assert_eq!(config.http.bind.port(), 8080);
            assert!(config.database.is_none());
            assert!(config.database().is_err());
            Ok(())
        });
    }

    #[test]
    fn env_overrides_toml() {
        figment::Jail::expect_with(|jail| {
            jail.create_file(
                "huxley.toml",
                "[http]\nbind = \"127.0.0.1:9000\"\n[database]\nurl = \"postgres://toml\"\n",
            )?;
            jail.set_env("HUXLEY_HTTP__BIND", "127.0.0.1:9100");
            let config = Config::load(None).unwrap();
            assert_eq!(config.http.bind.port(), 9100);
            assert!(config.database().is_ok());
            Ok(())
        });
    }

    #[test]
    fn file_env_vars_supply_secrets() {
        figment::Jail::expect_with(|jail| {
            jail.create_file("db-url", "postgres://from-file\n")?;
            let path = jail.directory().join("db-url");
            let env = [(
                "HUXLEY_DATABASE__URL_FILE".to_owned(),
                path.display().to_string(),
            )];
            let config = Config::from_figment(&figment(None, env.into_iter()).unwrap()).unwrap();
            let db = config.database().unwrap();
            // Trailing newline from the file is trimmed.
            assert_eq!(db.url.expose_secret(), "postgres://from-file");
            // Debug output must never reveal the secret.
            assert!(!format!("{db:?}").contains("from-file"));
            Ok(())
        });
    }
}
