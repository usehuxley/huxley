//! SCIM 2.0 provisioning (ADR-088): what an identity provider provisioned,
//! linked to the sign-in provider it belongs to. SCIM users are Huxley users
//! (their SCIM id is the user's id); SCIM groups are teams. The protocol
//! (JSON shapes, filters, PATCH) lives in the API; this is the data.

use huxley_auth::{AuthError, UserKind, UserStatus};
use huxley_core::RequestContext;
use huxley_db::audit::{self, AuditRecord};
use serde_json::{Value, json};
use sqlx::PgConnection;
use time::OffsetDateTime;
use uuid::Uuid;

#[derive(Debug, thiserror::Error)]
pub enum ScimError {
    #[error("not found")]
    NotFound,
    /// Something with that unique value already exists.
    #[error("{0}")]
    Conflict(String),
    /// A value that doesn't fit.
    #[error("{0}")]
    Invalid(String),
    #[error(transparent)]
    Db(#[from] huxley_db::DbError),
    #[error(transparent)]
    Auth(#[from] AuthError),
    #[error(transparent)]
    Authz(#[from] huxley_authz::AuthzError),
}

impl From<sqlx::Error> for ScimError {
    fn from(e: sqlx::Error) -> Self {
        Self::Db(huxley_db::DbError::Sqlx(e))
    }
}

pub type ScimResult<T> = Result<T, ScimError>;

/// The sign-in provider a SCIM token belongs to.
#[derive(Debug, Clone)]
pub struct ScimProvider {
    pub id: Uuid,
    pub key: String,
    pub name: String,
}

impl ScimProvider {
    /// The context SCIM changes are made (and audited) in.
    pub fn context(&self, base: &RequestContext) -> RequestContext {
        RequestContext {
            actor: huxley_core::Actor::System,
            ..base.clone()
        }
    }
}

fn token_hash(token: &str) -> Vec<u8> {
    huxley_auth::token::hash(token)
}

/// The provider a SCIM bearer token belongs to (and notes it was used).
pub async fn provider_by_token(
    conn: &mut PgConnection,
    token: &str,
) -> ScimResult<Option<ScimProvider>> {
    let row: Option<(Uuid, String, String)> = sqlx::query_as(
        "UPDATE oidc_providers SET scim_last_used_at = now() \
          WHERE scim_token_hash = $1 RETURNING id, key, name",
    )
    .bind(token_hash(token))
    .fetch_optional(conn)
    .await?;
    Ok(row.map(|(id, key, name)| ScimProvider { id, key, name }))
}

/// SCIM settings for a provider, as shown to admins.
#[derive(Debug, Clone, serde::Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ScimStatus {
    pub enabled: bool,
    #[serde(with = "time::serde::rfc3339::option")]
    pub token_created_at: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub last_used_at: Option<OffsetDateTime>,
    pub users: i64,
    pub groups: i64,
}

/// (enabled, token created, last used, users, groups)
type StatusRow = (
    bool,
    Option<OffsetDateTime>,
    Option<OffsetDateTime>,
    i64,
    i64,
);

pub async fn status(conn: &mut PgConnection, provider_id: Uuid) -> ScimResult<ScimStatus> {
    let row: Option<StatusRow> = sqlx::query_as(
        "SELECT scim_token_hash IS NOT NULL, scim_token_created_at, scim_last_used_at, \
                    (SELECT count(*) FROM scim_users WHERE provider_id = p.id), \
                    (SELECT count(*) FROM scim_groups WHERE provider_id = p.id) \
               FROM oidc_providers p WHERE id = $1",
    )
    .bind(provider_id)
    .fetch_optional(conn)
    .await?;
    let (enabled, token_created_at, last_used_at, users, groups) =
        row.ok_or(ScimError::NotFound)?;
    Ok(ScimStatus {
        enabled,
        token_created_at,
        last_used_at,
        users,
        groups,
    })
}

/// Turns SCIM on for a provider with a new token (replacing any old one),
/// and returns the token: it's shown once.
pub async fn new_token(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    provider_id: Uuid,
) -> ScimResult<String> {
    let token = huxley_auth::token::generate("hxscim_").secret;
    let changed = sqlx::query(
        "UPDATE oidc_providers SET scim_token_hash = $2, scim_token_created_at = now() WHERE id = $1",
    )
    .bind(provider_id)
    .bind(token_hash(&token))
    .execute(&mut *conn)
    .await?
    .rows_affected();
    if changed == 0 {
        return Err(ScimError::NotFound);
    }
    audit::record(
        conn,
        ctx,
        AuditRecord::new("sso.scim_token_created").target("oidc_provider", provider_id),
    )
    .await?;
    Ok(token)
}

/// Turns SCIM off for a provider. What it provisioned stays.
pub async fn disable(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    provider_id: Uuid,
) -> ScimResult<()> {
    let changed = sqlx::query(
        "UPDATE oidc_providers SET scim_token_hash = NULL, scim_token_created_at = NULL WHERE id = $1",
    )
    .bind(provider_id)
    .execute(&mut *conn)
    .await?
    .rows_affected();
    if changed == 0 {
        return Err(ScimError::NotFound);
    }
    audit::record(
        conn,
        ctx,
        AuditRecord::new("sso.scim_disabled").target("oidc_provider", provider_id),
    )
    .await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Users

/// A provisioned user.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ScimUser {
    pub id: Uuid,
    pub user_name: String,
    pub external_id: Option<String>,
    pub name: Value,
    pub display_name: String,
    pub email: String,
    pub active: bool,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

/// What a SCIM client sets on a user.
#[derive(Debug, Clone, Default)]
pub struct UserInput {
    pub user_name: String,
    pub external_id: Option<String>,
    pub name: Value,
    pub display_name: Option<String>,
    /// The primary (or first) email; the user name when it's an address.
    pub email: Option<String>,
    pub active: bool,
}

impl UserInput {
    fn email(&self) -> ScimResult<String> {
        let email = self
            .email
            .clone()
            .filter(|e| e.contains('@'))
            .or_else(|| Some(self.user_name.clone()).filter(|u| u.contains('@')))
            .ok_or_else(|| ScimError::Invalid("an email address (in emails or userName)".into()))?;
        Ok(email.trim().to_owned())
    }

    fn display(&self) -> String {
        let named = || {
            let formatted = self.name["formatted"]
                .as_str()
                .map(str::trim)
                .filter(|s| !s.is_empty());
            formatted.map(str::to_owned).or_else(|| {
                let parts: Vec<&str> = ["givenName", "familyName"]
                    .iter()
                    .filter_map(|k| {
                        self.name[*k]
                            .as_str()
                            .map(str::trim)
                            .filter(|s| !s.is_empty())
                    })
                    .collect();
                (!parts.is_empty()).then(|| parts.join(" "))
            })
        };
        self.display_name
            .clone()
            .map(|d| d.trim().to_owned())
            .filter(|d| !d.is_empty())
            .or_else(named)
            .unwrap_or_else(|| self.user_name.clone())
    }
}

macro_rules! select_users {
    ($rest:literal) => {
        concat!(
            "SELECT u.id, s.user_name, s.external_id, s.name, u.display_name, u.email, \
                    u.status = 'active' AS active, s.created_at, greatest(s.updated_at, u.updated_at) AS updated_at \
               FROM scim_users s JOIN users u ON u.id = s.user_id ",
            $rest
        )
    };
}

/// What users can be filtered by (`eq` only).
#[derive(Debug, Clone)]
pub enum UserFilter {
    UserName(String),
    ExternalId(String),
    Email(String),
}

pub async fn list_users(
    conn: &mut PgConnection,
    provider: &ScimProvider,
    filter: Option<&UserFilter>,
    offset: i64,
    limit: i64,
) -> ScimResult<(i64, Vec<ScimUser>)> {
    let (kind, value) = match filter {
        None => ("", ""),
        Some(UserFilter::UserName(v)) => ("userName", v.as_str()),
        Some(UserFilter::ExternalId(v)) => ("externalId", v.as_str()),
        Some(UserFilter::Email(v)) => ("email", v.as_str()),
    };
    let total: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM scim_users s JOIN users u ON u.id = s.user_id \
          WHERE s.provider_id = $1 \
            AND ($2 = '' OR ($2 = 'userName' AND lower(s.user_name) = lower($3)) \
                 OR ($2 = 'externalId' AND s.external_id = $3) \
                 OR ($2 = 'email' AND lower(u.email) = lower($3)))",
    )
    .bind(provider.id)
    .bind(kind)
    .bind(value)
    .fetch_one(&mut *conn)
    .await?;
    let users = sqlx::query_as(select_users!(
        "WHERE s.provider_id = $1 \
           AND ($2 = '' OR ($2 = 'userName' AND lower(s.user_name) = lower($3)) \
                OR ($2 = 'externalId' AND s.external_id = $3) \
                OR ($2 = 'email' AND lower(u.email) = lower($3))) \
         ORDER BY s.created_at, u.id OFFSET $4 LIMIT $5"
    ))
    .bind(provider.id)
    .bind(kind)
    .bind(value)
    .bind(offset)
    .bind(limit)
    .fetch_all(&mut *conn)
    .await?;
    Ok((total, users))
}

pub async fn get_user(
    conn: &mut PgConnection,
    provider: &ScimProvider,
    id: Uuid,
) -> ScimResult<ScimUser> {
    sqlx::query_as(select_users!("WHERE s.provider_id = $1 AND u.id = $2"))
        .bind(provider.id)
        .bind(id)
        .fetch_optional(conn)
        .await?
        .ok_or(ScimError::NotFound)
}

async fn user_name_taken(
    conn: &mut PgConnection,
    provider: &ScimProvider,
    user_name: &str,
    except: Option<Uuid>,
) -> ScimResult<bool> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM scim_users WHERE provider_id = $1 AND lower(user_name) = lower($2) \
                          AND ($3::uuid IS NULL OR user_id <> $3))",
    )
    .bind(provider.id)
    .bind(user_name.trim())
    .bind(except)
    .fetch_one(conn)
    .await?)
}

