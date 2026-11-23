//! Teams: named groups of users that hold role assignments (design doc
//! §11.3). Members receive the team's grants in addition to their own.
//! Callers check `team:manage` (and `role_assignment:manage` for granting a
//! team roles) before calling these; run them in system scope.

use huxley_core::RequestContext;
use huxley_db::audit::{self, AuditRecord};
use serde::Serialize;
use serde_json::json;
use sqlx::PgConnection;
use time::OffsetDateTime;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::{Assignment, AuthzError, AuthzResult, SELECT_ASSIGNMENTS};

#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct Team {
    pub id: Uuid,
    pub name: String,
    pub description: String,
    pub member_count: i64,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

/// A member, with enough about the user to display them.
#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct TeamMember {
    pub user_id: Uuid,
    pub display_name: String,
    pub email: String,
    /// `human` or `service`.
    pub kind: String,
    #[serde(with = "time::serde::rfc3339")]
    pub added_at: OffsetDateTime,
}

const SELECT_TEAMS: &str = "SELECT t.id, t.name, t.description, \
    (SELECT count(*) FROM team_members m WHERE m.team_id = t.id) AS member_count, t.created_at \
    FROM teams t";

fn normalize(name: &str, description: &str) -> AuthzResult<(String, String)> {
    let name = name.trim();
    let description = description.trim();
    if name.is_empty()
        || name.chars().count() > 100
        || name.chars().any(char::is_control)
        || description.chars().count() > 500
    {
        return Err(AuthzError::InvalidTeamName);
    }
    Ok((name.to_owned(), description.to_owned()))
}

pub async fn list(conn: &mut PgConnection) -> AuthzResult<Vec<Team>> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{SELECT_TEAMS} ORDER BY lower(t.name)"
    )))
    .fetch_all(conn)
    .await?)
}

pub async fn get(conn: &mut PgConnection, id: Uuid) -> AuthzResult<Team> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{SELECT_TEAMS} WHERE t.id = $1"
    )))
    .bind(id)
    .fetch_optional(conn)
    .await?
    .ok_or(AuthzError::TeamNotFound)
}

pub async fn create(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    name: &str,
    description: &str,
) -> AuthzResult<Team> {
    let (name, description) = normalize(name, description)?;
    let id = huxley_core::new_id();
    sqlx::query("INSERT INTO teams (id, name, description) VALUES ($1, $2, $3)")
        .bind(id)
        .bind(&name)
        .bind(&description)
        .execute(&mut *conn)
        .await?;
    audit::record(
        conn,
        ctx,
        AuditRecord::new("team.created")
            .target("team", id)
            .details(json!({ "name": name })),
    )
    .await?;
    get(conn, id).await
}

pub async fn update(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    id: Uuid,
    name: &str,
    description: &str,
) -> AuthzResult<Team> {
    let before = get(conn, id).await?;
    let (name, description) = normalize(name, description)?;
    sqlx::query(
        "UPDATE teams SET name = $2, description = $3, version = version + 1 WHERE id = $1",
    )
    .bind(id)
    .bind(&name)
    .bind(&description)
    .execute(&mut *conn)
    .await?;
    audit::record(
        conn,
        ctx,
        AuditRecord::new("team.updated")
            .target("team", id)
            .details(json!({
                "before": { "name": before.name, "description": before.description },
                "after": { "name": name, "description": description },
            })),
    )
    .await?;
    get(conn, id).await
}

/// Deletes a team and the role assignments it held. Members keep their own.
pub async fn delete(conn: &mut PgConnection, ctx: &RequestContext, id: Uuid) -> AuthzResult<()> {
    let team = get(conn, id).await?;
    sqlx::query("DELETE FROM teams WHERE id = $1")
        .bind(id)
        .execute(&mut *conn)
        .await?;
    audit::record(
        conn,
        ctx,
        AuditRecord::new("team.deleted")
            .target("team", id)
            .details(json!({ "name": team.name })),
    )
    .await?;
    Ok(())
}

pub async fn members(conn: &mut PgConnection, team_id: Uuid) -> AuthzResult<Vec<TeamMember>> {
    Ok(sqlx::query_as(
        "SELECT u.id AS user_id, u.display_name, u.email, u.kind::text AS kind, m.added_at \
           FROM team_members m JOIN users u ON u.id = m.user_id \
          WHERE m.team_id = $1 ORDER BY lower(u.display_name)",
    )
    .bind(team_id)
    .fetch_all(conn)
    .await?)
}

/// Adds a member. Idempotent.
pub async fn add_member(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    team_id: Uuid,
    user_id: Uuid,
) -> AuthzResult<()> {
    get(conn, team_id).await?;
    let exists: Option<(bool,)> = sqlx::query_as("SELECT true FROM users WHERE id = $1")
        .bind(user_id)
        .fetch_optional(&mut *conn)
        .await?;
    if exists.is_none() {
        return Err(AuthzError::UserNotFound);
    }
    let added = sqlx::query(
        "INSERT INTO team_members (team_id, user_id, added_by) VALUES ($1, $2, $3) \
         ON CONFLICT DO NOTHING",
    )
    .bind(team_id)
    .bind(user_id)
    .bind(ctx.actor.id())
    .execute(&mut *conn)
    .await?
    .rows_affected();
    if added > 0 {
        audit::record(
            conn,
            ctx,
            AuditRecord::new("team.member_added")
                .target("team", team_id)
                .details(json!({ "user": user_id })),
        )
        .await?;
        if ctx.actor.id() != Some(user_id) {
            let team = get(conn, team_id).await?;
            let title = format!("You were added to the team {}", team.name);
            huxley_notify::notify(
                conn,
                huxley_notify::Notice {
                    user_id,
                    kind: huxley_notify::kinds::TEAM_JOINED,
                    title: &title,
                    body: "You now have the team's roles as well as your own.",
                    link: None,
                },
            )
            .await?;
        }
    }
    Ok(())
}

pub async fn remove_member(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    team_id: Uuid,
    user_id: Uuid,
) -> AuthzResult<()> {
    let removed = sqlx::query("DELETE FROM team_members WHERE team_id = $1 AND user_id = $2")
        .bind(team_id)
        .bind(user_id)
        .execute(&mut *conn)
        .await?
        .rows_affected();
    if removed == 0 {
        return Err(AuthzError::TeamNotFound);
    }
    audit::record(
        conn,
        ctx,
        AuditRecord::new("team.member_removed")
            .target("team", team_id)
            .details(json!({ "user": user_id })),
    )
    .await?;
    Ok(())
}

/// The team's role assignments visible in the current RLS scope.
pub async fn assignments(conn: &mut PgConnection, team_id: Uuid) -> AuthzResult<Vec<Assignment>> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{SELECT_ASSIGNMENTS} WHERE a.team_id = $1 ORDER BY o.path, r.name"
    )))
    .bind(team_id)
    .fetch_all(conn)
    .await?)
}

/// Teams a user belongs to.
pub async fn of_user(conn: &mut PgConnection, user_id: Uuid) -> AuthzResult<Vec<Team>> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{SELECT_TEAMS} WHERE t.id IN (SELECT team_id FROM team_members WHERE user_id = $1) \
         ORDER BY lower(t.name)"
    )))
    .bind(user_id)
    .fetch_all(conn)
    .await?)
}
