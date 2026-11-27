//! Browsing pack registries and reviewing packs from them (ADR-093). A
//! registry's index is fetched (and its signature checked, when the
//! registry has a key), cached for five minutes, and shown with what each
//! organization has installed. Reviewing downloads the pack, checks its
//! SHA-256 against the index, and hands it to the same review as an upload.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use huxley_packs::registry::{self, Index, IndexedPack, IndexedVersion, RegistryConfig};

/// The largest index Huxley reads.
const MAX_INDEX_BYTES: usize = 5 * 1024 * 1024;
/// The largest pack (as `Pack::read` allows).
const MAX_PACK_BYTES: usize = 20 * 1024 * 1024;
const CACHE_FOR: Duration = Duration::from_secs(300);

type Cached = (Instant, Result<Arc<Index>, String>);

#[derive(Debug)]
pub struct PackRegistries {
    pub configs: Vec<RegistryConfig>,
    client: reqwest::Client,
    cache: Mutex<HashMap<usize, Cached>>,
}

impl PackRegistries {
    pub fn new(configs: Vec<RegistryConfig>) -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .connect_timeout(Duration::from_secs(10))
            .redirect(reqwest::redirect::Policy::limited(3))
            .user_agent(concat!("huxley/", env!("CARGO_PKG_VERSION")))
            .build()
            .expect("the HTTP client for pack registries builds");
        Self {
            configs,
            client,
            cache: Mutex::new(HashMap::new()),
        }
    }

    async fn get(&self, url: &str, max: usize) -> Result<Vec<u8>, String> {
        let response = self
            .client
            .get(url)
            .send()
            .await
            .map_err(|e| format!("couldn't reach {}: {}", host(url), e.without_url()))?;
        if !response.status().is_success() {
            return Err(format!("{} answered {}", host(url), response.status()));
        }
        if response.content_length().is_some_and(|n| n > max as u64) {
            return Err(format!("{url} is larger than {} MB", max / 1024 / 1024));
        }
        let bytes = response
            .bytes()
            .await
            .map_err(|e| format!("reading {} failed: {}", host(url), e.without_url()))?;
        if bytes.len() > max {
            return Err(format!("{url} is larger than {} MB", max / 1024 / 1024));
        }
        Ok(bytes.to_vec())
    }

    async fn fetch(&self, config: &RegistryConfig) -> Result<Index, String> {
        let bytes = self.get(&config.url, MAX_INDEX_BYTES).await?;
        if let Some(key) = config.key.as_deref().filter(|k| !k.trim().is_empty()) {
            let signature = self
                .get(&format!("{}.sig", config.url), 64 * 1024)
                .await
                .map_err(|e| format!("the index's signature: {e}"))?;
            registry::verify_index(&bytes, &signature, key)
                .map_err(|_| "the index's signature doesn't match the registry's key".to_owned())?;
        }
        registry::read_index(&bytes).map_err(|e| e.to_string())
    }

    /// Registry `n`'s index, from the cache when it's fresh.
    pub async fn index(&self, n: usize) -> Result<Arc<Index>, String> {
        let config = self.configs.get(n).ok_or("no such registry")?;
        if let Some((at, cached)) = self.cache.lock().expect("cache lock").get(&n)
            && at.elapsed() < CACHE_FOR
        {
            return cached.clone();
        }
        let fresh = self.fetch(config).await.map(Arc::new);
        self.cache
            .lock()
            .expect("cache lock")
            .insert(n, (Instant::now(), fresh.clone()));
        fresh
    }

    /// Downloads a version from registry `n` and checks it against the
    /// index.
    pub async fn download(&self, n: usize, version: &IndexedVersion) -> Result<Vec<u8>, String> {
        let base = reqwest::Url::parse(&self.configs[n].url).map_err(|e| e.to_string())?;
        let url = base
            .join(&version.url)
            .map_err(|_| format!("bad download URL {:?}", version.url))?;
        if !matches!(url.scheme(), "https" | "http") {
            return Err(format!("bad download URL {url}"));
        }
        let bytes = self.get(url.as_str(), MAX_PACK_BYTES).await?;
        registry::check_download(version, &bytes).map_err(|e| e.to_string())?;
        Ok(bytes)
    }
}

fn host(url: &str) -> String {
    reqwest::Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(str::to_owned))
        .unwrap_or_else(|| url.to_owned())
}

/// Finds `key` at `version` in an index.
pub fn find<'a>(
    index: &'a Index,
    key: &str,
    version: &str,
) -> Option<(&'a IndexedPack, &'a IndexedVersion)> {
    let pack = index.packs.iter().find(|p| p.key == key)?;
    let version = pack.versions.iter().find(|v| v.version == version)?;
    Some((pack, version))
}
