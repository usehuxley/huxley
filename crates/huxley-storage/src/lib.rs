//! Object storage (design doc §3: the `object_store` crate): attachments
//! today; spilled workflow payloads and dataset files later.
//!
//! Configuration (`[storage]`, or `HUXLEY_STORAGE__*`):
//! - `url`: where objects go. `file:///var/lib/huxley/objects` (the default
//!   is `./data/objects` relative to the working directory),
//!   `s3://bucket` (AWS S3, DigitalOcean Spaces, MinIO: set
//!   `options.aws_endpoint` for non-AWS), or `az://container`.
//! - `options`: provider settings and credentials, e.g. `aws_access_key_id`,
//!   `aws_secret_access_key`, `aws_region`, `aws_endpoint`,
//!   `azure_storage_account_name`, `azure_storage_access_key`. Supports
//!   `HUXLEY_STORAGE__OPTIONS__<NAME>_FILE` like every other setting.

use std::collections::BTreeMap;
use std::sync::Arc;

use bytes::Bytes;
use object_store::path::Path;
use object_store::{ObjectStore, ObjectStoreExt as _, PutPayload};
use secrecy::{ExposeSecret as _, SecretString};
use serde::Deserialize;

/// Storage settings.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct StorageConfig {
    pub url: String,
    /// Provider options and credentials (see the module docs).
    pub options: BTreeMap<String, SecretString>,
    /// Largest accepted upload, in megabytes.
    pub max_upload_mb: u64,
}

impl Default for StorageConfig {
    fn default() -> Self {
        Self {
            url: "./data/objects".into(),
            options: BTreeMap::new(),
            max_upload_mb: 25,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    #[error("invalid storage.url: {0}")]
    InvalidUrl(String),
    #[error("object not found")]
    NotFound,
    #[error("object storage failed: {0}")]
    Store(String),
}

impl From<object_store::Error> for StorageError {
    fn from(error: object_store::Error) -> Self {
        match error {
            object_store::Error::NotFound { .. } => Self::NotFound,
            other => Self::Store(other.to_string()),
        }
    }
}

pub type StorageResult<T> = Result<T, StorageError>;

/// Puts, gets, and deletes objects. Cheap to clone.
#[derive(Clone)]
pub struct Storage {
    store: Arc<dyn ObjectStore>,
    /// Prefix inside the store (from the URL path, for cloud stores).
    prefix: Path,
    max_upload_bytes: u64,
    description: String,
}

impl std::fmt::Debug for Storage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Storage")
            .field("store", &self.description)
            .field("prefix", &self.prefix)
            .field("max_upload_bytes", &self.max_upload_bytes)
            .finish_non_exhaustive()
    }
}

impl Storage {
    pub fn from_config(config: &StorageConfig) -> StorageResult<Self> {
        let max_upload_bytes = config.max_upload_mb.saturating_mul(1024 * 1024);
        let raw = config.url.trim();
        // A bare path means the local filesystem.
        let url = if raw.contains("://") {
            url::Url::parse(raw).map_err(|e| StorageError::InvalidUrl(e.to_string()))?
        } else {
            std::fs::create_dir_all(raw)
                .map_err(|e| StorageError::InvalidUrl(format!("{raw}: {e}")))?;
            let absolute =
                std::fs::canonicalize(raw).map_err(|e| StorageError::InvalidUrl(e.to_string()))?;
            url::Url::from_directory_path(&absolute).map_err(|()| {
                StorageError::InvalidUrl(format!("{} isn't a directory path", absolute.display()))
            })?
        };
        if url.scheme() == "file" {
            let dir = url
                .to_file_path()
                .map_err(|()| StorageError::InvalidUrl(raw.to_owned()))?;
            std::fs::create_dir_all(&dir)
                .map_err(|e| StorageError::InvalidUrl(format!("{}: {e}", dir.display())))?;
            let store = object_store::local::LocalFileSystem::new_with_prefix(&dir)
                .map_err(|e| StorageError::InvalidUrl(e.to_string()))?;
            return Ok(Self {
                store: Arc::new(store),
                prefix: Path::default(),
                max_upload_bytes,
                description: format!("local filesystem at {}", dir.display()),
            });
        }
        let options = config
            .options
            .iter()
            .map(|(k, v)| (k.to_ascii_lowercase(), v.expose_secret().to_owned()));
        let (store, prefix) = object_store::parse_url_opts(&url, options)
            .map_err(|e| StorageError::InvalidUrl(e.to_string()))?;
        Ok(Self {
            store: Arc::from(store),
            prefix,
            max_upload_bytes,
            description: format!("{}://{}", url.scheme(), url.host_str().unwrap_or_default()),
        })
    }

