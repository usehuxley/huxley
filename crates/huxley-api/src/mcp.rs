//! The MCP server (design doc §10.2, ADR-030): Streamable HTTP at `/mcp`,
//! JSON-RPC 2.0 over POST, answered with single JSON responses.
//!
//! Every tool call is made *through the API itself*, as the authenticated
//! user, so it passes the same permission checks, validation and scoping
//! as any other API call, and is audited `via = mcp`. Tools only create
//! and change drafts: nothing publishes (a person does that).

use std::net::SocketAddr;

use axum::body::Body;
use axum::extract::{ConnectInfo, State};
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use http::{HeaderMap, HeaderValue, Method, Request, StatusCode, header};
use serde_json::{Value, json};
use tower::ServiceExt as _;

use crate::authz::Authed;
use crate::{ApiError, AppState};

/// The API router, for making tool calls through it.
#[derive(Clone)]
pub struct Internal(pub axum::Router);

impl std::fmt::Debug for Internal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Internal")
    }
}

/// Marks a request as made by an MCP tool call (only the server can set a
/// request extension; clients can't), so it's audited `via = mcp`.
#[derive(Debug, Clone, Copy)]
pub struct McpCall;

/// Protocol versions this server speaks, newest first.
const VERSIONS: &[&str] = &["2025-06-18", "2025-03-26", "2024-11-05"];

const INSTRUCTIONS: &str = "Huxley is an automation platform for MSPs. Workflows are HWF documents \
(YAML or JSON; read the huxley://schemas/hwf/v1.json resource or get_workflow_schema). Build one \
with validate_workflow, try it with test_workflow_document (mock steps that would reach real \
systems), save it with create_workflow_draft or update_workflow_draft, and follow runs with \
get_run. You can only save drafts: a person publishes them.";

fn rpc_result(id: &Value, result: &Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn rpc_error(id: &Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

const UUID: &str = r#"{ "type": "string", "format": "uuid" }"#;

/// The tools: name, description, and input properties (JSON) with the
/// required ones. Each becomes an object schema.
const TOOLS: &[(&str, &str, &str, &[&str])] = &[
    (
        "list_organizations",
        "Organizations you can see (ids are needed to create workflows).",
        "{}",
        &[],
    ),
    (
        "list_integrations",
        "Integrations (imported APIs) you can see.",
        "{}",
        &[],
    ),
    (
        "list_actions",
        "Integration actions an organization can use in integration.action steps, with their inputs.",
        r#"{ "org_id": UUID }"#,
        &["org_id"],
    ),
    (
        "get_integration",
        "An integration with its actions and their input schemas.",
        r#"{ "id": UUID }"#,
        &["id"],
    ),
    (
        "get_workflow_schema",
        "The HWF JSON Schema: every step type and field.",
        "{}",
        &[],
    ),
    ("list_workflows", "Workflows you can see.", "{}", &[]),
    (
        "get_workflow",
        "A workflow with its current draft and any problems with it.",
        r#"{ "id": UUID }"#,
        &["id"],
    ),
    (
        "validate_workflow",
        "Checks a workflow document without saving it; returns its problems (none: it's ready).",
        r#"{ "document": DOCUMENT }"#,
        &["document"],
    ),
    (
        "test_workflow_document",
        "Test-runs a document without saving it. Mock steps that would reach real systems with their outputs (by step id). \
      Returns the run's id: follow it with get_run.",
        r#"{ "org_id": UUID, "document": DOCUMENT, "inputs": { "type": "object" },
          "mocks": { "type": "object", "description": "Step id → the output it returns instead of running." } }"#,
        &["org_id", "document"],
    ),
    (
        "create_workflow_draft",
        "Creates a workflow (as a draft) in an organization. A person reviews and publishes it.",
        r#"{ "org_id": UUID, "document": DOCUMENT }"#,
        &["org_id", "document"],
    ),
    (
        "update_workflow_draft",
        "Replaces a workflow's draft. A person reviews and publishes it.",
        r#"{ "id": UUID, "document": DOCUMENT }"#,
        &["id", "document"],
    ),
    (
        "test_run_workflow",
        "Test-runs a saved workflow's draft for real (its steps run). Returns the run's id.",
        r#"{ "id": UUID, "inputs": { "type": "object" } }"#,
        &["id"],
    ),
    (
        "get_run",
        "A run: its status, steps (inputs, outputs, errors) and outputs.",
        r#"{ "id": UUID }"#,
        &["id"],
    ),
    (
        "list_runs",
        "Recent runs, optionally of one workflow.",
        r#"{ "workflow_id": UUID }"#,
        &[],
    ),
    (
        "list_scripts",
        "Library scripts workflows can run with script steps.",
        "{}",
        &[],
    ),
    ("list_forms", "Forms that start workflows.", "{}", &[]),
    (
        "list_datasets",
        "Datasets (typed tables kept in Huxley) and their columns.",
        "{}",
        &[],
    ),
    (
        "query_dataset",
        "Rows of a dataset matching equality filters.",
        r#"{ "id": UUID, "filters": { "type": "object" }, "limit": { "type": "integer", "minimum": 1, "maximum": 1000 } }"#,
        &["id"],
    ),
];

const DOCUMENT: &str =
    r#"{ "description": "An HWF workflow document: a JSON object, or YAML text." }"#;

type Tool = (
    &'static str,
    &'static str,
    &'static str,
    &'static [&'static str],
);

