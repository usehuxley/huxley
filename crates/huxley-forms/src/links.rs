//! Public links (design doc §6.8): anyone with one can fill a form in for
//! one organization, without signing in. Runs started that way act as each
//! workflow's run-as service account (as webhooks do), never as a person.
//! Forms that look choices up through integrations can't be public: the
//! lookup would show integration data to anyone with the link.

use huxley_core::RequestContext;
use huxley_db::audit::{self, AuditRecord};
use ring::digest::{SHA256, digest};
use serde::Serialize;
use serde_json::json;
use sqlx::PgConnection;
use time::OffsetDateTime;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::{Form, FormError, FormResult, db, get, hints, invalid};

/// The prefix of public form link tokens.
pub const TOKEN_PREFIX: &str = "hxf_";

/// A public link as its managers see it (never the token).
#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct LinkView {
    pub id: Uuid,
    pub form_id: Uuid,
    pub org_id: Uuid,
    pub name: String,
    #[serde(with = "time::serde::rfc3339::option")]
    pub expires_at: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub revoked_at: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub last_used_at: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

const VIEW: &str = "id, form_id, org_id, name, expires_at, revoked_at, last_used_at, created_at";

fn hash(token: &str) -> Vec<u8> {
    digest(&SHA256, token.as_bytes()).as_ref().to_vec()
}

fn new_token() -> String {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).expect("the OS random source works");
    format!(
        "{TOKEN_PREFIX}{}",
        base64::Engine::encode(&base64::engine::general_purpose::URL_SAFE_NO_PAD, bytes)
    )
}

/// Makes a public link to `form` for `org_id` (checked by the caller to be
/// the form's organization or below it). Returns the token, shown once.
pub async fn create_link(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    form: &Form,
    org_id: Uuid,
    name: &str,
    expires_at: Option<OffsetDateTime>,
) -> FormResult<(String, LinkView)> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 200 {
        return Err(invalid("name", "1 to 200 characters"));
    }
    if expires_at.is_some_and(|t| t <= OffsetDateTime::now_utc()) {
        return Err(invalid("expiresAt", "a time in the future"));
    }
    if form.archived {
        return Err(FormError::Archived);
    }
    refuse_lookups(form)?;
    let token = new_token();
    let id = huxley_core::new_id();
    let view: LinkView = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "INSERT INTO form_links (id, form_id, org_id, token_hash, name, expires_at, created_by) \
         VALUES ($1, $2, $3, $4, $5, $6, $7) RETURNING {VIEW}"
    )))
    .bind(id)
    .bind(form.id)
    .bind(org_id)
    .bind(hash(&token))
    .bind(name)
    .bind(expires_at)
    .bind(ctx.actor.id())
    .fetch_one(&mut *conn)
    .await?;
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new("form.link_created")
            .org(org_id)
            .target("form", form.id)
            .details(json!({ "link": id, "name": name })),
    )
    .await
    .map_err(db)?;
    Ok((token, view))
}

/// A form with looked-up choices can't be public.
pub(crate) fn refuse_lookups(form: &Form) -> FormResult<()> {
    let looks_up = form.schema["properties"].as_object().is_some_and(|p| {
        p.keys()
            .any(|name| hints::options_from(&form.ui, name).is_some())
    });
    if looks_up {
        return Err(invalid(
            "ui",
            "forms that look choices up through an integration can't have public links",
        ));
    }
    Ok(())
}

pub async fn links(conn: &mut PgConnection, form_id: Uuid) -> FormResult<Vec<LinkView>> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {VIEW} FROM form_links WHERE form_id = $1 ORDER BY revoked_at IS NOT NULL, created_at DESC"
    )))
    .bind(form_id)
    .fetch_all(conn)
    .await?)
}

/// Stops a link working at once.
pub async fn revoke_link(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    form_id: Uuid,
    link_id: Uuid,
) -> FormResult<LinkView> {
    let view: LinkView = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "UPDATE form_links SET revoked_at = coalesce(revoked_at, now()) \
          WHERE id = $1 AND form_id = $2 RETURNING {VIEW}"
    )))
    .bind(link_id)
    .bind(form_id)
    .fetch_optional(&mut *conn)
    .await?
    .ok_or(FormError::NotFound)?;
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new("form.link_revoked")
            .org(view.org_id)
            .target("form", form_id)
            .details(json!({ "link": link_id })),
    )
    .await
    .map_err(db)?;
    Ok(view)
}

/// The form and link a token opens, if it works: not revoked, not
/// expired, the form not archived and still without lookups. Runs as the
/// system: the token is the credential.
pub async fn open(conn: &mut PgConnection, token: &str) -> FormResult<(Form, LinkView)> {
    if !token.starts_with(TOKEN_PREFIX) {
        return Err(FormError::NotFound);
    }
    let link: LinkView = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {VIEW} FROM form_links WHERE token_hash = $1 \
            AND revoked_at IS NULL AND (expires_at IS NULL OR expires_at > now())"
    )))
    .bind(hash(token))
    .fetch_optional(&mut *conn)
    .await?
    .ok_or(FormError::NotFound)?;
    let form = get(&mut *conn, link.form_id).await?;
    if form.archived || refuse_lookups(&form).is_err() {
        return Err(FormError::NotFound);
    }
    Ok((form, link))
}
