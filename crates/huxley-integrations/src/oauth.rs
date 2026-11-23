//! OAuth 2.0 state for connections (design doc §7.2): pending sign-ins
//! with their PKCE verifiers, and the tokens a provider issued, encrypted.
//! The HTTP side (token requests through the egress policy) is in the
//! engine; this module never talks to the network.

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use huxley_crypto::{Binding, Crypto};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::PgConnection;
use time::{Duration, OffsetDateTime};
use url::Url;
use uuid::Uuid;
use zeroize::Zeroizing;

use crate::AuthKind;
use crate::connections::{self, ConnectionError, ConnectionResult, ConnectionStatus, invalid};
use crate::store;

/// How long someone has to finish signing in.
const SIGN_IN_WINDOW: Duration = Duration::minutes(10);

/// Tokens a provider issued for a connection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tokens {
    pub access_token: String,
    pub refresh_token: Option<String>,
}

fn random_token() -> String {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).expect("the OS random source works");
    URL_SAFE_NO_PAD.encode(bytes)
}

/// The PKCE S256 challenge for a verifier (RFC 7636).
pub fn challenge(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

/// Starts signing in a connection that uses the authorization code flow:
/// records the attempt and returns the provider URL to send the browser to.
pub async fn begin_sign_in(
    conn: &mut PgConnection,
    connection_id: Uuid,
    user_id: Uuid,
    redirect_uri: &str,
) -> ConnectionResult<String> {
    let view = connections::get(&mut *conn, connection_id).await?;
    let integration = store::get(&mut *conn, view.integration_id).await?;
    let scheme = view.auth_scheme.as_deref().and_then(|k| {
        integration
            .definition
            .auth_schemes
            .iter()
            .find(|s| s.key == k)
    });
    let Some(AuthKind::OAuth2AuthorizationCode {
        authorization_url,
        scopes,
        ..
    }) = scheme.map(|s| &s.kind)
    else {
        return Err(invalid(
            "authScheme",
            "only connections that sign in with OAuth 2.0 can be signed in",
        ));
    };
    let client_id = view.settings["client_id"].as_str().unwrap_or_default();
    let chosen: Vec<String> = view.settings["scopes"].as_array().map_or_else(
        || scopes.clone(),
        |a| {
            a.iter()
                .filter_map(|s| s.as_str())
                .map(str::to_owned)
                .collect()
        },
    );
    let state = random_token();
    let verifier = random_token();
    let authorization_url =
        connections::fill_auth_url(authorization_url, &integration.definition, &view)?;
    let mut url = Url::parse(&authorization_url)
        .map_err(|_| invalid("authScheme", "the spec's authorization URL isn't a URL"))?;
    url.query_pairs_mut()
        .append_pair("response_type", "code")
        .append_pair("client_id", client_id)
        .append_pair("redirect_uri", redirect_uri)
        .append_pair("state", &state)
        .append_pair("code_challenge", &challenge(&verifier))
        .append_pair("code_challenge_method", "S256");
    if !chosen.is_empty() {
        url.query_pairs_mut()
            .append_pair("scope", &chosen.join(" "));
    }
    // Old attempts that were never finished.
    sqlx::query("DELETE FROM connection_sign_ins WHERE expires_at < now()")
        .execute(&mut *conn)
        .await?;
    sqlx::query(
        "INSERT INTO connection_sign_ins (state, connection_id, org_id, user_id, code_verifier, \
                redirect_uri, expires_at) VALUES ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(&state)
    .bind(connection_id)
    .bind(view.org_id)
    .bind(user_id)
    .bind(&verifier)
    .bind(redirect_uri)
    .bind(OffsetDateTime::now_utc() + SIGN_IN_WINDOW)
    .execute(conn)
    .await?;
    Ok(url.into())
}

/// A sign-in to finish: what the token request needs.
#[derive(Debug, Clone)]
pub struct PendingSignIn {
    pub connection_id: Uuid,
    pub code_verifier: String,
    pub redirect_uri: String,
}

/// Takes the sign-in `state` refers to (it can be used once), if it's
/// still valid and `user_id` started it.
pub async fn take_sign_in(
    conn: &mut PgConnection,
    state: &str,
    user_id: Uuid,
) -> ConnectionResult<PendingSignIn> {
    let row: Option<(Uuid, Uuid, String, String, OffsetDateTime)> = sqlx::query_as(
        "DELETE FROM connection_sign_ins WHERE state = $1 \
         RETURNING connection_id, user_id, code_verifier, redirect_uri, expires_at",
    )
    .bind(state)
    .fetch_optional(conn)
    .await?;
    match row {
        Some((connection_id, owner, code_verifier, redirect_uri, expires_at))
            if owner == user_id && expires_at > OffsetDateTime::now_utc() =>
        {
            Ok(PendingSignIn {
                connection_id,
                code_verifier,
                redirect_uri,
            })
        }
        _ => Err(invalid(
            "state",
            "this sign-in expired or isn't yours: start it again",
        )),
    }
}

/// The stored tokens and when the access token expires, if any.
pub async fn tokens(
    conn: &mut PgConnection,
    crypto: &Crypto,
    connection_id: Uuid,
) -> ConnectionResult<Option<(Tokens, Option<OffsetDateTime>)>> {
    let row: Option<(Option<Vec<u8>>, Option<OffsetDateTime>)> =
        sqlx::query_as("SELECT tokens_sealed, token_expires_at FROM connections WHERE id = $1")
            .bind(connection_id)
            .fetch_optional(&mut *conn)
            .await?;
    let (sealed, expires_at) = row.ok_or(ConnectionError::NotFound)?;
    let Some(sealed) = sealed else {
        return Ok(None);
    };
    let binding = Binding {
        purpose: "connection_tokens",
        record: connection_id,
    };
    let plain = Zeroizing::new(crypto.decrypt(binding, &sealed).await?);
    let tokens = serde_json::from_slice(&plain)
        .map_err(|e| ConnectionError::Db(sqlx::Error::Decode(Box::new(e))))?;
    Ok(Some((tokens, expires_at)))
}

/// Stores newly issued tokens and marks the connection ready. Keeps the
/// old refresh token if the provider didn't send a new one.
pub async fn store_tokens(
    conn: &mut PgConnection,
    crypto: &Crypto,
    connection_id: Uuid,
    org_id: Uuid,
    mut tokens: Tokens,
    expires_in: Option<i64>,
) -> ConnectionResult<()> {
    if tokens.refresh_token.is_none()
        && let Some((old, _)) = self::tokens(&mut *conn, crypto, connection_id).await?
    {
        tokens.refresh_token = old.refresh_token;
    }
    let plain = Zeroizing::new(serde_json::to_vec(&tokens).expect("tokens serialize"));
    let binding = Binding {
        purpose: "connection_tokens",
        record: connection_id,
    };
    let sealed = crypto.encrypt(org_id, binding, &plain).await?;
    let expires_at = expires_in.map(|s| OffsetDateTime::now_utc() + Duration::seconds(s));
    sqlx::query(
        "UPDATE connections SET tokens_sealed = $2, token_expires_at = $3, status = 'ready', \
                status_message = NULL WHERE id = $1",
    )
    .bind(connection_id)
    .bind(sealed)
    .bind(expires_at)
    .execute(conn)
    .await?;
    Ok(())
}

/// Records who signed a connection in.
pub async fn mark_authorized(
    conn: &mut PgConnection,
    connection_id: Uuid,
    user_id: Uuid,
) -> ConnectionResult<()> {
    sqlx::query("UPDATE connections SET authorized_by = $2, authorized_at = now() WHERE id = $1")
        .bind(connection_id)
        .bind(user_id)
        .execute(conn)
        .await?;
    Ok(())
}

/// Marks a connection as needing attention (sign in again, or fix it),
/// dropping tokens that no longer work.
pub async fn mark_failed(
    conn: &mut PgConnection,
    connection_id: Uuid,
    status: ConnectionStatus,
    message: &str,
) -> ConnectionResult<()> {
    sqlx::query(
        "UPDATE connections SET status = $2, status_message = $3, tokens_sealed = NULL, \
                token_expires_at = NULL WHERE id = $1",
    )
    .bind(connection_id)
    .bind(status)
    .bind(message)
    .execute(conn)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pkce_challenge_matches_rfc_7636() {
        // Appendix B of RFC 7636.
        assert_eq!(
            challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
        assert_eq!(random_token().len(), 43);
    }
}
