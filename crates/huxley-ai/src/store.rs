//! Organizations' AI providers and the call log. A provider set at an
//! organization serves it and every organization below that has none of
//! its own. Keys are encrypted and never shown again.

use huxley_core::RequestContext;
use huxley_crypto::{Binding, Crypto};
use huxley_db::audit::{self, AuditRecord};
use huxley_db::{OrgScope, begin_scoped};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::{PgConnection, PgPool};
use time::OffsetDateTime;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::provider::{self, AiError, Completion, Endpoint, ProviderKind, Request};

const KEY_PURPOSE: &str = "ai_provider_key";

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("{1}")]
    Invalid(String, String),
    #[error("no AI provider is set up here")]
    NotFound,
    #[error(transparent)]
    Db(#[from] sqlx::Error),
    #[error("{0}")]
    Crypto(String),
}

fn invalid(at: &str, message: impl Into<String>) -> StoreError {
    StoreError::Invalid(at.into(), message.into())
}

/// Settings for an organization's provider.
#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schema(as = AiProviderInput)]
pub struct ProviderInput {
    pub kind: ProviderKind,
    /// Default: the provider's default model, where it has one.
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub base_url: Option<String>,
    #[serde(default)]
    pub deployment: Option<String>,
    /// The API key. Required when first set; omit to keep the current one.
    #[serde(default)]
    pub api_key: Option<String>,
    #[serde(default)]
    pub monthly_token_budget: Option<i64>,
}

/// A provider as shown (never with its key).
#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
#[schema(as = AiProviderView)]
pub struct ProviderView {
    pub id: Uuid,
    pub org_id: Uuid,
    pub kind: ProviderKind,
    pub model: String,
    pub base_url: Option<String>,
    pub deployment: Option<String>,
    pub monthly_token_budget: Option<i64>,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

const VIEW: &str =
    "id, org_id, kind, model, base_url, deployment, monthly_token_budget, updated_at";

fn check(input: &ProviderInput) -> Result<String, StoreError> {
    let model = input
        .model
        .clone()
        .filter(|m| !m.trim().is_empty())
        .or_else(|| input.kind.default_model().map(str::to_owned))
        .ok_or_else(|| invalid("model", "name the model (or the deployment's model)"))?;
    if model.len() > 200 {
        return Err(invalid("model", "at most 200 characters"));
    }
    match (input.kind, &input.base_url) {
        (ProviderKind::AzureOpenai | ProviderKind::OpenaiCompatible, None) => {
            return Err(invalid("baseUrl", "this provider needs its base URL"));
        }
        (_, Some(url)) if !url.starts_with("https://") && !url.starts_with("http://") => {
            return Err(invalid("baseUrl", "an http(s) URL"));
        }
        _ => {}
    }
    if input.monthly_token_budget.is_some_and(|b| b <= 0) {
        return Err(invalid(
            "monthlyTokenBudget",
            "a positive number of tokens, or none",
        ));
    }
    Ok(model)
}

/// Sets `org_id`'s provider (creating or replacing it).
pub async fn set(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    crypto: &Crypto,
    org_id: Uuid,
    input: &ProviderInput,
) -> Result<ProviderView, StoreError> {
    let model = check(input)?;
    let existing: Option<Uuid> =
        sqlx::query_scalar("SELECT id FROM ai_providers WHERE org_id = $1")
            .bind(org_id)
            .fetch_optional(&mut *conn)
            .await?;
    let id = existing.unwrap_or_else(huxley_core::new_id);
    let key = input
        .api_key
        .as_deref()
        .map(str::trim)
        .filter(|k| !k.is_empty());
    let sealed = match key {
        Some(key) => Some(
            crypto
                .encrypt(
                    org_id,
                    Binding {
                        purpose: KEY_PURPOSE,
                        record: id,
                    },
                    key.as_bytes(),
                )
                .await
                .map_err(|e| StoreError::Crypto(e.to_string()))?,
        ),
        None if existing.is_none() => return Err(invalid("apiKey", "the provider's API key")),
        None => None,
    };
    if existing.is_some() {
        // Without a new key, the current one stays.
        sqlx::query(
            "UPDATE ai_providers SET kind = $2, model = $3, base_url = $4, deployment = $5, \
                    key_sealed = COALESCE($6, key_sealed), monthly_token_budget = $7, updated_at = now() \
              WHERE id = $1",
        )
        .bind(id)
        .bind(input.kind)
        .bind(&model)
        .bind(&input.base_url)
        .bind(&input.deployment)
        .bind(sealed.as_deref())
        .bind(input.monthly_token_budget)
        .execute(&mut *conn)
        .await?;
    } else {
        sqlx::query(
            "INSERT INTO ai_providers (id, org_id, kind, model, base_url, deployment, key_sealed, monthly_token_budget, created_by) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
        )
        .bind(id)
        .bind(org_id)
        .bind(input.kind)
        .bind(&model)
        .bind(&input.base_url)
        .bind(&input.deployment)
        .bind(sealed.as_deref())
        .bind(input.monthly_token_budget)
        .bind(ctx.actor.id())
        .execute(&mut *conn)
        .await?;
    }
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new("ai.provider_set")
            .org(org_id)
            .target("ai_provider", id)
            .details(json!({ "kind": input.kind, "model": model, "keyChanged": key.is_some() })),
    )
    .await
    .map_err(|e| StoreError::Db(sqlx::Error::Protocol(e.to_string())))?;
    own(conn, org_id).await?.ok_or(StoreError::NotFound)
}