fn check_user(input: &UserInput) -> ScimResult<()> {
    let user_name = input.user_name.trim();
    if user_name.is_empty() || user_name.chars().count() > 320 {
        return Err(ScimError::Invalid("userName: up to 320 characters".into()));
    }
    Ok(())
}

/// Provisions a user: a new Huxley user, or the existing one with that
/// email (adopted and linked).
pub async fn create_user(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    provider: &ScimProvider,
    input: &UserInput,
) -> ScimResult<ScimUser> {
    check_user(input)?;
    if user_name_taken(conn, provider, &input.user_name, None).await? {
        return Err(ScimError::Conflict(format!(
            "a user with userName {} exists",
            input.user_name.trim()
        )));
    }
    let email = input.email()?;
    let user = match huxley_auth::find_user_by_email(conn, &email).await? {
        Some(existing) if existing.kind == UserKind::Service => {
            return Err(ScimError::Conflict(format!("{email} is a service account")));
        }
        Some(existing) => {
            let linked: bool = sqlx::query_scalar(
                "SELECT EXISTS (SELECT 1 FROM scim_users WHERE provider_id = $1 AND user_id = $2)",
            )
            .bind(provider.id)
            .bind(existing.id)
            .fetch_one(&mut *conn)
            .await?;
            if linked {
                return Err(ScimError::Conflict(format!(
                    "a user with the email {email} exists"
                )));
            }
            existing
        }
        None => {
            huxley_auth::create_external_user(conn, ctx, &email, &input.display(), &provider.key)
                .await?
        }
    };
    sqlx::query(
        "INSERT INTO scim_users (provider_id, user_id, user_name, external_id, name) VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(provider.id)
    .bind(user.id)
    .bind(input.user_name.trim())
    .bind(input.external_id.as_deref())
    .bind(&input.name)
    .execute(&mut *conn)
    .await?;
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new("user.provisioned")
            .target("user", user.id)
            .details(json!({ "provider": provider.key, "userName": input.user_name.trim(), "externalId": input.external_id })),
    )
    .await?;
    // A provisioned user's name and status follow the provider's.
    apply_user(conn, ctx, provider, user.id, input).await?;
    get_user(conn, provider, user.id).await
}