/// The service desk group (the `servicedesk` module).
const DESK_TOOLS: &[Tool] = &[
    (
        "find_tickets",
        "Tickets you can see, most urgent first: by words in the title or number, state (open, resolved, closed, all), \
         organization, queue, priority (e.g. p1,p2) or assignee (a user id, me, or none).",
        r#"{ "search": { "type": "string" }, "state": { "type": "string", "enum": ["open", "resolved", "closed", "all"] },
          "org_id": UUID, "queue_id": UUID, "priority": { "type": "string" }, "assignee": { "type": "string" },
          "limit": { "type": "integer", "minimum": 1, "maximum": 100 } }"#,
        &[],
    ),
    (
        "get_ticket",
        "A ticket: its fields, status, queue, assignee and SLA.",
        r#"{ "id": UUID }"#,
        &["id"],
    ),
    (
        "get_ticket_notes",
        "A ticket's notes and replies (public, internal and resolution), oldest first.",
        r#"{ "id": UUID }"#,
        &["id"],
    ),
    (
        "recommend_assignees",
        "Who should take a ticket, best first, with reasons: skills, availability, load, history, familiarity.",
        r#"{ "id": UUID }"#,
        &["id"],
    ),
    (
        "similar_tickets",
        "Tickets nearest in meaning to this one.",
        r#"{ "id": UUID }"#,
        &["id"],
    ),
    (
        "add_ticket_note",
        "Adds a note to a ticket: internal (staff only, the default) or public (emailed to the requester).",
        r#"{ "id": UUID, "body": { "type": "string" }, "kind": { "type": "string", "enum": ["internal", "public"] } }"#,
        &["id", "body"],
    ),
    (
        "update_ticket",
        "Changes a ticket's priority (p1 to p4), queue or assignee (a user id; null clears it).",
        r#"{ "id": UUID, "priority": { "type": "string", "enum": ["p1", "p2", "p3", "p4"] }, "queue_id": UUID,
          "assignee_user_id": { "type": ["string", "null"], "format": "uuid" } }"#,
        &["id"],
    ),
];

/// The clients group (the `crm` module).
const CLIENT_TOOLS: &[Tool] = &[
    (
        "list_contacts",
        "A client company's contacts.",
        r#"{ "org_id": UUID }"#,
        &["org_id"],
    ),
    (
        "get_client_health",
        "A client company's health score (0 to 100), its factors and the reasons (the cx module).",
        r#"{ "org_id": UUID }"#,
        &["org_id"],
    ),
];

/// The assets group (the `cmdb` module).
const ASSET_TOOLS: &[Tool] = &[
    (
        "find_assets",
        "Assets you can see: by words in the name, serial number, hostname, model or manufacturer, and organization.",
        r#"{ "q": { "type": "string" }, "org_id": UUID, "limit": { "type": "integer", "minimum": 1, "maximum": 100 } }"#,
        &[],
    ),
    (
        "get_asset",
        "An asset with its details, relationships and tickets.",
        r#"{ "id": UUID }"#,
        &["id"],
    ),
];

