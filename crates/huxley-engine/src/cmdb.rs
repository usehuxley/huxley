//! The `cmdb` module's steps (`core.asset.*`).

use huxley_core::{RequestContext, Via};
use huxley_db::{OrgScope, begin_scoped};
use serde_json::Value as Json;
use uuid::Uuid;

use crate::Engine;
use crate::walk::error;

fn internal(e: impl std::fmt::Display) -> Json {
    error("internal", e.to_string())
}

impl Engine {
    /// `core.asset.sync`: runs a dataset → asset mapping rule, in one
    /// transaction. Its output is what it did.
    pub(crate) async fn sync_assets(&self, input: &Json) -> Result<Json, Json> {
        let id = input["mapping_id"]
            .as_str()
            .and_then(|s| s.parse::<Uuid>().ok())
            .ok_or_else(|| error("input", "mapping_id is a mapping rule's id"))?;
        let mut tx = begin_scoped(self.db(), &OrgScope::System)
            .await
            .map_err(internal)?;
        if !huxley_modules::is_active(&mut tx, huxley_cmdb::MODULE)
            .await
            .map_err(internal)?
        {
            return Err(error(
                "module_disabled",
                "Assets (the cmdb module) are turned off",
            ));
        }
        let ctx = RequestContext::system(Via::Workflow);
        let result = huxley_cmdb::mappings::run(&mut tx, &ctx, id, false)
            .await
            .map_err(|e| match e {
                huxley_cmdb::CmdbError::NotFound => {
                    error("not_found", "there's no such mapping rule")
                }
                other => error("input", other.to_string()),
            })?;
        let m = huxley_cmdb::mappings::get(&mut tx, id)
            .await
            .map_err(internal)?;
        huxley_cmdb::mappings::announce(&mut tx, &m, &result)
            .await
            .map_err(internal)?;
        tx.commit().await.map_err(internal)?;
        serde_json::to_value(result).map_err(internal)
    }
}