    /// In-memory storage for tests.
    pub fn memory() -> Self {
        Self {
            store: Arc::new(object_store::memory::InMemory::new()),
            prefix: Path::default(),
            max_upload_bytes: 25 * 1024 * 1024,
            description: "memory".into(),
        }
    }

    /// Changes the upload limit (tests, mostly).
    #[must_use]
    pub fn with_max_upload_bytes(mut self, bytes: u64) -> Self {
        self.max_upload_bytes = bytes;
        self
    }

    pub fn max_upload_bytes(&self) -> u64 {
        self.max_upload_bytes
    }

    fn path(&self, key: &str) -> Path {
        let mut path = self.prefix.clone();
        for part in key.split('/') {
            path = path.clone().join(part);
        }
        path
    }

    pub async fn put(&self, key: &str, data: Bytes) -> StorageResult<()> {
        self.store
            .put(&self.path(key), PutPayload::from(data))
            .await?;
        Ok(())
    }

    pub async fn get(&self, key: &str) -> StorageResult<Bytes> {
        Ok(self.store.get(&self.path(key)).await?.bytes().await?)
    }

    /// Deletes an object. Missing objects are fine.
    pub async fn delete(&self, key: &str) -> StorageResult<()> {
        match self.store.delete(&self.path(key)).await {
            Ok(()) | Err(object_store::Error::NotFound { .. }) => Ok(()),
            Err(e) => Err(e.into()),
        }
    }

    /// Deletes every object under `prefix` (a `/`-separated key prefix,
    /// e.g. a run's payload folder). Returns how many it deleted.
    pub async fn delete_prefix(&self, prefix: &str) -> StorageResult<usize> {
        use futures_util::TryStreamExt as _;
        let objects: Vec<object_store::ObjectMeta> = self
            .store
            .list(Some(&self.path(prefix.trim_end_matches('/'))))
            .try_collect()
            .await?;
        for o in &objects {
            match self.store.delete(&o.location).await {
                Ok(()) | Err(object_store::Error::NotFound { .. }) => {}
                Err(e) => return Err(e.into()),
            }
        }
        Ok(objects.len())
    }

    /// Writes and deletes a probe object: checks the store is reachable and
    /// writable. Run at startup.
    pub async fn check(&self) -> StorageResult<()> {
        let key = format!("_huxley/probe-{}", std::process::id());
        self.put(&key, Bytes::from_static(b"ok")).await?;
        self.delete(&key).await
    }

    pub fn describe(&self) -> &str {
        &self.description
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn prefixes_are_deleted_whole() {
        let s = Storage::memory();
        for k in ["runs/a/1.json", "runs/a/2.json", "runs/b/1.json"] {
            s.put(k, Bytes::from_static(b"x")).await.unwrap();
        }
        assert_eq!(s.delete_prefix("runs/a/").await.unwrap(), 2);
        assert!(s.get("runs/a/1.json").await.is_err());
        assert!(s.get("runs/b/1.json").await.is_ok());
    }

    #[tokio::test]
    async fn memory_round_trip() {
        let s = Storage::memory();
        s.put("a/b/c", Bytes::from_static(b"hello")).await.unwrap();
        assert_eq!(&s.get("a/b/c").await.unwrap()[..], b"hello");
        s.delete("a/b/c").await.unwrap();
        assert!(matches!(s.get("a/b/c").await, Err(StorageError::NotFound)));
        s.delete("a/b/c").await.unwrap(); // already gone is fine
    }

    #[tokio::test]
    async fn local_filesystem_from_a_bare_path() {
        let dir = std::env::temp_dir().join(format!("huxley-storage-test-{}", std::process::id()));
        let config = StorageConfig {
            url: dir.to_string_lossy().into_owned(),
            ..StorageConfig::default()
        };
        let s = Storage::from_config(&config).unwrap();
        s.check().await.unwrap();
        s.put("attachments/x", Bytes::from_static(b"data"))
            .await
            .unwrap();
        assert!(dir.join("attachments").join("x").exists());
        assert_eq!(&s.get("attachments/x").await.unwrap()[..], b"data");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn s3_urls_parse_with_options() {
        let mut options = BTreeMap::new();
        options.insert("aws_region".to_owned(), SecretString::from("us-east-1"));
        options.insert(
            "aws_endpoint".to_owned(),
            SecretString::from("https://nyc3.digitaloceanspaces.com"),
        );
        let config = StorageConfig {
            url: "s3://huxley-files/prod".into(),
            options,
            max_upload_mb: 10,
        };
        let s = Storage::from_config(&config).unwrap();
        assert_eq!(s.max_upload_bytes(), 10 * 1024 * 1024);
        assert_eq!(s.prefix.as_ref(), "prod");
        assert!(
            Storage::from_config(&StorageConfig {
                url: "ftp://nope".into(),
                ..StorageConfig::default()
            })
            .is_err()
        );
    }
}
