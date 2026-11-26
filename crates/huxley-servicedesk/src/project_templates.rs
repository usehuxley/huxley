//! Project templates (ADR-056): phases with tasks (estimates, what they
//! depend on, whether the client sees them) and milestones, laid out when a
//! project is created from one.

use huxley_core::RequestContext;
use huxley_db::audit::{self, AuditRecord};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::PgConnection;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::{DeskError, DeskResult};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TemplateTask {
    /// Unique in the template: what `dependsOn` and assignments name.
    pub key: String,
    pub title: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub estimate_hours: Option<f64>,
    /// Keys of tasks that must be done first.
    #[serde(default)]
    pub depends_on: Vec<String>,
    #[serde(default)]
    pub client_visible: bool,
    /// Starts this many days after the project does.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_offset_days: Option<i32>,
    /// Due this many days after it starts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_days: Option<i32>,
    /// Skills it needs, which become the task's requirements.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skills: Vec<TemplateSkill>,
}

/// A skill a template task needs, at a level (1 learning … 4 expert).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TemplateSkill {
    pub skill_id: Uuid,
    pub min_level: i16,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TemplatePhase {
    pub name: String,
    #[serde(default)]
    pub budget_hours: Option<f64>,
    #[serde(default)]
    pub tasks: Vec<TemplateTask>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TemplateMilestone {
    pub name: String,
    /// Days after the project starts it's due.
    #[serde(default)]
    pub offset_days: i32,
    #[serde(default = "yes")]
    pub client_visible: bool,
}

fn yes() -> bool {
    true
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Structure {
    #[serde(default)]
    pub phases: Vec<TemplatePhase>,
    #[serde(default)]
    pub milestones: Vec<TemplateMilestone>,
}

impl Structure {
    /// Every task, with its phase's index.
    pub fn tasks(&self) -> impl Iterator<Item = (usize, &TemplateTask)> {
        self.phases
            .iter()
            .enumerate()
            .flat_map(|(i, p)| p.tasks.iter().map(move |t| (i, t)))
    }

    /// Checks names, unique task keys, dependencies that exist and don't
    /// loop, and sizes.
    pub fn check(&self) -> DeskResult<()> {
        let bad = |at: String, msg: &str| DeskError::invalid(&at, msg);
        if self.phases.len() > 50 || self.milestones.len() > 50 {
            return Err(bad(
                "structure".into(),
                "at most 50 phases and 50 milestones",
            ));
        }
        let mut keys: Vec<&str> = Vec::new();
        for (i, p) in self.phases.iter().enumerate() {
            if p.name.trim().is_empty() || p.name.chars().count() > 200 {
                return Err(bad(
                    format!("structure/phases/{i}/name"),
                    "1 to 200 characters",
                ));
            }
            if p.tasks.len() > 200 {
                return Err(bad(
                    format!("structure/phases/{i}/tasks"),
                    "at most 200 tasks a phase",
                ));
            }
            for (j, t) in p.tasks.iter().enumerate() {
                let at = format!("structure/phases/{i}/tasks/{j}");
                if t.key.trim().is_empty() || t.key.len() > 60 || keys.contains(&t.key.as_str()) {
                    return Err(bad(
                        format!("{at}/key"),
                        "a unique key of up to 60 characters",
                    ));
                }
                if t.title.trim().is_empty() || t.title.chars().count() > 300 {
                    return Err(bad(format!("{at}/title"), "1 to 300 characters"));
                }
                if t.estimate_hours
                    .is_some_and(|h| !(0.0..=10_000.0).contains(&h))
                {
                    return Err(bad(format!("{at}/estimateHours"), "0 to 10,000"));
                }
                if t.start_offset_days
                    .is_some_and(|d| !(0..=3650).contains(&d))
                    || t.duration_days.is_some_and(|d| !(0..=3650).contains(&d))
                {
                    return Err(bad(format!("{at}/startOffsetDays"), "0 to 3,650 days"));
                }
                if t.skills.len() > 20 || t.skills.iter().any(|s| !(1..=4).contains(&s.min_level)) {
                    return Err(bad(
                        format!("{at}/skills"),
                        "up to 20 skills, levels 1 to 4",
                    ));
                }
                keys.push(&t.key);
            }
        }
        for (_, t) in self.tasks() {
            if let Some(missing) = t.depends_on.iter().find(|d| !keys.contains(&d.as_str())) {
                return Err(bad(
                    format!("structure/tasks/{}/dependsOn", t.key),
                    &format!("no task called {missing:?}"),
                ));
            }
        }
        if let Some(key) = self.cycle() {
            return Err(bad(
                format!("structure/tasks/{key}/dependsOn"),
                "dependencies can't loop",
            ));
        }
        for (i, m) in self.milestones.iter().enumerate() {
            if m.name.trim().is_empty() || m.name.chars().count() > 200 {
                return Err(bad(
                    format!("structure/milestones/{i}/name"),
                    "1 to 200 characters",
                ));
            }
            if !(0..=3650).contains(&m.offset_days) {
                return Err(bad(
                    format!("structure/milestones/{i}/offsetDays"),
                    "0 to 3650",
                ));
            }
        }
        Ok(())
    }

    /// A task on a dependency loop, if any.
    fn cycle(&self) -> Option<String> {
        let deps: std::collections::BTreeMap<&str, Vec<&str>> = self
            .tasks()
            .map(|(_, t)| {
                (
                    t.key.as_str(),
                    t.depends_on.iter().map(String::as_str).collect(),
                )
            })
            .collect();
        let mut done = Vec::new();
        deps.keys()
            .find_map(|k| visit(k, &deps, &mut Vec::new(), &mut done))
    }
}

/// Depth-first search for a loop through `key`.
fn visit<'a>(
    key: &'a str,
    deps: &std::collections::BTreeMap<&'a str, Vec<&'a str>>,
    path: &mut Vec<&'a str>,
    done: &mut Vec<&'a str>,
) -> Option<String> {
    if path.contains(&key) {
        return Some(key.to_owned());
    }
    if done.contains(&key) {
        return None;
    }
    path.push(key);
    for d in deps.get(key).into_iter().flatten() {
        if let Some(found) = visit(d, deps, path, done) {
            return Some(found);
        }
    }
    path.pop();
    done.push(key);
    None
}

#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
#[schema(as = ProjectTemplate)]
pub struct Template {
    pub id: Uuid,
    pub org_id: Uuid,
    pub name: String,
    pub description: String,
    #[sqlx(json)]
    pub structure: Structure,
    pub version: i32,
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schema(as = ProjectTemplateInput)]
pub struct TemplateInput {
    pub org_id: Uuid,
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub structure: Structure,
    #[serde(default)]
    pub version: Option<i32>,
}

const SELECT: &str = "SELECT id, org_id, name, description, structure, version FROM prj_templates";

/// Templates in the transaction's scope, by name.
pub async fn list(conn: &mut PgConnection) -> DeskResult<Vec<Template>> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{SELECT} ORDER BY lower(name), id"
    )))
    .fetch_all(conn)
    .await?)
}

