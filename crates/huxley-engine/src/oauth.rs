//! OAuth 2.0 token requests for connections (design doc §7.2), through the
//! same egress policy as HTTP steps: token URLs come from imported specs.
//!
//! A token is reused until a minute before it expires. Fetching or
//! refreshing one holds the connection's row lock, so concurrent workers
//! wait for one request instead of each spending (and, with rotating
//! refresh tokens, invalidating) a refresh.

use std::time::Duration;

use huxley_core::RequestContext;
use huxley_db::audit::{self, AuditRecord};
use huxley_db::{OrgScope, begin_scoped};
use huxley_integrations::connections::{self, ConnectionError, ConnectionStatus, ResolvedAuth};
use huxley_integrations::oauth::{self, Tokens};
use serde_json::{Value as Json, json};
use sqlx::PgConnection;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{Engine, EngineError};

/// Tokens are refreshed this long before they expire.
const EARLY: time::Duration = time::Duration::seconds(60);
const TOKEN_TIMEOUT: Duration = Duration::from_secs(30);

/// Why a connection has no usable token.
#[derive(Debug, thiserror::Error)]
pub enum TokenError {
    /// Someone has to sign the connection in (again).
    #[error("{0}")]
    NeedsSignIn(String),
    /// The provider refused, or couldn't be reached.
    #[error("{0}")]
    Failed(String),
    #[error(transparent)]
    Connection(#[from] ConnectionError),
    #[error(transparent)]
    Engine(#[from] EngineError),
}

impl From<sqlx::Error> for TokenError {
    fn from(e: sqlx::Error) -> Self {
        Self::Engine(e.into())
    }
}

fn usable(expires_at: Option<OffsetDateTime>) -> bool {
    expires_at.is_none_or(|at| at > OffsetDateTime::now_utc() + EARLY)
}

/// A token endpoint's answer: the tokens, and seconds until expiry.
struct Issued {
    tokens: Tokens,
    expires_in: Option<i64>,
}

/// A token endpoint's error, with the OAuth `error` code if it sent one.
struct Refused {
    code: Option<String>,
    message: String,
}

impl Engine {
    /// A usable access token for an OAuth 2.0 connection, fetching or
    /// refreshing one if needed. `None` for connections that don't use
    /// OAuth 2.0.
    pub async fn connection_token(
        &self,
        connection_id: Uuid,
    ) -> Result<Option<String>, TokenError> {
        // Fast path: a stored token that's still good.
        let mut tx = begin_scoped(self.db(), &OrgScope::System)
            .await
            .map_err(EngineError::from)?;
        if let Some((tokens, expires_at)) =
            oauth::tokens(&mut tx, &self.crypto, connection_id).await?
            && usable(expires_at)
        {
            tx.commit().await?;
            return Ok(Some(tokens.access_token));
        }
        // Slow path: one request at a time per connection.
        sqlx::query("SELECT 1 FROM connections WHERE id = $1 FOR UPDATE")
            .bind(connection_id)
            .execute(&mut *tx)
            .await?;
        let token = self.fetch_token(&mut tx, connection_id).await;
        // Failures that mark the connection (sign in again) must stick.
        tx.commit().await?;
        token
    }

    async fn fetch_token(
        &self,
        tx: &mut PgConnection,
        connection_id: Uuid,
    ) -> Result<Option<String>, TokenError> {
        let stored = oauth::tokens(&mut *tx, &self.crypto, connection_id).await?;
        if let Some((tokens, expires_at)) = &stored
            && usable(*expires_at)
        {
            // Another worker got one while we waited for the lock.
            return Ok(Some(tokens.access_token.clone()));
        }
        let resolved = connections::resolve(&mut *tx, &self.crypto, connection_id).await?;
        let view = connections::get(&mut *tx, connection_id).await?;
        let issued = match &resolved.auth {
            ResolvedAuth::OAuth2ClientCredentials {
                token_url,
                client_id,
                client_secret,
                scopes,
            } => {
                let mut form = vec![("grant_type", "client_credentials".to_owned())];
                if !scopes.is_empty() {
                    form.push(("scope", scopes.join(" ")));
                }
                self.token_request(token_url, client_id, client_secret, form)
                    .await
                    .map_err(|r| TokenError::Failed(r.message))?
            }
            ResolvedAuth::OAuth2AuthorizationCode {
                token_url,
                client_id,
                client_secret,
            } => {
                let Some(refresh) = stored.and_then(|(t, _)| t.refresh_token) else {
                    let message = "the sign-in expired: sign in again";
                    oauth::mark_failed(tx, connection_id, ConnectionStatus::NeedsAuth, message)
                        .await?;
                    return Err(TokenError::NeedsSignIn(message.into()));
                };
                let form = vec![
                    ("grant_type", "refresh_token".to_owned()),
                    ("refresh_token", refresh),
                ];
                match self
                    .token_request(token_url, client_id, client_secret, form)
                    .await
                {
                    Ok(issued) => issued,
                    // The refresh token was revoked or expired.
                    Err(Refused {
                        code: Some(code),
                        message,
                    }) if code == "invalid_grant" => {
                        let message =
                            format!("the provider ended the sign-in ({message}): sign in again");
                        oauth::mark_failed(
                            tx,
                            connection_id,
                            ConnectionStatus::NeedsAuth,
                            &message,
                        )
                        .await?;
                        return Err(TokenError::NeedsSignIn(message));
                    }
                    Err(refused) => return Err(TokenError::Failed(refused.message)),
                }
            }
            _ => return Ok(None),
        };
        let token = issued.tokens.access_token.clone();
        oauth::store_tokens(
            tx,
            &self.crypto,
            connection_id,
            view.org_id,
            issued.tokens,
            issued.expires_in,
        )
        .await?;
        Ok(Some(token))
    }

    /// Finishes a sign-in: exchanges the provider's code (with the PKCE
    /// verifier) for tokens. `scope` is the signed-in user's.
    pub async fn complete_sign_in(
        &self,
        ctx: &RequestContext,
        scope: &OrgScope,
        user_id: Uuid,
        state: &str,
        code: &str,
    ) -> Result<connections::ConnectionView, TokenError> {
        let mut tx = begin_scoped(self.db(), scope)
            .await
            .map_err(EngineError::from)?;
        let pending = oauth::take_sign_in(&mut tx, state, user_id).await?;
        // The pending row is gone whatever happens next: commit that.
        tx.commit().await?;

        let mut tx = begin_scoped(self.db(), &OrgScope::System)
            .await
            .map_err(EngineError::from)?;
        let id = pending.connection_id;
        let resolved = connections::resolve(&mut tx, &self.crypto, id).await?;
        let ResolvedAuth::OAuth2AuthorizationCode {
            token_url,
            client_id,
            client_secret,
        } = &resolved.auth
        else {
            return Err(TokenError::Failed(
                "the connection no longer signs in with OAuth 2.0".into(),
            ));
        };
        let form = vec![
            ("grant_type", "authorization_code".to_owned()),
            ("code", code.to_owned()),
            ("redirect_uri", pending.redirect_uri.clone()),
            ("code_verifier", pending.code_verifier.clone()),
        ];
        let issued = self
            .token_request(token_url, client_id, client_secret, form)
            .await
            .map_err(|r| TokenError::Failed(r.message))?;
        let view = connections::get(&mut tx, id).await?;
        oauth::store_tokens(
            &mut tx,
            &self.crypto,
            id,
            view.org_id,
            issued.tokens,
            issued.expires_in,
        )
        .await?;
        oauth::mark_authorized(&mut tx, id, user_id).await?;
        audit::record(
            &mut tx,
            ctx,
            AuditRecord::new("connection.signed_in")
                .org(view.org_id)
                .target("connection", id),
        )
        .await
        .map_err(EngineError::from)?;
        let view = connections::get(&mut tx, id).await?;
        tx.commit().await?;
        Ok(view)
    }

    /// POSTs a form to a token endpoint (client credentials in the body,
    /// RFC 6749 §2.3.1) and reads its JSON answer.
    async fn token_request(
        &self,
        token_url: &str,
        client_id: &str,
        client_secret: &str,
        mut form: Vec<(&'static str, String)>,
    ) -> Result<Issued, Refused> {
        let refused = |message: String| Refused {
            code: None,
            message,
        };
        let url = url::Url::parse(token_url)
            .map_err(|_| refused(format!("the token URL {token_url:?} isn't a URL")))?;
        self.egress
            .policy
            .check_url(&url)
            .map_err(|b| refused(format!("the token URL is blocked: {}", b.0)))?;
        form.push(("client_id", client_id.to_owned()));
        form.push(("client_secret", client_secret.to_owned()));
        let body = url::form_urlencoded::Serializer::new(String::new())
            .extend_pairs(form.iter().map(|(k, v)| (*k, v.as_str())))
            .finish();
        let response = self
            .egress
            .client
            .post(url)
            .header("accept", "application/json")
            .header("content-type", "application/x-www-form-urlencoded")
            .body(body)
            .timeout(TOKEN_TIMEOUT)
            .send()
            .await
            .map_err(|e| {
                refused(crate::egress::blocked_reason(&e).map_or_else(
                    || format!("the token endpoint couldn't be reached: {e}"),
                    |b| format!("the token URL is blocked: {b}"),
                ))
            })?;
        let status = response.status();
        let body: Json = response.json().await.unwrap_or(Json::Null);
        if !status.is_success() {
            let code = body["error"].as_str().map(str::to_owned);
            let detail = body["error_description"]
                .as_str()
                .or(code.as_deref())
                .unwrap_or("no details")
                .to_owned();
            return Err(Refused {
                code,
                message: format!("the token endpoint answered {status}: {detail}"),
            });
        }
        let Some(access_token) = body["access_token"].as_str() else {
            return Err(refused("the token endpoint sent no access_token".into()));
        };
        let expires_in = match &body["expires_in"] {
            Json::Number(n) => n.as_i64(),
            Json::String(s) => s.parse().ok(),
            _ => None,
        };
        Ok(Issued {
            tokens: Tokens {
                access_token: access_token.to_owned(),
                refresh_token: body["refresh_token"].as_str().map(str::to_owned),
            },
            expires_in,
        })
    }
}

/// What to show for a failed token request, for step errors.
pub fn token_error(e: &TokenError) -> Json {
    match e {
        TokenError::NeedsSignIn(m) | TokenError::Failed(m) => {
            json!({ "kind": "auth", "message": m })
        }
        other => json!({ "kind": "internal", "message": other.to_string() }),
    }
}
