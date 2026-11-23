//! Where workflows call integration actions: publish-time checks that
//! every `integration.action` step names something that exists, and which
//! published workflows an update would break.

use std::collections::BTreeMap;

use huxley_hwf::{Diagnostic, Step, Workflow};
use serde::Serialize;
use serde_json::Value as Json;
use sqlx::PgConnection;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::Definition;
use crate::diff::DefinitionDiff;

/// One `integration.action` step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActionUse {
    /// Where it is, e.g. `spec.steps[2].then[0]`.
    pub path: String,
    pub step_id: String,
    pub integration: String,
    pub action: String,
}

/// Every `integration.action` step in a workflow.
pub fn action_uses(workflow: &Workflow) -> Vec<ActionUse> {
    fn visit(path: &str, steps: &[Step], out: &mut Vec<ActionUse>) {
        for (i, step) in steps.iter().enumerate() {
            let here = format!("{path}[{i}]");
            if let Step::IntegrationAction(s) = step
                && let Some((integration, action)) = s.action.split_once('/')
            {
                out.push(ActionUse {
                    path: here.clone(),
                    step_id: step.id().to_owned(),
                    integration: integration.to_owned(),
                    action: action.to_owned(),
                });
            }
            for (label, block) in step.blocks() {
                visit(&format!("{here}.{label}"), block, out);
            }
        }
    }
    let mut out = Vec::new();
    visit("spec.steps", &workflow.spec.steps, &mut out);
    visit("spec.on_error", &workflow.spec.on_error, &mut out);
    out
}

/// The integration `key` refers to from `org_id` (nearest organization
/// first), with its current definition and aliases.
async fn current(
    conn: &mut PgConnection,
    org_id: Uuid,
    key: &str,
) -> sqlx::Result<Option<(Definition, Json)>> {
    let row: Option<(Json, Json)> = sqlx::query_as(
        "SELECT v.definition, v.aliases FROM integrations i \
           JOIN organizations o ON o.id = i.org_id \
           JOIN integration_versions v ON v.integration_id = i.id AND v.version = i.current_version \
          WHERE i.key = $1 AND o.path @> (SELECT path FROM organizations WHERE id = $2) \
          ORDER BY nlevel(o.path) DESC LIMIT 1",
    )
    .bind(key)
    .bind(org_id)
    .fetch_optional(conn)
    .await?;
    Ok(row.and_then(|(d, a)| serde_json::from_value(d).ok().map(|d| (d, a))))
}

/// Problems with a workflow's action steps, as seen from `org_id`: an
/// integration that doesn't exist there, or an action it doesn't have
/// (under its own key or a renamed one's).
pub async fn check_actions(
    conn: &mut PgConnection,
    org_id: Uuid,
    workflow: &Workflow,
) -> sqlx::Result<Vec<Diagnostic>> {
    let mut problems = Vec::new();
    let mut cache: BTreeMap<String, Option<(Definition, Json)>> = BTreeMap::new();
    for used in action_uses(workflow) {
        if !cache.contains_key(&used.integration) {
            let found = current(&mut *conn, org_id, &used.integration).await?;
            cache.insert(used.integration.clone(), found);
        }
        let path = format!("{}.action", used.path);
        match &cache[&used.integration] {
            None => problems.push(Diagnostic {
                path,
                message: format!(
                    "there is no integration {:?} in this organization or above",
                    used.integration
                ),
            }),
            Some((definition, aliases)) => {
                let key = aliases[&used.action].as_str().unwrap_or(&used.action);
                if !definition.actions.iter().any(|a| a.key == key) {
                    problems.push(Diagnostic {
                        path,
                        message: format!(
                            "integration {:?} has no action {:?}",
                            used.integration, used.action
                        ),
                    });
                }
            }
        }
    }
    Ok(problems)
}

/// A published workflow an update would break.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AffectedWorkflow {
    pub workflow_id: Uuid,
    pub name: String,
    pub steps: Vec<AffectedStep>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AffectedStep {
    pub step_id: String,
    pub action: String,
    pub problem: String,
}

/// The published workflows (current versions, at or below the
/// integration's organization) whose steps call actions an update removes
/// or breaks. Renamed actions aren't listed: aliases keep them working.
pub async fn affected(
    conn: &mut PgConnection,
    integration_org: Uuid,
    integration_key: &str,
    aliases: &Json,
    diff: &DefinitionDiff,
) -> sqlx::Result<Vec<AffectedWorkflow>> {
    let mut broken: BTreeMap<&str, String> = BTreeMap::new();
    for key in &diff.removed {
        broken.insert(key, "the action is removed".into());
    }
    for action in &diff.changed {
        if let Some(change) = action.changes.iter().find(|c| c.breaking) {
            broken.insert(&action.key, change.message.clone());
        }
    }
    if broken.is_empty() {
        return Ok(Vec::new());
    }
    let rows: Vec<(Uuid, String, Json)> = sqlx::query_as(
        "SELECT w.id, w.name, v.document FROM workflows w \
           JOIN workflow_versions v ON v.workflow_id = w.id AND v.version = w.current_version \
           JOIN organizations o ON o.id = w.org_id \
          WHERE w.status = 'active' \
            AND o.path <@ (SELECT path FROM organizations WHERE id = $1) \
          ORDER BY lower(w.name), w.id",
    )
    .bind(integration_org)
    .fetch_all(conn)
    .await?;
    let mut affected = Vec::new();
    for (workflow_id, name, document) in rows {
        let Ok(workflow) = serde_json::from_value::<Workflow>(document) else {
            continue;
        };
        let steps: Vec<AffectedStep> = action_uses(&workflow)
            .into_iter()
            .filter(|u| u.integration == integration_key)
            .filter_map(|u| {
                // Steps may use a key that was renamed earlier.
                let key = aliases[&u.action].as_str().unwrap_or(&u.action);
                broken.get(key).map(|problem| AffectedStep {
                    step_id: u.step_id,
                    action: u.action,
                    problem: problem.clone(),
                })
            })
            .collect();
        if !steps.is_empty() {
            affected.push(AffectedWorkflow {
                workflow_id,
                name,
                steps,
            });
        }
    }
    Ok(affected)
}
