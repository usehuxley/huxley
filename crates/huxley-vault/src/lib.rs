//! The `vault` module (design doc §8.5): client passwords, API keys, Wi-Fi
//! keys and secret notes, kept per organization.
//!
//! - Each secret is sealed with the organization's data key (envelope
//!   encryption, purpose `vault_item_secret`, bound to the item's id) and is
//!   never listed or returned except by [`reveal`].
//! - Every reveal is kept in `vault_reveals` (surviving the item's
//!   deletion), audited, and raised as `vault.secret_revealed`, without the
//!   secret.
//! - Records are organization-scoped (RLS); the API checks `vault:*`
//!   permissions and that the module is on.

use huxley_core::RequestContext;
use huxley_crypto::{Binding, Crypto};
use huxley_db::audit::{self, AuditRecord};
use huxley_db::outbox::{self, DomainEvent};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::PgConnection;
use time::OffsetDateTime;
use utoipa::ToSchema;
use uuid::Uuid;
use zeroize::Zeroizing;

/// The module's key in the registry.
pub const MODULE: &str = "vault";

/// What an item holds.
pub const KINDS: &[&str] = &["login", "api_key", "wifi", "note", "other"];

const PURPOSE: &str = "vault_item_secret";
/// The longest secret kept.
const MAX_SECRET: usize = 10_000;

#[derive(Debug, thiserror::Error)]
pub enum VaultError {
    #[error("not found")]
    NotFound,
    #[error("{field}: {message}")]
    Invalid { field: String, message: String },
    #[error("it was changed by someone else; reload and try again")]
    VersionConflict,
    #[error("this item has no secret")]
    NoSecret,
    #[error("{0}")]
    Crypto(String),
    #[error(transparent)]
    Db(#[from] huxley_db::DbError),
    #[error(transparent)]
    Sqlx(#[from] sqlx::Error),
}

impl VaultError {
    fn invalid(field: &str, message: impl Into<String>) -> Self {
        Self::Invalid {
            field: field.to_owned(),
            message: message.into(),
        }
    }
}

pub type VaultResult<T> = Result<T, VaultError>;

/// Trims text; empty becomes `None`. Rejects text over `max` characters and
/// control characters (other than line breaks and tabs).
fn text(field: &str, value: Option<&str>, max: usize) -> VaultResult<Option<String>> {
    let Some(value) = value.map(str::trim).filter(|v| !v.is_empty()) else {
        return Ok(None);
    };
    if value.chars().count() > max {
        return Err(VaultError::invalid(
            field,
            format!("at most {max} characters"),
        ));
    }
    if value
        .chars()
        .any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t'))
    {
        return Err(VaultError::invalid(field, "contains control characters"));
    }
    Ok(Some(value.to_owned()))
}

/// An item as shown: never with its secret.
#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
#[schema(as = VaultItem)]
pub struct Item {
    pub id: Uuid,
    pub org_id: Uuid,
    pub org_name: String,
    pub name: String,
    pub kind: String,
    pub username: String,
    pub url: String,
    pub notes: String,
    pub tags: Vec<String>,
    /// Whether it holds a secret to reveal.
    pub has_secret: bool,
    #[serde(with = "time::serde::rfc3339::option")]
    pub secret_changed_at: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub last_revealed_at: Option<OffsetDateTime>,
    pub updated_by_name: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
    /// For optimistic locking: send it back when changing the item.
    pub version: i32,
}

const SELECT: &str = "SELECT v.id, v.org_id, o.name AS org_name, v.name, v.kind, v.username, v.url, v.notes, \
    v.tags, v.secret_sealed IS NOT NULL AS has_secret, v.secret_changed_at, \
    (SELECT max(r.revealed_at) FROM vault_reveals r WHERE r.item_id = v.id) AS last_revealed_at, \
    u.display_name AS updated_by_name, v.updated_at, v.version \
    FROM vault_items v JOIN organizations o ON o.id = v.org_id LEFT JOIN users u ON u.id = v.updated_by";

/// A new item.
#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schema(as = VaultItemInput)]
pub struct ItemInput {
    pub org_id: Uuid,
    pub name: String,
    /// `login` (the default), `api_key`, `wifi`, `note` or `other`.
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
    /// Not secret: how to use it, who owns it.
    #[serde(default)]
    pub notes: Option<String>,
    #[serde(default)]
    pub tags: Option<Vec<String>>,
    /// The password, key or secret note. Stored encrypted.
    #[serde(default)]
    pub secret: Option<String>,
}

/// Changes to an item: what's given changes.
#[derive(Debug, Clone, Default, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schema(as = VaultItemChanges)]
pub struct ItemChanges {
    /// The version you read.
    pub version: i32,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub notes: Option<String>,
    #[serde(default)]
    pub tags: Option<Vec<String>>,
    /// A new secret (leave out to keep the current one).
    #[serde(default)]
    pub secret: Option<String>,
    /// Remove the secret.
    #[serde(default)]
    pub clear_secret: bool,
}

/// What to list.
#[derive(Debug, Clone, Default)]
pub struct Filter {
    /// Words in the name, username, URL or tags.
    pub q: Option<String>,
    /// Only this organization's items.
    pub org_id: Option<Uuid>,
    pub limit: i64,
}

/// Items the transaction's scope can see, by name.
pub async fn list(conn: &mut PgConnection, filter: &Filter) -> VaultResult<Vec<Item>> {
    let q = filter.q.as_deref().map(str::trim).filter(|q| !q.is_empty());
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{SELECT} WHERE ($1::uuid IS NULL OR v.org_id = $1) \
            AND ($2::text IS NULL OR (v.name || ' ' || v.username || ' ' || v.url || ' ' || kb_tags_text(v.tags)) \
                 ILIKE '%' || $2 || '%' OR v.name % $2) \
          ORDER BY lower(v.name), v.id LIMIT $3"
    )))
    .bind(filter.org_id)
    .bind(q)
    .bind(filter.limit.clamp(1, 500))
    .fetch_all(conn)
    .await?)
}

