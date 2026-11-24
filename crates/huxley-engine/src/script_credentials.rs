//! A script's connection credentials (design doc §6.9): fetched when a
//! runner claims the task, so tokens are fresh, and handed over as
//! environment variables that are then scrubbed from what comes back.

use huxley_db::{OrgScope, begin_scoped};
use huxley_integrations::connections::{self, ConnectionStatus, ResolvedAuth};
use serde_json::Value as Json;
use uuid::Uuid;

use crate::walk::error;
use crate::{Engine, EngineResult};

/// Environment variables for one connection, and the values among them
/// to scrub.
#[derive(Default)]
pub(crate) struct Credentials {
    pub env: Vec<(String, String)>,
    pub secrets: Vec<String>,
}

impl Engine {
    /// `NAME_URL` plus `NAME_TOKEN`, `NAME_KEY`, or `NAME_USERNAME` and
    /// `NAME_PASSWORD` for connection `id`.
    pub(crate) async fn script_credentials(
        &self,
        name: &str,
        id: Uuid,
        org_id: Uuid,
    ) -> EngineResult<Result<Credentials, Json>> {
        let mut tx = begin_scoped(self.db(), &OrgScope::System).await?;
        let view = connections::get(&mut tx, id)
            .await
            .map_err(|e| crate::EngineError::Document(e.to_string()))?;
        let resolved = match connections::resolve_for(&mut tx, &self.crypto, id, Some(org_id)).await
        {
            Ok(resolved) => resolved,
            Err(e) => {
                tx.commit().await?;
                return Ok(Err(error("connection", e.to_string())));
            }
        };
        tx.commit().await?;
        if view.status == ConnectionStatus::NeedsAuth {
            return Ok(Err(error(
                "auth",
                format!("connection {:?} needs someone to sign it in", view.key),
            )));
        }
        let mut out = Credentials::default();
        out.env
            .push((format!("{name}_URL"), resolved.base_url.clone()));
        let mut secret = |suffix: &str, value: String| {
            out.secrets.push(value.clone());
            out.env.push((format!("{name}{suffix}"), value));
        };
        match resolved.auth {
            ResolvedAuth::None => {}
            ResolvedAuth::ApiKey { value, .. } => secret("_KEY", value),
            ResolvedAuth::Bearer { token } => secret("_TOKEN", token),
            ResolvedAuth::Basic { username, password } => {
                secret("_PASSWORD", password);
                out.env.push((format!("{name}_USERNAME"), username));
            }
            ResolvedAuth::OAuth2ClientCredentials { .. }
            | ResolvedAuth::OAuth2AuthorizationCode { .. } => {
                match self.connection_token_for(id, Some(org_id)).await {
                    Ok(Some(token)) => secret("_TOKEN", token),
                    Ok(None) => {}
                    Err(e) => return Ok(Err(crate::oauth::token_error(&e))),
                }
            }
        }
        Ok(Ok(out))
    }
}