pub async fn get(conn: &mut PgConnection, id: Uuid) -> DeskResult<Template> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!("{SELECT} WHERE id = $1")))
        .bind(id)
        .fetch_optional(conn)
        .await?
        .ok_or(DeskError::NotFound)
}

fn checked(input: &TemplateInput) -> DeskResult<String> {
    let name = input.name.trim();
    if name.is_empty() || name.chars().count() > 200 {
        return Err(DeskError::invalid("name", "1 to 200 characters"));
    }
    if input.description.chars().count() > 2000 {
        return Err(DeskError::invalid(
            "description",
            "at most 2,000 characters",
        ));
    }
    input.structure.check()?;
    Ok(name.to_owned())
}

pub async fn create(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    input: &TemplateInput,
) -> DeskResult<Template> {
    let name = checked(input)?;
    let id = Uuid::now_v7();
    sqlx::query("INSERT INTO prj_templates (id, org_id, name, description, structure) VALUES ($1, $2, $3, $4, $5)")
        .bind(id)
        .bind(input.org_id)
        .bind(&name)
        .bind(input.description.trim())
        .bind(sqlx::types::Json(&input.structure))
        .execute(&mut *conn)
        .await?;
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new("project_template.created")
            .org(input.org_id)
            .details(json!({ "name": name })),
    )
    .await?;
    get(conn, id).await
}

pub async fn update(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    id: Uuid,
    input: &TemplateInput,
) -> DeskResult<Template> {
    let name = checked(input)?;
    let done = sqlx::query(
        "UPDATE prj_templates SET name = $2, description = $3, structure = $4, version = version + 1, updated_at = now() \
          WHERE id = $1 AND ($5::integer IS NULL OR version = $5)",
    )
    .bind(id)
    .bind(&name)
    .bind(input.description.trim())
    .bind(sqlx::types::Json(&input.structure))
    .bind(input.version)
    .execute(&mut *conn)
    .await?
    .rows_affected();
    if done == 0 {
        return Err(DeskError::VersionConflict);
    }
    let t = get(&mut *conn, id).await?;
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new("project_template.updated").org(t.org_id),
    )
    .await?;
    Ok(t)
}

pub async fn delete(conn: &mut PgConnection, ctx: &RequestContext, id: Uuid) -> DeskResult<bool> {
    let org: Option<Uuid> =
        sqlx::query_scalar("DELETE FROM prj_templates WHERE id = $1 RETURNING org_id")
            .bind(id)
            .fetch_optional(&mut *conn)
            .await?;
    if let Some(org) = org {
        audit::record(
            conn,
            ctx,
            AuditRecord::new("project_template.deleted").org(org),
        )
        .await?;
    }
    Ok(org.is_some())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task(key: &str, deps: &[&str]) -> TemplateTask {
        TemplateTask {
            key: key.into(),
            title: key.into(),
            description: String::new(),
            estimate_hours: Some(2.0),
            depends_on: deps.iter().map(|d| (*d).to_owned()).collect(),
            client_visible: false,
            start_offset_days: None,
            duration_days: None,
            skills: Vec::new(),
        }
    }

    #[test]
    fn structures_are_checked() {
        let ok = Structure {
            phases: vec![TemplatePhase {
                name: "Setup".into(),
                budget_hours: Some(10.0),
                tasks: vec![task("a", &[]), task("b", &["a"])],
            }],
            milestones: vec![TemplateMilestone {
                name: "Live".into(),
                offset_days: 14,
                client_visible: true,
            }],
        };
        assert!(ok.check().is_ok());
        let mut looping = ok.clone();
        looping.phases[0].tasks[0].depends_on = vec!["b".into()];
        assert!(looping.check().is_err());
        let mut missing = ok.clone();
        missing.phases[0].tasks[1].depends_on = vec!["zz".into()];
        assert!(missing.check().is_err());
        let mut twice = ok;
        twice.phases[0].tasks[1].key = "a".into();
        assert!(twice.check().is_err());
    }
}