pub async fn get(conn: &mut PgConnection, id: Uuid) -> VaultResult<Item> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!("{SELECT} WHERE v.id = $1")))
        .bind(id)
        .fetch_optional(conn)
        .await?
        .ok_or(VaultError::NotFound)
}

fn kind(value: Option<&str>) -> VaultResult<Option<String>> {
    match value {
        None => Ok(None),
        Some(k) if KINDS.contains(&k) => Ok(Some(k.to_owned())),
        Some(_) => Err(VaultError::invalid(
            "kind",
            "login, api_key, wifi, note or other",
        )),
    }
}

fn tags(value: Option<&[String]>) -> VaultResult<Option<Vec<String>>> {
    let Some(value) = value else { return Ok(None) };
    let mut out: Vec<String> = Vec::new();
    for tag in value {
        let Some(tag) = text("tags", Some(tag), 50)? else {
            continue;
        };
        let tag = tag.to_lowercase();
        if !out.contains(&tag) {
            out.push(tag);
        }
    }
    if out.len() > 20 {
        return Err(VaultError::invalid("tags", "at most 20 tags"));
    }
    Ok(Some(out))
}

/// Seals a secret for an item. Before anything is audited in the caller's
/// transaction: an organization's first data key is audited on its own.
async fn seal(crypto: &Crypto, org_id: Uuid, id: Uuid, secret: &str) -> VaultResult<Vec<u8>> {
    if secret.chars().count() > MAX_SECRET {
        return Err(VaultError::invalid(
            "secret",
            format!("at most {MAX_SECRET} characters"),
        ));
    }
    crypto
        .encrypt(
            org_id,
            Binding {
                purpose: PURPOSE,
                record: id,
            },
            secret.as_bytes(),
        )
        .await
        .map_err(|e| VaultError::Crypto(e.to_string()))
}

/// The secret as given: only an empty one is "none" (spaces may matter).
fn secret(value: Option<&str>) -> Option<&str> {
    value.filter(|s| !s.is_empty())
}

