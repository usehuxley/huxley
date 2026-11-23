//! Connections (design doc §7.2): one of an integration's auth schemes with
//! its credentials, owned by an org and optionally shared with the orgs
//! below. Secret fields are encrypted with the org's data key and never
//! returned; [`resolve`] decrypts them for the code that calls the API.

use std::collections::BTreeMap;

use huxley_core::RequestContext;
use huxley_crypto::{Binding, Crypto, CryptoError};
use huxley_db::audit::{self, AuditRecord};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value as Json, json};
use sqlx::PgConnection;
use time::OffsetDateTime;
use utoipa::ToSchema;
use uuid::Uuid;
use zeroize::Zeroizing;

use crate::store::{self, StoreError};
use crate::{AuthKind, AuthScheme, Definition, ParamLocation};

#[derive(Debug, thiserror::Error)]
pub enum ConnectionError {
    #[error("connection not found")]
    NotFound,
    #[error("another connection in this organization already has the key {0:?}")]
    KeyTaken(String),
    /// A field is missing or wrong: `(field, message)`.
    #[error("{1}")]
    Invalid(String, String),
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error(transparent)]
    Crypto(#[from] CryptoError),
    #[error(transparent)]
    Db(#[from] sqlx::Error),
}

pub type ConnectionResult<T> = Result<T, ConnectionError>;

pub(crate) fn invalid(field: &str, message: impl Into<String>) -> ConnectionError {
    ConnectionError::Invalid(field.to_owned(), message.into())
}

fn db(e: huxley_db::DbError) -> ConnectionError {
    match e {
        huxley_db::DbError::Sqlx(e) => ConnectionError::Db(e),
        other => ConnectionError::Db(sqlx::Error::Protocol(other.to_string())),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema, sqlx::Type)]
#[sqlx(type_name = "connection_status", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum ConnectionStatus {
    Ready,
    /// Waiting for someone to sign in (OAuth 2.0 authorization code).
    NeedsAuth,
    Error,
}

/// The fields a connection needs for an auth kind: plain settings, and
/// secrets that are encrypted and never shown again.
#[derive(Debug)]
pub struct Fields {
    pub settings: &'static [&'static str],
    pub secrets: &'static [&'static str],
}

/// What a connection using `kind` must be given.
pub const fn fields(kind: &AuthKind) -> Fields {
    match kind {
        AuthKind::ApiKey { .. } => Fields {
            settings: &[],
            secrets: &["key"],
        },
        AuthKind::Basic => Fields {
            settings: &["username"],
            secrets: &["password"],
        },
        AuthKind::Bearer { .. } => Fields {
            settings: &[],
            secrets: &["token"],
        },
        AuthKind::OAuth2ClientCredentials { .. }
        | AuthKind::OAuth2AuthorizationCode { .. }
        | AuthKind::OpenIdConnect { .. } => Fields {
            settings: &["client_id", "scopes"],
            secrets: &["client_secret"],
        },
    }
}

/// A connection as shown: settings, and which secrets are set.
#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionView {
    pub id: Uuid,
    pub org_id: Uuid,
    pub integration_id: Uuid,
    pub key: String,
    pub name: String,
    pub auth_scheme: Option<String>,
    pub base_url: Option<String>,
    #[schema(value_type = std::collections::HashMap<String, String>)]
    pub server_variables: Json,
    #[schema(value_type = std::collections::HashMap<String, serde_json::Value>)]
    pub settings: Json,
    /// Names of the secret fields that have a value.
    pub secret_fields: Vec<String>,
    /// Usable by the organizations below this one.
    pub shared: bool,
    pub status: ConnectionStatus,
    pub status_message: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

const VIEW: &str = "id, org_id, integration_id, key, name, auth_scheme, base_url, \
                    server_variables, settings, secret_fields, shared, status, status_message, \
                    created_at, updated_at";