/// Tool groups and the module each needs.
const GROUPS: &[(&str, &[Tool])] = &[
    ("servicedesk", DESK_TOOLS),
    ("crm", CLIENT_TOOLS),
    ("cmdb", ASSET_TOOLS),
];

/// The tools while `modules` are on: the core ones and each group whose
/// module is.
fn tools_for(modules: &std::collections::BTreeSet<&str>) -> Vec<&'static Tool> {
    let mut all: Vec<&'static Tool> = TOOLS.iter().collect();
    for (module, group) in GROUPS {
        if modules.contains(module) {
            all.extend(group.iter());
        }
    }
    all
}

/// A tool's input schema.
fn schema(properties: &str, required: &[&str]) -> Value {
    let text = properties
        .replace("UUID", UUID)
        .replace("DOCUMENT", DOCUMENT);
    let properties: Value = serde_json::from_str(&text).expect("tool schemas are JSON");
    json!({ "type": "object", "properties": properties, "required": required, "additionalProperties": false })
}

/// The tools while `modules` are on, for a model: name, description and
/// input schema.
pub(crate) fn tool_defs(
    modules: &std::collections::BTreeSet<&str>,
) -> Vec<huxley_ai::chat::ToolDef> {
    tools_for(modules)
        .into_iter()
        .map(
            |(name, description, properties, required)| huxley_ai::chat::ToolDef {
                name: (*name).to_owned(),
                description: (*description).to_owned(),
                input_schema: schema(properties, required),
            },
        )
        .collect()
}

/// Tools that change something (a person approves each call in the
/// assistant); the rest only read.
pub(crate) const CHANGING: &[&str] = &[
    "add_ticket_note",
    "update_ticket",
    "create_workflow_draft",
    "update_workflow_draft",
    "test_workflow_document",
    "test_run_workflow",
];

/// A tool's answer as text for a model, and whether it failed.
pub(crate) fn result_text(outcome: Result<(StatusCode, Value), String>) -> (String, bool) {
    let result = tool_result(outcome);
    let text = result["content"][0]["text"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    (text, result["isError"] == true)
}

/// A published workflow offered as a tool (it has an `mcp_tool` trigger).
#[derive(Debug, Clone)]
pub(crate) struct WorkflowTool {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
    pub workflow_id: uuid::Uuid,
}

/// How long a workflow tool waits for its run before answering with the
/// run's id instead.
const WORKFLOW_WAIT: std::time::Duration = std::time::Duration::from_secs(25);

/// The published workflows with an `mcp_tool` trigger that `authed` may
/// run, as tools (named `run_<key>`; a suffix tells same-key ones apart).
pub(crate) async fn workflow_tools(state: &AppState, authed: &Authed) -> Vec<WorkflowTool> {
    type Row = (
        uuid::Uuid,
        String,
        String,
        Option<String>,
        String,
        Value,
        Option<Value>,
    );
    let rows: Vec<Row> = async {
        let mut tx = huxley_db::begin_scoped(&state.db, &huxley_db::OrgScope::System).await.ok()?;
        let rows = sqlx::query_as(
            "SELECT w.id, w.key, w.name, w.description, o.path::text, t.config, v.document -> 'spec' -> 'inputs' \
               FROM workflow_triggers t \
               JOIN workflows w ON w.id = t.workflow_id \
               JOIN organizations o ON o.id = w.org_id \
               JOIN workflow_versions v ON v.workflow_id = w.id AND v.version = w.current_version \
              WHERE t.type = 'mcp_tool' AND w.status = 'active' \
              ORDER BY w.key, w.id",
        )
        .fetch_all(&mut *tx)
        .await
        .ok()?;
        tx.commit().await.ok()?;
        Some(rows)
    }
    .await
    .unwrap_or_default();
    let mut tools: Vec<WorkflowTool> = Vec::new();
    for (id, key, name, description, path, config, inputs) in rows {
        if !authed
            .principal
            .can_at_path(huxley_authz::permissions::WORKFLOW_RUN, &path)
        {
            continue;
        }
        let base: String = format!("run_{}", key.replace('-', "_"))
            .chars()
            .take(56)
            .collect();
        let tool_name = if tools.iter().any(|t| t.name == base) {
            format!("{base}_{}", &id.simple().to_string()[24..])
        } else {
            base
        };
        let description = config["description"]
            .as_str()
            .map(str::to_owned)
            .or(description.filter(|d| !d.trim().is_empty()))
            .unwrap_or_else(|| format!("Runs the workflow {name}."));
        let input_schema = match inputs {
            Some(schema) if schema.is_object() => schema,
            _ => json!({ "type": "object", "properties": {} }),
        };
        tools.push(WorkflowTool {
            name: tool_name,
            description: format!("{description} (Runs the published workflow \"{name}\".)"),
            input_schema,
            workflow_id: id,
        });
    }
    tools
}

impl Caller<'_> {
    /// Runs a workflow tool: starts the published workflow with `args` as
    /// its inputs, and waits a while for it to finish.
    pub(crate) async fn run_workflow(
        &self,
        tool: &WorkflowTool,
        args: &Value,
    ) -> (StatusCode, Value) {
        let inputs = if args.is_object() {
            args.clone()
        } else {
            json!({})
        };
        let (status, started) = self
            .send(Call::post(
                format!("/api/v1/workflows/{}/runs", tool.workflow_id),
                json!({ "inputs": inputs }),
            ))
            .await;
        if !status.is_success() {
            return (status, started);
        }
        let run_id = started["id"].as_str().unwrap_or_default().to_owned();
        let deadline = std::time::Instant::now() + WORKFLOW_WAIT;
        loop {
            let (status, run) = self.send(Call::get(format!("/api/v1/runs/{run_id}"))).await;
            if !status.is_success() {
                return (status, run);
            }
            let state = run["status"].as_str().unwrap_or_default();
            if matches!(state, "succeeded" | "failed" | "cancelled") {
                return (
                    StatusCode::OK,
                    json!({ "runId": run_id, "status": state, "outputs": run["outputs"], "error": run["error"] }),
                );
            }
            if std::time::Instant::now() > deadline {
                return (
                    StatusCode::OK,
                    json!({ "runId": run_id, "status": state, "note": "Still running: follow it with get_run." }),
                );
            }
            tokio::time::sleep(std::time::Duration::from_millis(400)).await;
        }
    }
}