/// Sets a provisioned user's attributes (SCIM PUT, and PATCH once applied).
pub async fn replace_user(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    provider: &ScimProvider,
    id: Uuid,
    input: &UserInput,
) -> ScimResult<ScimUser> {
    check_user(input)?;
    get_user(conn, provider, id).await?;
    if user_name_taken(conn, provider, &input.user_name, Some(id)).await? {
        return Err(ScimError::Conflict(format!(
            "a user with userName {} exists",
            input.user_name.trim()
        )));
    }
    sqlx::query(
        "UPDATE scim_users SET user_name = $3, external_id = $4, name = $5, updated_at = now() \
          WHERE provider_id = $1 AND user_id = $2",
    )
    .bind(provider.id)
    .bind(id)
    .bind(input.user_name.trim())
    .bind(input.external_id.as_deref())
    .bind(&input.name)
    .execute(&mut *conn)
    .await?;
    apply_user(conn, ctx, provider, id, input).await?;
    get_user(conn, provider, id).await
}

/// Brings the Huxley user in line: email, name and status (disabling
/// revokes their sessions).
async fn apply_user(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    provider: &ScimProvider,
    id: Uuid,
    input: &UserInput,
) -> ScimResult<()> {
    let user = huxley_auth::find_user(conn, id)
        .await?
        .ok_or(ScimError::NotFound)?;
    let email = input.email()?;
    if !email.eq_ignore_ascii_case(&user.email) {
        if huxley_auth::find_user_by_email(conn, &email)
            .await?
            .is_some()
        {
            return Err(ScimError::Conflict(format!(
                "another user has the email {email}"
            )));
        }
        sqlx::query(
            "UPDATE users SET email = $2, email_verified_at = now(), version = version + 1 WHERE id = $1",
        )
        .bind(id)
        .bind(&email)
        .execute(&mut *conn)
        .await?;
        audit::record(
            &mut *conn,
            ctx,
            AuditRecord::new("user.email_changed")
                .target("user", id)
                .details(json!({ "before": user.email, "after": email, "provider": provider.key })),
        )
        .await?;
    }
    let display = input.display();
    let status = if input.active {
        UserStatus::Active
    } else {
        UserStatus::Disabled
    };
    if display != user.display_name || status != user.status {
        huxley_auth::update_user(conn, ctx, id, Some(&display), Some(status)).await?;
    }
    Ok(())
}