/// What a connection is made from (or changed to).
#[derive(Debug, Clone, Default, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConnectionInput {
    pub name: String,
    /// Defaults to one made from the name.
    #[serde(default)]
    pub key: Option<String>,
    /// One of the integration's auth scheme keys; none for open APIs.
    #[serde(default)]
    pub auth_scheme: Option<String>,
    #[serde(default)]
    pub base_url: Option<String>,
    #[serde(default)]
    #[schema(value_type = std::collections::HashMap<String, String>)]
    pub server_variables: Map<String, Json>,
    #[serde(default)]
    #[schema(value_type = std::collections::HashMap<String, serde_json::Value>)]
    pub settings: Map<String, Json>,
    /// Secret values. On update, fields left out keep their value.
    #[serde(default)]
    #[schema(value_type = std::collections::HashMap<String, String>)]
    pub secrets: BTreeMap<String, String>,
    #[serde(default)]
    pub shared: bool,
}

fn purpose() -> &'static str {
    "connection_secrets"
}

fn scheme<'a>(definition: &'a Definition, key: &str) -> ConnectionResult<&'a AuthScheme> {
    definition
        .auth_schemes
        .iter()
        .find(|s| s.key == key)
        .ok_or_else(|| {
            invalid(
                "authScheme",
                format!("the integration has no auth scheme {key:?}"),
            )
        })
}

/// Checks the input against the scheme: settings and secrets it knows
/// about, every secret present (counting ones already stored).
fn check(
    definition: &Definition,
    input: &ConnectionInput,
    stored_secrets: &[String],
) -> ConnectionResult<()> {
    if input.name.trim().is_empty() || input.name.chars().count() > 200 {
        return Err(invalid("name", "a name of 1 to 200 characters"));
    }
    if let Some(url) = &input.base_url {
        let parsed = url::Url::parse(url).map_err(|_| invalid("baseUrl", "not a URL"))?;
        if !matches!(parsed.scheme(), "https" | "http") {
            return Err(invalid("baseUrl", "an http or https URL"));
        }
    } else if definition.servers.is_empty() {
        return Err(invalid(
            "baseUrl",
            "the integration's spec has no servers: give a base URL",
        ));
    }
    let Some(key) = &input.auth_scheme else {
        if !input.secrets.is_empty() || !input.settings.is_empty() {
            return Err(invalid(
                "authScheme",
                "choose an auth scheme for these credentials",
            ));
        }
        return Ok(());
    };
    let wanted = fields(&scheme(definition, key)?.kind);
    for name in input.settings.keys() {
        if !wanted.settings.contains(&name.as_str()) {
            return Err(invalid(
                &format!("settings.{name}"),
                "not used by this auth scheme",
            ));
        }
    }
    for name in input.secrets.keys() {
        if !wanted.secrets.contains(&name.as_str()) {
            return Err(invalid(
                &format!("secrets.{name}"),
                "not used by this auth scheme",
            ));
        }
    }
    for name in wanted.secrets {
        let given = input.secrets.get(*name).is_some_and(|v| !v.is_empty());
        if !given && !stored_secrets.iter().any(|s| s == name) {
            return Err(invalid(&format!("secrets.{name}"), "required"));
        }
    }
    if wanted.settings.contains(&"username")
        && input
            .settings
            .get("username")
            .and_then(Json::as_str)
            .is_none_or(str::is_empty)
    {
        return Err(invalid("settings.username", "required"));
    }
    if wanted.settings.contains(&"client_id")
        && input
            .settings
            .get("client_id")
            .and_then(Json::as_str)
            .is_none_or(str::is_empty)
    {
        return Err(invalid("settings.client_id", "required"));
    }
    Ok(())
}

async fn seal(
    crypto: &Crypto,
    org_id: Uuid,
    id: Uuid,
    secrets: &BTreeMap<String, String>,
) -> ConnectionResult<Option<Vec<u8>>> {
    if secrets.is_empty() {
        return Ok(None);
    }
    let plain = Zeroizing::new(serde_json::to_vec(secrets).expect("strings serialize"));
    let binding = Binding {
        purpose: purpose(),
        record: id,
    };
    Ok(Some(crypto.encrypt(org_id, binding, &plain).await?))
}

