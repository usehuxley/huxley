//! Payload spill (design doc §6.4). A step input or output whose JSON is
//! larger than the spill threshold goes to object storage. Its row keeps a
//! small marker (`{"$spilled": {"bytes": n}}`) in place of the value, and
//! the object's key in `input_ref` / `output_ref`. The journal loads spilled
//! values back when a run advances, so expressions see the real value.
//!
//! Encrypted (secret-tainted) values aren't spilled: they stay sealed in
//! the row. Objects are written before the row that names them commits, so
//! a rolled-back transaction can leave an unreferenced object behind under
//! the run's prefix (`workflow-payloads/<org>/<run>/`); run retention
//! removes the prefix with the run.

use huxley_storage::Storage;
use serde_json::{Value as Json, json};
use uuid::Uuid;

use crate::{EngineError, EngineResult};

/// The marker's key in a row whose value was spilled.
pub const SPILLED: &str = "$spilled";

/// Spill settings.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PayloadConfig {
    /// Inputs and outputs larger than this many kilobytes of JSON go to
    /// object storage. 0 keeps everything in the database.
    pub spill_kb: u64,
}

impl Default for PayloadConfig {
    fn default() -> Self {
        Self { spill_kb: 256 }
    }
}

#[derive(Debug, Clone, Default)]
pub(crate) struct Payloads {
    storage: Option<Storage>,
    spill_bytes: usize,
}

/// What a row stores for a value that was spilled.
pub(crate) struct Spilled {
    pub marker: Json,
    pub key: String,
}

impl Payloads {
    /// Object storage, if it's set up.
    pub(crate) const fn storage(&self) -> Option<&Storage> {
        self.storage.as_ref()
    }

    pub(crate) fn new(storage: Option<Storage>, config: &PayloadConfig) -> Self {
        Self {
            storage,
            spill_bytes: usize::try_from(config.spill_kb.saturating_mul(1024))
                .unwrap_or(usize::MAX),
        }
    }

    /// Spills `value` if it's over the threshold (and storage is set up).
    /// `None`: store it in the row as it is.
    pub(crate) async fn spill(
        &self,
        org_id: Uuid,
        run_id: Uuid,
        value: &Json,
    ) -> EngineResult<Option<Spilled>> {
        let Some(storage) = &self.storage else {
            return Ok(None);
        };
        if self.spill_bytes == 0 {
            return Ok(None);
        }
        let bytes = serde_json::to_vec(value).map_err(|e| EngineError::Document(e.to_string()))?;
        if bytes.len() <= self.spill_bytes {
            return Ok(None);
        }
        let key = format!(
            "workflow-payloads/{org_id}/{run_id}/{}.json",
            Uuid::now_v7()
        );
        let size = bytes.len();
        storage
            .put(&key, bytes.into())
            .await
            .map_err(|e| EngineError::Storage(e.to_string()))?;
        Ok(Some(Spilled {
            marker: json!({ SPILLED: { "bytes": size } }),
            key,
        }))
    }

    /// Reads a spilled value back.
    pub(crate) async fn load(&self, key: &str) -> EngineResult<Json> {
        let storage = self.storage.as_ref().ok_or_else(|| {
            EngineError::Storage(format!(
                "a step payload is in object storage ({key}), but this process has none configured"
            ))
        })?;
        let bytes = storage
            .get(key)
            .await
            .map_err(|e| EngineError::Storage(format!("{key}: {e}")))?;
        serde_json::from_slice(&bytes).map_err(|e| EngineError::Document(e.to_string()))
    }

    /// Stores `value` for a row: the value itself, or a marker plus the key.
    pub(crate) async fn store(
        &self,
        org_id: Uuid,
        run_id: Uuid,
        value: Option<&Json>,
    ) -> EngineResult<(Option<Json>, Option<String>)> {
        let Some(value) = value else {
            return Ok((None, None));
        };
        Ok(match self.spill(org_id, run_id, value).await? {
            Some(spilled) => (Some(spilled.marker), Some(spilled.key)),
            None => (Some(value.clone()), None),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn large_values_spill_and_load_back() {
        let payloads = Payloads::new(Some(Storage::memory()), &PayloadConfig { spill_kb: 1 });
        let (org, run) = (Uuid::now_v7(), Uuid::now_v7());
        let small = json!({ "a": 1 });
        assert!(payloads.spill(org, run, &small).await.unwrap().is_none());

        let large = json!({ "body": "x".repeat(2000) });
        let spilled = payloads.spill(org, run, &large).await.unwrap().unwrap();
        assert!(
            spilled
                .key
                .starts_with(&format!("workflow-payloads/{org}/{run}/"))
        );
        assert!(spilled.marker[SPILLED]["bytes"].as_u64().unwrap() > 2000);
        assert_eq!(payloads.load(&spilled.key).await.unwrap(), large);
    }

    #[tokio::test]
    async fn nothing_spills_without_storage_or_with_zero() {
        let large = json!("x".repeat(5000));
        let (org, run) = (Uuid::now_v7(), Uuid::now_v7());
        let none = Payloads::new(None, &PayloadConfig { spill_kb: 1 });
        assert!(none.spill(org, run, &large).await.unwrap().is_none());
        assert!(none.load("anything").await.is_err());
        let off = Payloads::new(Some(Storage::memory()), &PayloadConfig { spill_kb: 0 });
        assert!(off.spill(org, run, &large).await.unwrap().is_none());
    }
}