/// Deprovisions a user: disabled (sessions revoked), unlinked, and out of
/// the provider's teams. The user and their history stay.
pub async fn delete_user(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    provider: &ScimProvider,
    id: Uuid,
) -> ScimResult<()> {
    get_user(conn, provider, id).await?;
    huxley_auth::update_user(conn, ctx, id, None, Some(UserStatus::Disabled)).await?;
    sqlx::query(
        "DELETE FROM team_members WHERE user_id = $2 \
           AND team_id IN (SELECT team_id FROM scim_groups WHERE provider_id = $1)",
    )
    .bind(provider.id)
    .bind(id)
    .execute(&mut *conn)
    .await?;
    sqlx::query("DELETE FROM scim_users WHERE provider_id = $1 AND user_id = $2")
        .bind(provider.id)
        .bind(id)
        .execute(&mut *conn)
        .await?;
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new("user.deprovisioned")
            .target("user", id)
            .details(json!({ "provider": provider.key })),
    )
    .await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Groups (teams)

#[derive(Debug, Clone)]
pub struct ScimGroup {
    pub id: Uuid,
    pub display_name: String,
    pub external_id: Option<String>,
    /// (user id, display name)
    pub members: Vec<(Uuid, String)>,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

#[derive(Debug, Clone, Default)]
pub struct GroupInput {
    pub display_name: String,
    pub external_id: Option<String>,
    pub members: Vec<Uuid>,
}

#[derive(Debug, Clone)]
pub enum GroupFilter {
    DisplayName(String),
    ExternalId(String),
}

type GroupRow = (Uuid, String, Option<String>, OffsetDateTime, OffsetDateTime);

async fn with_members(
    conn: &mut PgConnection,
    rows: Vec<GroupRow>,
    members: bool,
) -> ScimResult<Vec<ScimGroup>> {
    let mut out = Vec::with_capacity(rows.len());
    for (id, display_name, external_id, created_at, updated_at) in rows {
        let members = if members {
            sqlx::query_as(
                "SELECT u.id, u.display_name FROM team_members m JOIN users u ON u.id = m.user_id \
                  WHERE m.team_id = $1 ORDER BY u.id",
            )
            .bind(id)
            .fetch_all(&mut *conn)
            .await?
        } else {
            Vec::new()
        };
        out.push(ScimGroup {
            id,
            display_name,
            external_id,
            members,
            created_at,
            updated_at,
        });
    }
    Ok(out)
}

/// Groups, with their members unless `members` is false (clients that
/// only look groups up ask to leave members out).
pub async fn list_groups(
    conn: &mut PgConnection,
    provider: &ScimProvider,
    filter: Option<&GroupFilter>,
    offset: i64,
    limit: i64,
    members: bool,
) -> ScimResult<(i64, Vec<ScimGroup>)> {
    let (kind, value) = match filter {
        None => ("", ""),
        Some(GroupFilter::DisplayName(v)) => ("displayName", v.as_str()),
        Some(GroupFilter::ExternalId(v)) => ("externalId", v.as_str()),
    };
    let total: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM scim_groups g JOIN teams t ON t.id = g.team_id \
          WHERE g.provider_id = $1 AND ($2 = '' OR ($2 = 'displayName' AND lower(t.name) = lower($3)) \
                OR ($2 = 'externalId' AND g.external_id = $3))",
    )
    .bind(provider.id)
    .bind(kind)
    .bind(value)
    .fetch_one(&mut *conn)
    .await?;
    let rows: Vec<GroupRow> = sqlx::query_as(
        "SELECT t.id, t.name, g.external_id, g.created_at, greatest(g.updated_at, t.updated_at) \
           FROM scim_groups g JOIN teams t ON t.id = g.team_id \
          WHERE g.provider_id = $1 AND ($2 = '' OR ($2 = 'displayName' AND lower(t.name) = lower($3)) \
                OR ($2 = 'externalId' AND g.external_id = $3)) \
          ORDER BY g.created_at, t.id OFFSET $4 LIMIT $5",
    )
    .bind(provider.id)
    .bind(kind)
    .bind(value)
    .bind(offset)
    .bind(limit)
    .fetch_all(&mut *conn)
    .await?;
    Ok((total, with_members(conn, rows, members).await?))
}