async fn open(
    crypto: &Crypto,
    id: Uuid,
    sealed: &[u8],
) -> ConnectionResult<BTreeMap<String, String>> {
    let binding = Binding {
        purpose: purpose(),
        record: id,
    };
    let plain = Zeroizing::new(crypto.decrypt(binding, sealed).await?);
    serde_json::from_slice(&plain)
        .map_err(|e| ConnectionError::Db(sqlx::Error::Decode(Box::new(e))))
}

fn initial_status(definition: &Definition, scheme_key: Option<&str>) -> ConnectionStatus {
    let needs_sign_in = scheme_key
        .and_then(|k| definition.auth_schemes.iter().find(|s| s.key == k))
        .is_some_and(|s| {
            matches!(
                s.kind,
                AuthKind::OAuth2AuthorizationCode { .. } | AuthKind::OpenIdConnect { .. }
            )
        });
    if needs_sign_in {
        ConnectionStatus::NeedsAuth
    } else {
        ConnectionStatus::Ready
    }
}

/// Creates a connection to `integration_id` in `org_id`. Encrypts before
/// auditing: the first encryption for an org writes its data key (and an
/// audit record) in a transaction of its own.
pub async fn create(
    conn: &mut PgConnection,
    crypto: &Crypto,
    ctx: &RequestContext,
    org_id: Uuid,
    integration_id: Uuid,
    input: &ConnectionInput,
) -> ConnectionResult<ConnectionView> {
    let integration = store::get(&mut *conn, integration_id).await?;
    check(&integration.definition, input, &[])?;
    let key = input
        .key
        .clone()
        .unwrap_or_else(|| store::key_from(&input.name));
    if !store::valid_key(&key) {
        return Err(invalid(
            "key",
            "lowercase letters, digits and hyphens, starting with a letter",
        ));
    }
    let id = huxley_core::new_id();
    let sealed = seal(crypto, org_id, id, &input.secrets).await?;
    let secret_fields: Vec<&String> = input.secrets.keys().collect();
    let inserted = sqlx::query(
        "INSERT INTO connections (id, org_id, integration_id, key, name, auth_scheme, base_url, \
                server_variables, settings, secrets_sealed, secret_fields, shared, status, \
                created_by, updated_by) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $14) \
         ON CONFLICT (org_id, key) DO NOTHING",
    )
    .bind(id)
    .bind(org_id)
    .bind(integration_id)
    .bind(&key)
    .bind(input.name.trim())
    .bind(&input.auth_scheme)
    .bind(&input.base_url)
    .bind(Json::Object(input.server_variables.clone()))
    .bind(Json::Object(input.settings.clone()))
    .bind(sealed)
    .bind(secret_fields)
    .bind(input.shared)
    .bind(initial_status(
        &integration.definition,
        input.auth_scheme.as_deref(),
    ))
    .bind(ctx.actor.id())
    .execute(&mut *conn)
    .await?;
    if inserted.rows_affected() == 0 {
        return Err(ConnectionError::KeyTaken(key));
    }
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new("connection.created")
            .org(org_id)
            .target("connection", id)
            .details(json!({
                "integration": integration_id,
                "key": key,
                "authScheme": input.auth_scheme,
                "shared": input.shared,
            })),
    )
    .await
    .map_err(db)?;
    get(conn, id).await
}