fn is_tool(name: &str) -> bool {
    TOOLS
        .iter()
        .chain(GROUPS.iter().flat_map(|(_, g)| g.iter()))
        .any(|(n, ..)| *n == name)
}

/// A tool's `document` argument: an object, or YAML/JSON text.
fn document(args: &Value) -> Result<Value, String> {
    match &args["document"] {
        Value::String(text) => serde_saphyr::from_str::<Value>(text)
            .map_err(|e| format!("document isn't valid YAML: {e}")),
        Value::Object(_) => Ok(args["document"].clone()),
        _ => Err("document is a workflow document (an object, or YAML text)".into()),
    }
}

fn arg<'a>(args: &'a Value, name: &str) -> Result<&'a str, String> {
    args[name]
        .as_str()
        .ok_or_else(|| format!("{name} is required"))
}

/// An API request a tool makes.
#[derive(Debug)]
struct Call {
    method: Method,
    path: String,
    body: Option<Value>,
}

impl Call {
    fn get(path: impl Into<String>) -> Self {
        Self {
            method: Method::GET,
            path: path.into(),
            body: None,
        }
    }
    fn post(path: impl Into<String>, body: Value) -> Self {
        Self {
            method: Method::POST,
            path: path.into(),
            body: Some(body),
        }
    }
}

