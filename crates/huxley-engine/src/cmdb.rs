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

    /// `core.asset.set_warranty`: records a warranty a lookup found, for an
    /// asset at the run's organization or below.
    pub(crate) async fn set_warranty(&self, org_id: Uuid, input: &Json) -> Result<Json, Json> {
        let asset = input["asset_id"]
            .as_str()
            .and_then(|s| s.parse::<Uuid>().ok())
            .ok_or_else(|| error("input", "asset_id is an asset's id"))?;
        let source = input["source"]
            .as_str()
            .ok_or_else(|| error("input", "source names the lookup, e.g. dell"))?
            .to_owned();
        let mut fields = input.clone();
        if let Some(o) = fields.as_object_mut() {
            o.remove("asset_id");
            o.remove("source");
        }
        let warranty: huxley_cmdb::warranties::WarrantyInput = serde_json::from_value(
            serde_json::to_value(serde_json::json!({
                "endsOn": fields["ends_on"], "startsOn": fields.get("starts_on"), "kind": fields.get("kind"),
                "provider": fields.get("provider"), "reference": fields.get("reference"), "notes": fields.get("notes"),
            }))
            .map_err(internal)?,
        )
        .map_err(|e| error("input", format!("the warranty: {e}")))?;
        let mut tx = begin_scoped(self.db(), &OrgScope::System)
            .await
            .map_err(internal)?;
        if !huxley_modules::is_active_at(&mut tx, huxley_cmdb::MODULE, org_id)
            .await
            .map_err(internal)?
        {
            return Err(error(
                "module_disabled",
                "Assets (the cmdb module) are turned off for this organization",
            ));
        }
        let within: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM assets a JOIN organizations o ON o.id = a.org_id \
               WHERE a.id = $1 AND o.path <@ (SELECT path FROM organizations WHERE id = $2))",
        )
        .bind(asset)
        .bind(org_id)
        .fetch_one(&mut *tx)
        .await
        .map_err(internal)?;
        if !within {
            return Err(error(
                "not_found",
                "there's no such asset at this organization or below",
            ));
        }
        let ctx = RequestContext::system(Via::Workflow);
        let w = huxley_cmdb::warranties::record(&mut tx, &ctx, asset, &source, &warranty)
            .await
            .map_err(|e| error("input", e.to_string()))?;
        tx.commit().await.map_err(internal)?;
        serde_json::to_value(w).map_err(internal)
    }
}