/// Changes a connection. Secret fields left out of the input keep their
/// stored value; changing the auth scheme drops them all.
pub async fn update(
    conn: &mut PgConnection,
    crypto: &Crypto,
    ctx: &RequestContext,
    id: Uuid,
    input: &ConnectionInput,
) -> ConnectionResult<ConnectionView> {
    let current = get(&mut *conn, id).await?;
    let integration = store::get(&mut *conn, current.integration_id).await?;
    let same_scheme = current.auth_scheme == input.auth_scheme;
    let kept: Vec<String> = if same_scheme {
        current.secret_fields.clone()
    } else {
        Vec::new()
    };
    check(&integration.definition, input, &kept)?;
    let mut secrets = BTreeMap::new();
    if same_scheme && !current.secret_fields.is_empty() {
        let (sealed,): (Option<Vec<u8>>,) =
            sqlx::query_as("SELECT secrets_sealed FROM connections WHERE id = $1")
                .bind(id)
                .fetch_one(&mut *conn)
                .await?;
        if let Some(sealed) = sealed {
            secrets = open(crypto, id, &sealed).await?;
        }
    }
    for (name, value) in &input.secrets {
        if !value.is_empty() {
            secrets.insert(name.clone(), value.clone());
        }
    }
    let sealed = seal(crypto, current.org_id, id, &secrets).await?;
    let secret_fields: Vec<&String> = secrets.keys().collect();
    let status = if same_scheme {
        current.status
    } else {
        initial_status(&integration.definition, input.auth_scheme.as_deref())
    };
    sqlx::query(
        "UPDATE connections SET name = $2, auth_scheme = $3, base_url = $4, \
                server_variables = $5, settings = $6, secrets_sealed = $7, secret_fields = $8, \
                shared = $9, status = $10, updated_by = $11, updated_at = now() \
          WHERE id = $1",
    )
    .bind(id)
    .bind(input.name.trim())
    .bind(&input.auth_scheme)
    .bind(&input.base_url)
    .bind(Json::Object(input.server_variables.clone()))
    .bind(Json::Object(input.settings.clone()))
    .bind(sealed)
    .bind(secret_fields)
    .bind(input.shared)
    .bind(status)
    .bind(ctx.actor.id())
    .execute(&mut *conn)
    .await?;
    let changed: Vec<&String> = input
        .secrets
        .iter()
        .filter(|(_, v)| !v.is_empty())
        .map(|(k, _)| k)
        .collect();
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new("connection.updated")
            .org(current.org_id)
            .target("connection", id)
            .details(json!({
                "authScheme": input.auth_scheme,
                "shared": input.shared,
                // Which secrets were replaced, never their values.
                "secretsChanged": changed,
            })),
    )
    .await
    .map_err(db)?;
    get(conn, id).await
}

pub async fn delete(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    id: Uuid,
) -> ConnectionResult<()> {
    let row: Option<(Uuid, String)> =
        sqlx::query_as("DELETE FROM connections WHERE id = $1 RETURNING org_id, key")
            .bind(id)
            .fetch_optional(&mut *conn)
            .await?;
    let (org_id, key) = row.ok_or(ConnectionError::NotFound)?;
    audit::record(
        conn,
        ctx,
        AuditRecord::new("connection.deleted")
            .org(org_id)
            .target("connection", id)
            .details(json!({ "key": key })),
    )
    .await
    .map_err(db)?;
    Ok(())
}

pub async fn get(conn: &mut PgConnection, id: Uuid) -> ConnectionResult<ConnectionView> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {VIEW} FROM connections WHERE id = $1"
    )))
    .bind(id)
    .fetch_optional(conn)
    .await?
    .ok_or(ConnectionError::NotFound)
}

/// Connections in the caller's scope, optionally for one integration.
pub async fn list(
    conn: &mut PgConnection,
    integration_id: Option<Uuid>,
) -> ConnectionResult<Vec<ConnectionView>> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {VIEW} FROM connections \
          WHERE ($1::uuid IS NULL OR integration_id = $1) ORDER BY lower(name), id"
    )))
    .bind(integration_id)
    .fetch_all(conn)
    .await?)
}