/// The API request for a simple tool, or `None` for one that's handled
/// specially.
fn plan(name: &str, args: &Value) -> Result<Option<Call>, String> {
    Ok(Some(match name {
        "list_organizations" => Call::get("/api/v1/orgs"),
        "list_integrations" => Call::get("/api/v1/integrations"),
        "list_actions" => Call::get(format!(
            "/api/v1/integrations/actions?orgId={}",
            arg(args, "org_id")?
        )),
        "get_integration" => Call::get(format!("/api/v1/integrations/{}", arg(args, "id")?)),
        "list_workflows" => Call::get("/api/v1/workflows"),
        "get_workflow" => Call::get(format!("/api/v1/workflows/{}", arg(args, "id")?)),
        "validate_workflow" => Call::post(
            "/api/v1/workflows/validate",
            json!({ "document": document(args)? }),
        ),
        "test_workflow_document" => Call::post(
            "/api/v1/workflows/test-runs",
            json!({
                "orgId": arg(args, "org_id")?, "document": document(args)?,
                "inputs": args.get("inputs").cloned().unwrap_or_else(|| json!({})),
                "mocks": args.get("mocks").cloned().unwrap_or_else(|| json!({}))
            }),
        ),
        "create_workflow_draft" => Call::post(
            "/api/v1/workflows",
            json!({ "orgId": arg(args, "org_id")?, "document": document(args)? }),
        ),
        "test_run_workflow" => Call::post(
            format!("/api/v1/workflows/{}/runs", arg(args, "id")?),
            json!({ "inputs": args.get("inputs").cloned().unwrap_or_else(|| json!({})), "test": true }),
        ),
        "get_run" => Call::get(format!("/api/v1/runs/{}", arg(args, "id")?)),
        "list_runs" => match args["workflow_id"].as_str() {
            Some(id) => Call::get(format!("/api/v1/runs?workflowId={id}&limit=25")),
            None => Call::get("/api/v1/runs?limit=25"),
        },
        "list_scripts" => Call::get("/api/v1/scripts"),
        "list_forms" => Call::get("/api/v1/forms"),
        "list_datasets" => Call::get("/api/v1/datasets"),
        "query_dataset" => Call::post(
            format!("/api/v1/datasets/{}/rows/query", arg(args, "id")?),
            json!({
                "filters": args.get("filters").cloned().unwrap_or_else(|| json!({})),
                "limit": args.get("limit").cloned().unwrap_or(json!(100))
            }),
        ),
        "find_tickets" => {
            let mut query: Vec<String> = Vec::new();
            for (arg, param) in [
                ("search", "search"),
                ("state", "state"),
                ("org_id", "orgId"),
                ("queue_id", "queueId"),
                ("priority", "priority"),
                ("assignee", "assignee"),
            ] {
                if let Some(v) = args[arg].as_str() {
                    query.push(format!("{param}={}", urlencode(v)));
                }
            }
            query.push(format!(
                "limit={}",
                args["limit"].as_i64().unwrap_or(25).clamp(1, 100)
            ));
            Call::get(format!("/api/v1/tickets?{}", query.join("&")))
        }
        "get_ticket" => Call::get(format!("/api/v1/tickets/{}", arg(args, "id")?)),
        "get_ticket_notes" => Call::get(format!("/api/v1/tickets/{}/notes", arg(args, "id")?)),
        "recommend_assignees" => Call::get(format!(
            "/api/v1/tickets/{}/recommendations",
            arg(args, "id")?
        )),
        "similar_tickets" => Call::get(format!("/api/v1/tickets/{}/similar", arg(args, "id")?)),
        "add_ticket_note" => Call::post(
            format!("/api/v1/tickets/{}/notes", arg(args, "id")?),
            json!({ "body": arg(args, "body")?, "kind": args["kind"].as_str().unwrap_or("internal") }),
        ),
        "list_contacts" => Call::get(format!("/api/v1/orgs/{}/contacts", arg(args, "org_id")?)),
        "get_client_health" => Call::get(format!("/api/v1/orgs/{}/health", arg(args, "org_id")?)),
        "find_assets" => {
            let mut query = vec![format!(
                "limit={}",
                args["limit"].as_i64().unwrap_or(25).clamp(1, 100)
            )];
            if let Some(q) = args["q"].as_str() {
                query.push(format!("q={}", urlencode(q)));
            }
            if let Some(org) = args["org_id"].as_str() {
                query.push(format!("orgId={}", urlencode(org)));
            }
            Call::get(format!("/api/v1/assets?{}", query.join("&")))
        }
        "get_asset" => Call::get(format!("/api/v1/assets/{}", arg(args, "id")?)),
        "get_workflow_schema" | "update_workflow_draft" | "update_ticket" => return Ok(None),
        other => return Err(format!("no tool called {other:?}")),
    }))
}