/// Adds an item.
pub async fn create(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    crypto: &Crypto,
    input: &ItemInput,
) -> VaultResult<Item> {
    let name = text("name", Some(&input.name), 200)?
        .ok_or_else(|| VaultError::invalid("name", "an item needs a name"))?;
    let kind = kind(input.kind.as_deref())?.unwrap_or_else(|| "login".into());
    let username = text("username", input.username.as_deref(), 500)?.unwrap_or_default();
    let url = text("url", input.url.as_deref(), 2000)?.unwrap_or_default();
    let notes = text("notes", input.notes.as_deref(), 10_000)?.unwrap_or_default();
    let tags = tags(input.tags.as_deref())?.unwrap_or_default();
    let id = huxley_core::new_id();
    let sealed = match secret(input.secret.as_deref()) {
        Some(s) => Some(seal(crypto, input.org_id, id, s).await?),
        None => None,
    };
    sqlx::query(
        "INSERT INTO vault_items (id, org_id, name, kind, username, url, notes, tags, secret_sealed, \
                secret_changed_at, created_by, updated_by) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, CASE WHEN $9 IS NULL THEN NULL ELSE now() END, $10, $10)",
    )
    .bind(id)
    .bind(input.org_id)
    .bind(&name)
    .bind(&kind)
    .bind(&username)
    .bind(&url)
    .bind(&notes)
    .bind(&tags)
    .bind(sealed.as_deref())
    .bind(ctx.actor.id())
    .execute(&mut *conn)
    .await?;
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new("vault.item_created")
            .org(input.org_id)
            .target("vault_item", id)
            .details(json!({ "name": name, "kind": kind, "hasSecret": sealed.is_some() })),
    )
    .await?;
    get(conn, id).await
}

/// Changes an item; a new secret replaces the old one.
pub async fn update(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    crypto: &Crypto,
    id: Uuid,
    changes: &ItemChanges,
) -> VaultResult<Item> {
    let before = get(conn, id).await?;
    if before.version != changes.version {
        return Err(VaultError::VersionConflict);
    }
    let name = match &changes.name {
        Some(n) => text("name", Some(n), 200)?
            .ok_or_else(|| VaultError::invalid("name", "an item needs a name"))?,
        None => before.name.clone(),
    };
    let kind = kind(changes.kind.as_deref())?.unwrap_or_else(|| before.kind.clone());
    let field =
        |name: &str, value: &Option<String>, old: &str, max: usize| -> VaultResult<String> {
            match value {
                Some(v) => Ok(text(name, Some(v), max)?.unwrap_or_default()),
                None => Ok(old.to_owned()),
            }
        };
    let username = field("username", &changes.username, &before.username, 500)?;
    let url = field("url", &changes.url, &before.url, 2000)?;
    let notes = field("notes", &changes.notes, &before.notes, 10_000)?;
    let tags = tags(changes.tags.as_deref())?.unwrap_or_else(|| before.tags.clone());
    let new_secret = match secret(changes.secret.as_deref()) {
        Some(s) => Some(seal(crypto, before.org_id, id, s).await?),
        None => None,
    };
    let secret_change = if new_secret.is_some() {
        "set"
    } else if changes.clear_secret && before.has_secret {
        "cleared"
    } else {
        "kept"
    };
    let rows = sqlx::query(
        "UPDATE vault_items SET name = $3, kind = $4, username = $5, url = $6, notes = $7, tags = $8, \
                secret_sealed = CASE WHEN $9::bytea IS NOT NULL THEN $9 WHEN $10 THEN NULL ELSE secret_sealed END, \
                secret_changed_at = CASE WHEN $9::bytea IS NOT NULL OR ($10 AND secret_sealed IS NOT NULL) THEN now() \
                                         ELSE secret_changed_at END, \
                updated_by = $11, updated_at = now(), version = version + 1 \
          WHERE id = $1 AND version = $2",
    )
    .bind(id)
    .bind(changes.version)
    .bind(&name)
    .bind(&kind)
    .bind(&username)
    .bind(&url)
    .bind(&notes)
    .bind(&tags)
    .bind(new_secret.as_deref())
    .bind(changes.clear_secret)
    .bind(ctx.actor.id())
    .execute(&mut *conn)
    .await?
    .rows_affected();
    if rows == 0 {
        return Err(VaultError::VersionConflict);
    }
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new("vault.item_updated")
            .org(before.org_id)
            .target("vault_item", id)
            .details(json!({ "name": name, "secret": secret_change })),
    )
    .await?;
    get(conn, id).await
}