pub async fn get_group(
    conn: &mut PgConnection,
    provider: &ScimProvider,
    id: Uuid,
) -> ScimResult<ScimGroup> {
    let rows: Vec<GroupRow> = sqlx::query_as(
        "SELECT t.id, t.name, g.external_id, g.created_at, greatest(g.updated_at, t.updated_at) \
           FROM scim_groups g JOIN teams t ON t.id = g.team_id WHERE g.provider_id = $1 AND t.id = $2",
    )
    .bind(provider.id)
    .bind(id)
    .fetch_all(&mut *conn)
    .await?;
    with_members(conn, rows, true)
        .await?
        .pop()
        .ok_or(ScimError::NotFound)
}

/// Creates the team for a group, or adopts the unmanaged team with that
/// name (keeping its role assignments).
pub async fn create_group(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    provider: &ScimProvider,
    input: &GroupInput,
) -> ScimResult<ScimGroup> {
    let name = input.display_name.trim();
    let existing: Option<(Uuid, bool)> = sqlx::query_as(
        "SELECT t.id, EXISTS (SELECT 1 FROM scim_groups g WHERE g.team_id = t.id) \
           FROM teams t WHERE lower(t.name) = lower($1)",
    )
    .bind(name)
    .fetch_optional(&mut *conn)
    .await?;
    let team_id = match existing {
        Some((_, true)) => return Err(ScimError::Conflict(format!("a group named {name} exists"))),
        Some((id, false)) => id,
        None => {
            huxley_authz::teams::create(conn, ctx, name, "")
                .await
                .map_err(group_error)?
                .id
        }
    };
    sqlx::query("INSERT INTO scim_groups (provider_id, team_id, external_id) VALUES ($1, $2, $3)")
        .bind(provider.id)
        .bind(team_id)
        .bind(input.external_id.as_deref())
        .execute(&mut *conn)
        .await?;
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new("team.provisioned")
            .target("team", team_id)
            .details(json!({ "provider": provider.key, "externalId": input.external_id })),
    )
    .await?;
    set_members(conn, ctx, team_id, &input.members).await?;
    get_group(conn, provider, team_id).await
}