/// Makes API calls as the MCP client's user.
pub(crate) struct Caller<'a> {
    internal: &'a axum::Router,
    /// The caller's own credentials: a bearer token, or a session cookie.
    credentials: Vec<(header::HeaderName, HeaderValue)>,
    peer: Option<ConnectInfo<SocketAddr>>,
    user_agent: Option<HeaderValue>,
}

impl<'a> Caller<'a> {
    /// Calls as whoever sent `headers` (their token or session cookie).
    pub(crate) fn new(
        internal: &'a axum::Router,
        headers: &HeaderMap,
        peer: Option<ConnectInfo<SocketAddr>>,
    ) -> Self {
        let credentials = [header::AUTHORIZATION, header::COOKIE]
            .into_iter()
            .filter_map(|name| headers.get(&name).cloned().map(|value| (name, value)))
            .collect();
        Self {
            internal,
            credentials,
            peer,
            user_agent: headers.get(header::USER_AGENT).cloned(),
        }
    }

    async fn send(&self, call: Call) -> (StatusCode, Value) {
        let mut request = Request::builder()
            .method(call.method)
            .uri(&call.path)
            // Session-cookie calls need the anti-forgery header, like any.
            .header(crate::csrf::HEADER, crate::csrf::VALUE);
        for (name, value) in &self.credentials {
            request = request.header(name, value.clone());
        }
        if let Some(agent) = &self.user_agent {
            request = request.header(header::USER_AGENT, agent.clone());
        }
        let mut request = match call.body {
            Some(body) => request
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body.to_string())),
            None => request.body(Body::empty()),
        }
        .expect("valid request");
        request.extensions_mut().insert(McpCall);
        if let Some(peer) = self.peer {
            request.extensions_mut().insert(peer);
        }
        let response = match self.internal.clone().oneshot(request).await {
            Ok(response) => response,
            Err(never) => match never {},
        };
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), 32 * 1024 * 1024)
            .await
            .unwrap_or_default();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }

    pub(crate) async fn call_tool(
        &self,
        name: &str,
        args: &Value,
    ) -> Result<(StatusCode, Value), String> {
        match name {
            "get_workflow_schema" => Ok((StatusCode::OK, huxley_hwf::json_schema())),
            "update_workflow_draft" => {
                let id = arg(args, "id")?;
                let document = document(args)?;
                // The draft's current revision, so the save isn't refused as stale.
                let (status, current) = self
                    .send(Call::get(format!("/api/v1/workflows/{id}")))
                    .await;
                if !status.is_success() {
                    return Ok((status, current));
                }
                Ok(self
                    .send(Call {
                        method: Method::PUT,
                        path: format!("/api/v1/workflows/{id}/draft"),
                        body: Some(
                            json!({ "revision": current["revision"], "document": document }),
                        ),
                    })
                    .await)
            }
            "update_ticket" => {
                let id = arg(args, "id")?;
                // The ticket's current version, so the change isn't refused as stale.
                let (status, current) = self.send(Call::get(format!("/api/v1/tickets/{id}"))).await;
                if !status.is_success() {
                    return Ok((status, current));
                }
                let mut changes = json!({ "version": current["version"] });
                for (arg_name, field) in [
                    ("priority", "priority"),
                    ("queue_id", "queueId"),
                    ("assignee_user_id", "assigneeUserId"),
                ] {
                    if let Some(v) = args.get(arg_name) {
                        changes[field] = v.clone();
                    }
                }
                Ok(self
                    .send(Call {
                        method: Method::PATCH,
                        path: format!("/api/v1/tickets/{id}"),
                        body: Some(changes),
                    })
                    .await)
            }
            _ => match plan(name, args)? {
                Some(call) => Ok(self.send(call).await),
                None => Err(format!("no tool called {name:?}")),
            },
        }
    }
}

