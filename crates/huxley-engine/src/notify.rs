//! The `core.notify` step. Notifications are written in the transaction that
//! records the step's result, so a step sends them exactly once.

use serde_json::{Value as Json, json};
use sqlx::PgConnection;
use uuid::Uuid;

use crate::walk::error;
use crate::{EngineError, EngineResult};

/// Most people one step may notify.
const MAX_RECIPIENTS: usize = 500;

enum Target {
    User(Uuid),
    Team(Uuid),
    Role(String),
}

fn targets(to: &Json) -> Result<Vec<Target>, Json> {
    let entries: Vec<&str> = match to {
        Json::String(s) => vec![s.as_str()],
        Json::Array(items) => items
            .iter()
            .map(|i| {
                i.as_str()
                    .ok_or_else(|| error("definition", "recipients are strings"))
            })
            .collect::<Result<_, _>>()?,
        _ => return Err(error("definition", "`to` is a recipient or a list of them")),
    };
    entries
        .into_iter()
        .map(|entry| {
            let (kind, value) = entry.trim().split_once(':').unwrap_or(("", entry));
            let id = || {
                value
                    .parse::<Uuid>()
                    .map_err(|_| error("definition", format!("{entry:?}: expected {kind}:<id>")))
            };
            match kind {
                "user" => Ok(Target::User(id()?)),
                "team" => Ok(Target::Team(id()?)),
                "role" if !value.is_empty() => Ok(Target::Role(value.to_owned())),
                _ => Err(error(
                    "definition",
                    format!("{entry:?}: use user:<id>, team:<id> or role:<key>"),
                )),
            }
        })
        .collect()
}

/// The people `to` names (`user:<id>`, `team:<id>`, `role:<key>` at
/// `org_id`). The outer result is infrastructure; the inner one says what's
/// wrong with `to`.
pub(crate) async fn recipients(
    conn: &mut PgConnection,
    org_id: Uuid,
    to: &Json,
) -> EngineResult<Result<Vec<Uuid>, Json>> {
    let targets = match targets(to) {
        Ok(targets) => targets,
        Err(e) => return Ok(Err(e)),
    };
    let mut users: Vec<Uuid> = Vec::new();
    for target in targets {
        let found: Vec<(Uuid,)> = match target {
            Target::User(id) => vec![(id,)],
            Target::Team(id) => {
                sqlx::query_as("SELECT user_id FROM team_members WHERE team_id = $1")
                    .bind(id)
                    .fetch_all(&mut *conn)
                    .await?
            }
            // Everyone holding the role at the run's org: directly or through
            // a team, assigned there or (inheriting) on an ancestor.
            Target::Role(key) => {
                sqlx::query_as(
                    "SELECT DISTINCT COALESCE(ra.user_id, tm.user_id) \
                       FROM role_assignments ra \
                       JOIN roles r ON r.id = ra.role_id \
                       JOIN organizations at ON at.id = ra.org_id \
                       JOIN organizations here ON here.id = $2 \
                       LEFT JOIN team_members tm ON tm.team_id = ra.team_id \
                      WHERE r.key = $1 \
                        AND (ra.org_id = here.id OR (ra.inherit AND at.path @> here.path)) \
                        AND COALESCE(ra.user_id, tm.user_id) IS NOT NULL",
                )
                .bind(&key)
                .bind(org_id)
                .fetch_all(&mut *conn)
                .await?
            }
        };
        for (user,) in found {
            if !users.contains(&user) {
                users.push(user);
            }
        }
        if users.len() > MAX_RECIPIENTS {
            return Ok(Err(error(
                "limit",
                format!("at most {MAX_RECIPIENTS} people"),
            )));
        }
    }
    Ok(Ok(users))
}

/// The user ids in `exclude`: one, a list, or none. Empty values (an
/// expression that came out empty) are ignored.
fn excluded(exclude: &Json) -> Result<Vec<Uuid>, Json> {
    let items: Vec<&Json> = match exclude {
        Json::Null => Vec::new(),
        Json::Array(items) => items.iter().collect(),
        one => vec![one],
    };
    items
        .into_iter()
        .filter_map(|item| match item {
            Json::Null => None,
            Json::String(s) if s.trim().is_empty() || s == "None" => None,
            Json::String(s) => Some(
                s.trim()
                    .trim_start_matches("user:")
                    .parse::<Uuid>()
                    .map_err(|_| error("definition", format!("exclude: {s:?} isn't a user id"))),
            ),
            _ => Some(Err(error("definition", "exclude lists user ids"))),
        })
        .collect()
}

/// Sends the notification. The outer result is infrastructure; the inner one
/// is the step's outcome.
pub(crate) async fn send(
    conn: &mut PgConnection,
    org_id: Uuid,
    input: &Json,
) -> EngineResult<Result<Json, Json>> {
    let title = input["title"].as_str().unwrap_or_default().trim();
    if title.is_empty() {
        return Ok(Err(error("definition", "a notification needs a title")));
    }
    let kind = match input["kind"].as_str().filter(|k| !k.is_empty()) {
        None => huxley_notify::kinds::WORKFLOW,
        Some(key) => match huxley_notify::kinds::sendable(key) {
            Some(kind) => kind.key,
            None => {
                return Ok(Err(error(
                    "definition",
                    format!("{key:?} isn't a kind workflows can send"),
                )));
            }
        },
    };
    let exclude = match excluded(&input["exclude"]) {
        Ok(ids) => ids,
        Err(e) => return Ok(Err(e)),
    };
    let users: Vec<Uuid> = match recipients(&mut *conn, org_id, &input["to"]).await? {
        Ok(users) => users.into_iter().filter(|u| !exclude.contains(u)).collect(),
        Err(e) => return Ok(Err(e)),
    };
    let message = input["message"].as_str().unwrap_or_default();
    let link = input["url"].as_str().filter(|u| u.starts_with('/'));

    let mut sent = 0;
    for user_id in &users {
        let notice = huxley_notify::Notice {
            user_id: *user_id,
            kind,
            title,
            body: message,
            link,
        };
        if huxley_notify::notify(conn, notice)
            .await
            .map_err(|e| EngineError::Document(e.to_string()))?
            .is_some()
        {
            sent += 1;
        }
    }
    Ok(Ok(json!({ "recipients": users.len(), "sent": sent })))
}