/// Removes `org_id`'s own provider (it falls back to one above, if any).
pub async fn remove(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    org_id: Uuid,
) -> Result<(), StoreError> {
    let removed = sqlx::query("DELETE FROM ai_providers WHERE org_id = $1")
        .bind(org_id)
        .execute(&mut *conn)
        .await?;
    if removed.rows_affected() == 0 {
        return Err(StoreError::NotFound);
    }
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new("ai.provider_removed").org(org_id),
    )
    .await
    .map_err(|e| StoreError::Db(sqlx::Error::Protocol(e.to_string())))?;
    Ok(())
}

/// `org_id`'s own provider.
pub async fn own(
    conn: &mut PgConnection,
    org_id: Uuid,
) -> Result<Option<ProviderView>, StoreError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {VIEW} FROM ai_providers WHERE org_id = $1"
    )))
    .bind(org_id)
    .fetch_optional(conn)
    .await?)
}

/// The provider serving `org_id`: its own, or the nearest above it.
pub async fn effective(
    conn: &mut PgConnection,
    org_id: Uuid,
) -> Result<Option<ProviderView>, StoreError> {
    let columns = VIEW
        .split(", ")
        .map(|c| format!("p.{c}"))
        .collect::<Vec<_>>()
        .join(", ");
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {columns} FROM ai_providers p JOIN organizations o ON o.id = p.org_id \
          WHERE o.path @> (SELECT path FROM organizations WHERE id = $1) \
          ORDER BY nlevel(o.path) DESC LIMIT 1"
    )))
    .bind(org_id)
    .fetch_optional(conn)
    .await?)
}

/// Tokens a provider has used this calendar month (UTC).
pub async fn used_this_month(
    conn: &mut PgConnection,
    provider_id: Uuid,
) -> Result<i64, StoreError> {
    Ok(sqlx::query_scalar(
        "SELECT coalesce(sum(input_tokens + output_tokens), 0)::bigint FROM ai_calls \
          WHERE provider_id = $1 AND created_at >= date_trunc('month', now() AT TIME ZONE 'UTC') AT TIME ZONE 'UTC'",
    )
    .bind(provider_id)
    .fetch_one(conn)
    .await?)
}

/// A logged call.
#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
#[schema(as = AiCall)]
pub struct CallView {
    pub id: Uuid,
    pub org_id: Uuid,
    pub feature: String,
    pub run_id: Option<Uuid>,
    pub model: String,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub outcome: String,
    pub error: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

/// Recent calls visible in the transaction's scope.
pub async fn calls(
    conn: &mut PgConnection,
    org_id: Uuid,
    limit: i64,
) -> Result<Vec<CallView>, StoreError> {
    Ok(sqlx::query_as(
        "SELECT id, org_id, feature, run_id, model, input_tokens, output_tokens, outcome, error, created_at \
           FROM ai_calls WHERE org_id = $1 ORDER BY created_at DESC LIMIT $2",
    )
    .bind(org_id)
    .bind(limit.clamp(1, 500))
    .fetch_all(conn)
    .await?)
}

/// Makes AI calls for organizations: finds their provider, checks its
/// budget, calls it, and logs the call.
#[derive(Debug, Clone)]
pub struct Ai {
    db: PgPool,
    crypto: Crypto,
    http: reqwest::Client,
}

/// What a successful call used.
#[derive(Debug, Clone, Copy)]
struct Usage<'a> {
    model: &'a str,
    input: i64,
    output: i64,
}

/// What a call is for, for the log.
#[derive(Debug, Clone, Copy)]
pub struct Purpose<'a> {
    pub feature: &'a str,
    pub run_id: Option<Uuid>,
}

impl Ai {
    /// `http` is used for every call (e.g. a client enforcing an egress policy).
    pub const fn new(db: PgPool, crypto: Crypto, http: reqwest::Client) -> Self {
        Self { db, crypto, http }
    }

