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
    // Poll triggers call a list action too.
    for (i, trigger) in workflow.spec.triggers.iter().enumerate() {
        if let huxley_hwf::Trigger::Poll(p) = trigger
            && let Some((integration, action)) = p.action.split_once('/')
        {
            out.push(ActionUse {
                path: format!("spec.triggers[{i}]"),
                step_id: p.id.clone().unwrap_or_else(|| "poll".into()),
                integration: integration.to_owned(),
                action: action.to_owned(),
            });
        }
    }
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

/// An action a workflow in some organization can call: for pickers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ActionOption {
    /// What a step's `action` says: `<integration>/<action>`.
    pub action: String,
    pub integration: String,
    pub integration_name: String,
    pub name: String,
    pub group: Option<String>,
    pub method: String,
    pub path: String,
    /// Input names, required ones first.
    pub inputs: Vec<String>,
    pub required: Vec<String>,
    pub deprecated: bool,
    pub description: Option<String>,
    /// The inputs' JSON Schema (asked for with `schemas`, for code generation).
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(value_type = Option<Object>)]
    pub input_schema: Option<Json>,
    /// The output's JSON Schema, when known (with `schemas`).
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(value_type = Option<Object>)]
    pub output_schema: Option<Json>,
}

/// The actions workflows in `org_id` can call: those of the nearest
/// integration with each key, in this organization or above.
pub async fn available_actions(
    conn: &mut PgConnection,
    org_id: Uuid,
) -> sqlx::Result<Vec<ActionOption>> {
    let rows: Vec<(String, String, Json, Json, Json)> = sqlx::query_as(
        "SELECT DISTINCT ON (i.key) i.key, i.name, v.definition, v.aliases, i.overlay \
           FROM integrations i \
           JOIN organizations o ON o.id = i.org_id \
           JOIN integration_versions v ON v.integration_id = i.id AND v.version = i.current_version \
          WHERE o.path @> (SELECT path FROM organizations WHERE id = $1) \
          ORDER BY i.key, nlevel(o.path) DESC",
    )
    .bind(org_id)
    .fetch_all(conn)
    .await?;
    let mut options = Vec::new();
    for (key, integration_name, definition, aliases, overlay) in rows {
        let Ok(mut definition) = serde_json::from_value::<Definition>(definition) else {
            continue;
        };
        crate::overlay::Overlay::from_json(&overlay).apply(&mut definition, &aliases);
        for action in definition.actions.into_iter().filter(|a| !a.hidden) {
            let required: Vec<String> = action.input_schema["required"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Json::as_str)
                .map(str::to_owned)
                .collect();
            let mut inputs = required.clone();
            for name in action.input_schema["properties"]
                .as_object()
                .into_iter()
                .flat_map(serde_json::Map::keys)
            {
                if !inputs.contains(name) {
                    inputs.push(name.clone());
                }
            }
            options.push(ActionOption {
                action: format!("{key}/{}", action.key),
                integration: key.clone(),
                integration_name: integration_name.clone(),
                name: action.name,
                group: action.group,
                method: action.method,
                path: action.path,
                inputs,
                required,
                deprecated: action.deprecated,
                description: action.description,
                input_schema: Some(action.input_schema),
                output_schema: action.output_schema,
            });
        }
    }
    Ok(options)
}