/// How to authenticate a request, with secrets decrypted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolvedAuth {
    None,
    ApiKey {
        location: ParamLocation,
        name: String,
        value: String,
    },
    Basic {
        username: String,
        password: String,
    },
    Bearer {
        token: String,
    },
    OAuth2ClientCredentials {
        token_url: String,
        client_id: String,
        client_secret: String,
        scopes: Vec<String>,
    },
    /// Needs a token from someone signing in (sign-in and refresh come
    /// with the OAuth 2.0 flows).
    OAuth2AuthorizationCode {
        token_url: String,
        client_id: String,
        client_secret: String,
    },
}

/// What calling an action through a connection needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    pub connection_id: Uuid,
    pub base_url: String,
    pub auth: ResolvedAuth,
}

/// The base URL: the connection's own, or the integration's first server
/// with its `{variables}` filled from the connection (or their defaults).
pub fn base_url(definition: &Definition, view: &ConnectionView) -> Option<String> {
    if let Some(url) = &view.base_url {
        return Some(url.trim_end_matches('/').to_owned());
    }
    let server = definition.servers.first()?;
    let mut url = server.url.clone();
    for variable in &server.variables {
        let value = view
            .server_variables
            .get(&variable.name)
            .and_then(Json::as_str)
            .unwrap_or(&variable.default);
        url = url.replace(&format!("{{{}}}", variable.name), value);
    }
    Some(url.trim_end_matches('/').to_owned())
}

/// Decrypts a connection for calling its integration. Runs as the system:
/// the caller has already decided the run may use it.
pub async fn resolve(
    conn: &mut PgConnection,
    crypto: &Crypto,
    id: Uuid,
) -> ConnectionResult<Resolved> {
    let view = get(&mut *conn, id).await?;
    let integration = store::get(&mut *conn, view.integration_id).await?;
    let definition = &integration.definition;
    let base_url = base_url(definition, &view)
        .ok_or_else(|| invalid("baseUrl", "the connection has no base URL"))?;
    let (sealed,): (Option<Vec<u8>>,) =
        sqlx::query_as("SELECT secrets_sealed FROM connections WHERE id = $1")
            .bind(id)
            .fetch_one(&mut *conn)
            .await?;
    let secrets = match sealed {
        Some(sealed) => open(crypto, id, &sealed).await?,
        None => BTreeMap::new(),
    };
    let secret = |name: &str| secrets.get(name).cloned().unwrap_or_default();
    let setting = |name: &str| {
        view.settings
            .get(name)
            .and_then(Json::as_str)
            .unwrap_or_default()
            .to_owned()
    };
    let auth = match &view.auth_scheme {
        None => ResolvedAuth::None,
        Some(key) => match &scheme(definition, key)?.kind {
            AuthKind::ApiKey { location, name } => ResolvedAuth::ApiKey {
                location: *location,
                name: name.clone(),
                value: secret("key"),
            },
            AuthKind::Basic => ResolvedAuth::Basic {
                username: setting("username"),
                password: secret("password"),
            },
            AuthKind::Bearer { .. } => ResolvedAuth::Bearer {
                token: secret("token"),
            },
            AuthKind::OAuth2ClientCredentials { token_url, scopes } => {
                let chosen: Vec<String> = view.settings["scopes"].as_array().map_or_else(
                    || scopes.clone(),
                    |a| {
                        a.iter()
                            .filter_map(Json::as_str)
                            .map(str::to_owned)
                            .collect()
                    },
                );
                ResolvedAuth::OAuth2ClientCredentials {
                    token_url: token_url.clone(),
                    client_id: setting("client_id"),
                    client_secret: secret("client_secret"),
                    scopes: chosen,
                }
            }
            AuthKind::OAuth2AuthorizationCode { token_url, .. } => {
                ResolvedAuth::OAuth2AuthorizationCode {
                    token_url: token_url.clone(),
                    client_id: setting("client_id"),
                    client_secret: secret("client_secret"),
                }
            }
            AuthKind::OpenIdConnect { .. } => {
                return Err(invalid(
                    "authScheme",
                    "OpenID Connect connections aren't supported yet",
                ));
            }
        },
    };
    Ok(Resolved {
        connection_id: id,
        base_url,
        auth,
    })
}