/// A tool's result: the API's answer as structured content and text, or
/// its problem as an error the model can read.
fn tool_result(outcome: Result<(StatusCode, Value), String>) -> Value {
    match outcome {
        Ok((status, body)) if status.is_success() => {
            // Structured content is an object.
            let structured = if body.is_object() {
                body
            } else {
                json!({ "items": body })
            };
            json!({
                "content": [{ "type": "text", "text": serde_json::to_string(&structured).unwrap_or_default() }],
                "structuredContent": structured,
                "isError": false
            })
        }
        Ok((status, problem)) => {
            let detail = problem["errors"]
                .as_array()
                .filter(|e| !e.is_empty())
                .map(|errors| {
                    errors
                        .iter()
                        .map(|e| {
                            format!(
                                "{}: {}",
                                e["field"].as_str().unwrap_or(""),
                                e["message"].as_str().unwrap_or("")
                            )
                        })
                        .collect::<Vec<_>>()
                        .join("; ")
                })
                .or_else(|| problem["detail"].as_str().map(str::to_owned))
                .or_else(|| problem["title"].as_str().map(str::to_owned))
                .unwrap_or_else(|| status.to_string());
            json!({ "content": [{ "type": "text", "text": format!("{status}: {detail}") }], "isError": true })
        }
        Err(message) => {
            json!({ "content": [{ "type": "text", "text": message }], "isError": true })
        }
    }
}

const SCHEMA_URI: &str = "huxley://schemas/hwf/v1.json";

async fn handle(
    caller: &Caller<'_>,
    workflows: &[WorkflowTool],
    modules: &std::collections::BTreeSet<&str>,
    message: &Value,
) -> Option<Value> {
    let id = message.get("id")?.clone();
    let method = message["method"].as_str().unwrap_or_default();
    let params = &message["params"];
    Some(match method {
        "initialize" => {
            let asked = params["protocolVersion"].as_str().unwrap_or_default();
            let version = VERSIONS
                .iter()
                .find(|v| **v == asked)
                .unwrap_or(&VERSIONS[0]);
            rpc_result(
                &id,
                &json!({
                    "protocolVersion": version,
                    "capabilities": { "tools": { "listChanged": false }, "resources": { "listChanged": false } },
                    "serverInfo": { "name": "huxley", "version": env!("CARGO_PKG_VERSION") },
                    "instructions": INSTRUCTIONS
                }),
            )
        }
        "ping" => rpc_result(&id, &json!({})),
        "tools/list" => {
            let mut tools: Vec<Value> = tools_for(modules)
                .into_iter()
                .map(|(name, description, properties, required)| {
                    json!({ "name": name, "description": description, "inputSchema": schema(properties, required) })
                })
                .collect();
            tools.extend(
                workflows
                    .iter()
                    .map(|t| json!({ "name": t.name, "description": t.description, "inputSchema": t.input_schema })),
            );
            rpc_result(&id, &json!({ "tools": tools }))
        }
        "tools/call" => {
            let Some(name) = params["name"].as_str() else {
                return Some(rpc_error(&id, -32602, "params.name is required"));
            };
            let args = params
                .get("arguments")
                .cloned()
                .unwrap_or_else(|| json!({}));
            if let Some(tool) = workflows.iter().find(|t| t.name == name) {
                return Some(rpc_result(
                    &id,
                    &tool_result(Ok(caller.run_workflow(tool, &args).await)),
                ));
            }
            if !is_tool(name) {
                return Some(rpc_error(&id, -32602, &format!("no tool called {name:?}")));
            }
            let args = params
                .get("arguments")
                .cloned()
                .unwrap_or_else(|| json!({}));
            rpc_result(&id, &tool_result(caller.call_tool(name, &args).await))
        }
        "resources/list" => rpc_result(
            &id,
            &json!({ "resources": [{
                "uri": SCHEMA_URI, "name": "HWF JSON Schema", "mimeType": "application/schema+json",
                "description": "The schema of Huxley workflow documents: every step type and field."
            }] }),
        ),
        "resources/read" => {
            if params["uri"] != SCHEMA_URI {
                return Some(rpc_error(&id, -32002, "no such resource"));
            }
            rpc_result(
                &id,
                &json!({ "contents": [{
                    "uri": SCHEMA_URI, "mimeType": "application/schema+json",
                    "text": huxley_hwf::json_schema().to_string()
                }] }),
            )
        }
        _ => rpc_error(&id, -32601, &format!("method {method:?} isn't supported")),
    })
}

fn json_response(status: StatusCode, body: &Value) -> Response {
    (
        status,
        [(header::CONTENT_TYPE, "application/json")],
        body.to_string(),
    )
        .into_response()
}