fn group_error(e: huxley_authz::AuthzError) -> ScimError {
    match e {
        huxley_authz::AuthzError::InvalidTeamName => {
            ScimError::Invalid("displayName: a name of up to 100 characters".into())
        }
        huxley_authz::AuthzError::TeamNameTaken => {
            ScimError::Conflict("a group with that name exists".into())
        }
        other => ScimError::Authz(other),
    }
}

/// Sets a group's name and members (SCIM PUT, and PATCH once applied).
pub async fn replace_group(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    provider: &ScimProvider,
    id: Uuid,
    input: &GroupInput,
) -> ScimResult<ScimGroup> {
    let current = get_group(conn, provider, id).await?;
    let name = input.display_name.trim();
    if name != current.display_name {
        let taken: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM teams WHERE lower(name) = lower($1) AND id <> $2)",
        )
        .bind(name)
        .bind(id)
        .fetch_one(&mut *conn)
        .await?;
        if taken {
            return Err(ScimError::Conflict(format!("a group named {name} exists")));
        }
        let description: String = sqlx::query_scalar("SELECT description FROM teams WHERE id = $1")
            .bind(id)
            .fetch_one(&mut *conn)
            .await?;
        huxley_authz::teams::update(conn, ctx, id, name, &description)
            .await
            .map_err(group_error)?;
    }
    sqlx::query(
        "UPDATE scim_groups SET external_id = $3, updated_at = now() WHERE provider_id = $1 AND team_id = $2",
    )
    .bind(provider.id)
    .bind(id)
    .bind(input.external_id.as_deref())
    .execute(&mut *conn)
    .await?;
    set_members(conn, ctx, id, &input.members).await?;
    get_group(conn, provider, id).await
}

/// Makes the team's members exactly `members`: the provider is the source
/// of truth for a team it manages.
async fn set_members(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    team_id: Uuid,
    members: &[Uuid],
) -> ScimResult<()> {
    let known: Vec<Uuid> = sqlx::query_scalar("SELECT id FROM users WHERE id = ANY($1)")
        .bind(members)
        .fetch_all(&mut *conn)
        .await?;
    if let Some(unknown) = members.iter().find(|m| !known.contains(m)) {
        return Err(ScimError::Invalid(format!("members: no user {unknown}")));
    }
    let current: Vec<Uuid> =
        sqlx::query_scalar("SELECT user_id FROM team_members WHERE team_id = $1")
            .bind(team_id)
            .fetch_all(&mut *conn)
            .await?;
    for user in members.iter().filter(|m| !current.contains(m)) {
        huxley_authz::teams::add_member(conn, ctx, team_id, *user, None).await?;
    }
    for user in current.iter().filter(|c| !members.contains(c)) {
        huxley_authz::teams::remove_member(conn, ctx, team_id, *user).await?;
    }
    Ok(())
}

/// Removes a group's team (and with it the team's role assignments).
pub async fn delete_group(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    provider: &ScimProvider,
    id: Uuid,
) -> ScimResult<()> {
    get_group(conn, provider, id).await?;
    huxley_authz::teams::delete(conn, ctx, id).await?;
    Ok(())
}

/// Whether a team is managed by a provider (and which), for the UI.
pub async fn team_manager(conn: &mut PgConnection, team_id: Uuid) -> ScimResult<Option<String>> {
    Ok(sqlx::query_scalar(
        "SELECT p.name FROM scim_groups g JOIN oidc_providers p ON p.id = g.provider_id WHERE g.team_id = $1",
    )
    .bind(team_id)
    .fetch_optional(conn)
    .await?)
}