    async fn log(
        &self,
        org_id: Uuid,
        provider: &ProviderView,
        purpose: Purpose<'_>,
        result: Result<Usage<'_>, &AiError>,
    ) -> Result<(), AiError> {
        let (outcome, error, model, input, output) = match result {
            Ok(usage) => (
                "ok",
                None,
                usage.model.to_owned(),
                usage.input,
                usage.output,
            ),
            Err(e) => {
                let outcome = match e {
                    AiError::Refused(_) => "refused",
                    AiError::OverBudget { .. } => "over_budget",
                    _ => "failed",
                };
                (outcome, Some(e.to_string()), provider.model.clone(), 0, 0)
            }
        };
        let internal = |e: &dyn std::fmt::Display| AiError::Internal(e.to_string());
        let mut tx = begin_scoped(&self.db, &OrgScope::System)
            .await
            .map_err(|e| internal(&e))?;
        sqlx::query(
            "INSERT INTO ai_calls (id, org_id, provider_id, feature, run_id, model, input_tokens, output_tokens, outcome, error) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)",
        )
        .bind(huxley_core::new_id())
        .bind(org_id)
        .bind(provider.id)
        .bind(purpose.feature)
        .bind(purpose.run_id)
        .bind(model)
        .bind(input)
        .bind(output)
        .bind(outcome)
        .bind(error)
        .execute(&mut *tx)
        .await
        .map_err(|e| internal(&e))?;
        tx.commit().await.map_err(|e| internal(&e))?;
        Ok(())
    }

    /// The provider serving `org_id`, and either its endpoint and key or
    /// why it can't be used now (its budget is spent).
    async fn prepare(
        &self,
        org_id: Uuid,
        model: Option<&str>,
    ) -> Result<(ProviderView, Result<(Endpoint, String), AiError>), AiError> {
        let internal = |e: &dyn std::fmt::Display| AiError::Internal(e.to_string());
        let mut tx = begin_scoped(&self.db, &OrgScope::System)
            .await
            .map_err(|e| internal(&e))?;
        let Some(provider) = effective(&mut tx, org_id).await.map_err(|e| internal(&e))? else {
            return Err(AiError::NotConfigured);
        };
        let sealed: Vec<u8> =
            sqlx::query_scalar("SELECT key_sealed FROM ai_providers WHERE id = $1")
                .bind(provider.id)
                .fetch_one(&mut *tx)
                .await
                .map_err(|e| internal(&e))?;
        let used = used_this_month(&mut tx, provider.id)
            .await
            .map_err(|e| internal(&e))?;
        tx.commit().await.map_err(|e| internal(&e))?;
        if let Some(budget) = provider.monthly_token_budget.filter(|b| used >= *b) {
            return Ok((provider, Err(AiError::OverBudget { used, budget })));
        }
        let key = self
            .crypto
            .decrypt(
                Binding {
                    purpose: KEY_PURPOSE,
                    record: provider.id,
                },
                &sealed,
            )
            .await
            .map_err(|e| internal(&e))?;
        let endpoint = Endpoint {
            kind: provider.kind,
            model: model
                .filter(|m| !m.trim().is_empty())
                .unwrap_or(&provider.model)
                .to_owned(),
            base_url: provider.base_url.clone(),
            deployment: provider.deployment.clone(),
        };
        Ok((
            provider,
            Ok((endpoint, String::from_utf8_lossy(&key).into_owned())),
        ))
    }

    /// Calls `org_id`'s provider (its own or the nearest above) with
    /// `request`, optionally with another `model`. Every call is logged.
    pub async fn complete(
        &self,
        org_id: Uuid,
        purpose: Purpose<'_>,
        model: Option<&str>,
        request: &Request,
    ) -> Result<Completion, AiError> {
        let (provider, ready) = self.prepare(org_id, model).await?;
        let result = match ready {
            Ok((endpoint, key)) => provider::complete(&self.http, &endpoint, &key, request).await,
            Err(e) => Err(e),
        };
        let usage = result.as_ref().map(|c| Usage {
            model: &c.model,
            input: c.input_tokens,
            output: c.output_tokens,
        });
        self.log(org_id, &provider, purpose, usage).await?;
        result
    }

    /// The next assistant turn of a conversation with tools, through
    /// `org_id`'s provider. Logged and budgeted like any call. With
    /// `on_text`, the turn is streamed and the text reaches it as it's
    /// written; the turn is the same either way.
    pub async fn turn(
        &self,
        org_id: Uuid,
        purpose: Purpose<'_>,
        system: Option<&str>,
        messages: &[crate::chat::Message],
        tools: &[crate::chat::ToolDef],
        on_text: Option<&(dyn Fn(&str) + Send + Sync)>,
    ) -> Result<crate::chat::Turn, AiError> {
        let (provider, ready) = self.prepare(org_id, None).await?;
        let result = match (ready, on_text) {
            (Ok((endpoint, key)), None) => {
                crate::chat::turn(&self.http, &endpoint, &key, system, messages, tools, 0).await
            }
            (Ok((endpoint, key)), Some(on_text)) => {
                crate::chat::turn_streaming(
                    &self.http, &endpoint, &key, system, messages, tools, 0, on_text,
                )
                .await
            }
            (Err(e), _) => Err(e),
        };
        let usage = result.as_ref().map(|t| Usage {
            model: &t.model,
            input: t.input_tokens,
            output: t.output_tokens,
        });
        self.log(org_id, &provider, purpose, usage).await?;
        result
    }
}