/// `POST /mcp`: one JSON-RPC message (or a batch). Notifications and
/// responses get `202 Accepted`; requests get their response.
pub async fn post(
    State(state): State<AppState>,
    Extension(Internal(internal)): Extension<Internal>,
    authed: Result<Authed, ApiError>,
    peer: Option<Extension<ConnectInfo<SocketAddr>>>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    // No token: point the client at OAuth discovery (RFC 9728). A bad one:
    // the same, with `invalid_token`.
    let sent = crate::authz::bearer_token(&headers).is_some();
    if authed.is_err() || !sent {
        let mut response = json_response(
            StatusCode::UNAUTHORIZED,
            &json!({ "error": "invalid_token", "error_description": "Sign in with OAuth, or send an API token as Authorization: Bearer <token>." }),
        );
        response.headers_mut().insert(
            header::WWW_AUTHENTICATE,
            crate::oauth_routes::www_authenticate(&state, sent.then_some("invalid_token")),
        );
        return response;
    }
    let Ok(authed) = authed else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    let caller = Caller::new(&internal, &headers, peer.map(|Extension(p)| p));
    let workflows = workflow_tools(&state, &authed).await;
    let modules = enabled_modules(&state).await;
    match &body {
        Value::Array(messages) => {
            let mut out = Vec::new();
            for message in messages {
                if let Some(reply) = handle(&caller, &workflows, &modules, message).await {
                    out.push(reply);
                }
            }
            if out.is_empty() {
                StatusCode::ACCEPTED.into_response()
            } else {
                json_response(StatusCode::OK, &Value::Array(out))
            }
        }
        Value::Object(_) => match handle(&caller, &workflows, &modules, &body).await {
            Some(reply) => json_response(StatusCode::OK, &reply),
            None => StatusCode::ACCEPTED.into_response(),
        },
        _ => json_response(
            StatusCode::BAD_REQUEST,
            &rpc_error(&Value::Null, -32600, "not a JSON-RPC message"),
        ),
    }
}

/// The modules on for the instance (empty if they can't be read).
async fn enabled_modules(state: &AppState) -> std::collections::BTreeSet<&'static str> {
    let Ok(mut tx) = huxley_db::begin_scoped(&state.db, &huxley_db::OrgScope::System).await else {
        return std::collections::BTreeSet::new();
    };
    let set = huxley_modules::enabled_set(&mut tx)
        .await
        .unwrap_or_default();
    let _ = tx.commit().await;
    set
}

/// A query string value, percent-encoded.
fn urlencode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                char::from(b).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// `GET /mcp`: this server doesn't open server-to-client streams.
pub async fn get() -> Response {
    (StatusCode::METHOD_NOT_ALLOWED, [(header::ALLOW, "POST")]).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn documents_may_be_yaml_or_objects() {
        let yaml = json!({ "document": "apiVersion: usehuxley.io/v1\nkind: Workflow\n" });
        assert_eq!(document(&yaml).unwrap()["kind"], "Workflow");
        assert_eq!(
            document(&json!({ "document": { "kind": "Workflow" } })).unwrap()["kind"],
            "Workflow"
        );
        assert!(document(&json!({ "document": 3 })).is_err());
    }

    #[test]
    fn every_tool_has_a_plan_or_a_handler() {
        let grouped = GROUPS.iter().flat_map(|(_, g)| g.iter());
        for (name, _, properties, required) in TOOLS.iter().chain(grouped) {
            let args = json!({ "id": "01a20000-0000-7000-8000-000000000000", "org_id": "01a20000-0000-7000-8000-000000000000",
                               "document": { "kind": "Workflow" }, "body": "x" });
            let planned = plan(name, &args);
            assert!(planned.is_ok(), "{name}: {planned:?}");
            let schema = schema(properties, required);
            for r in *required {
                assert!(schema["properties"].get(r).is_some(), "{name}: {r}");
            }
        }
        // Nothing publishes.
        assert!(TOOLS.iter().all(|(name, ..)| !name.contains("publish")));
        // Groups appear with their module.
        let none = std::collections::BTreeSet::new();
        assert!(!tool_defs(&none).iter().any(|t| t.name == "find_tickets"));
        let desk: std::collections::BTreeSet<&str> = ["servicedesk"].into_iter().collect();
        assert!(tool_defs(&desk).iter().any(|t| t.name == "find_tickets"));
    }
}