/// Deletes an item. Its reveal log stays.
pub async fn delete(conn: &mut PgConnection, ctx: &RequestContext, id: Uuid) -> VaultResult<()> {
    let before = get(conn, id).await?;
    sqlx::query("DELETE FROM vault_items WHERE id = $1")
        .bind(id)
        .execute(&mut *conn)
        .await?;
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new("vault.item_deleted")
            .org(before.org_id)
            .target("vault_item", id)
            .details(json!({ "name": before.name })),
    )
    .await?;
    Ok(())
}

/// Reveals an item's secret, keeping who, when and why: in the reveal log,
/// the audit log, and a `vault.secret_revealed` event (none of them with
/// the secret).
pub async fn reveal(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    crypto: &Crypto,
    id: Uuid,
    reason: Option<&str>,
) -> VaultResult<Zeroizing<String>> {
    let item = get(conn, id).await?;
    let reason = text("reason", reason, 500)?.unwrap_or_default();
    let sealed: Option<Vec<u8>> =
        sqlx::query_scalar("SELECT secret_sealed FROM vault_items WHERE id = $1")
            .bind(id)
            .fetch_one(&mut *conn)
            .await?;
    let sealed = sealed.ok_or(VaultError::NoSecret)?;
    let plain = crypto
        .decrypt(
            Binding {
                purpose: PURPOSE,
                record: id,
            },
            &sealed,
        )
        .await
        .map_err(|e| VaultError::Crypto(e.to_string()))?;
    let secret = Zeroizing::new(String::from_utf8_lossy(&plain).into_owned());
    sqlx::query(
        "INSERT INTO vault_reveals (id, item_id, org_id, item_name, user_id, reason) VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(huxley_core::new_id())
    .bind(id)
    .bind(item.org_id)
    .bind(&item.name)
    .bind(ctx.actor.id())
    .bind(&reason)
    .execute(&mut *conn)
    .await?;
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new("vault.secret_revealed")
            .org(item.org_id)
            .target("vault_item", id)
            .details(json!({ "name": item.name, "reason": reason })),
    )
    .await?;
    outbox::emit(
        conn,
        DomainEvent {
            event_type: "vault.secret_revealed",
            org_id: Some(item.org_id),
            subject_type: "vault_item",
            subject_id: id,
            payload: json!({ "name": item.name, "kind": item.kind, "reason": reason, "actorId": ctx.actor.id() }),
        },
    )
    .await?;
    Ok(secret)
}

/// A reveal, as logged.
#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
#[schema(as = VaultReveal)]
pub struct Reveal {
    pub item_id: Option<Uuid>,
    pub item_name: String,
    pub user_name: Option<String>,
    pub reason: String,
    #[serde(with = "time::serde::rfc3339")]
    pub revealed_at: OffsetDateTime,
}

/// Reveals of one item, or of everything the scope can see (`item`
/// `None`), newest first.
pub async fn reveals(
    conn: &mut PgConnection,
    item: Option<Uuid>,
    limit: i64,
) -> VaultResult<Vec<Reveal>> {
    Ok(sqlx::query_as(
        "SELECT r.item_id, r.item_name, u.display_name AS user_name, r.reason, r.revealed_at \
           FROM vault_reveals r LEFT JOIN users u ON u.id = r.user_id \
          WHERE ($1::uuid IS NULL OR r.item_id = $1) \
          ORDER BY r.revealed_at DESC LIMIT $2",
    )
    .bind(item)
    .bind(limit.clamp(1, 500))
    .fetch_all(conn)
    .await?)
}
