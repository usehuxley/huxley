//! The built-in assistant (design doc §10.3): a conversation with the
//! organization's AI provider that can use Huxley's tools (the MCP server's,
//! called through the API as the user). Tools that only read run at once;
//! tools that change something wait for the user to approve or deny.

use std::collections::BTreeMap;

use huxley_ai::chat::{Message, ToolCall, ToolResult};
use huxley_ai::{AiError, Purpose};
use huxley_db::{OrgScope, begin_scoped};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::PgConnection;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::mcp::{CHANGING, Caller, WorkflowTool, result_text, tool_defs};
use crate::{ApiError, ApiResult, AppState};

/// Most model turns one message may lead to.
const MAX_TURNS: usize = 12;

/// What a streamed answer reports as it goes (see `assistant_routes`).
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Progress {
    /// Text the assistant wrote.
    Text { text: String },
    /// One assistant turn ended (text after this belongs to the next).
    TurnEnded,
    /// A tool call started.
    ToolStarted { id: String, name: String },
    /// A tool call finished.
    ToolFinished {
        id: String,
        #[serde(rename = "isError")]
        is_error: bool,
    },
    /// The conversation as it now stands. The last event.
    Done { conversation: Box<Conversation> },
    /// Answering failed. The last event.
    Failed { problem: Box<ApiError> },
}

/// Where progress goes, when the answer is streamed.
pub type Sink = Option<tokio::sync::mpsc::UnboundedSender<Progress>>;

fn report(sink: &Sink, progress: Progress) {
    if let Some(sink) = sink {
        // The receiver is gone if the browser left; the work carries on.
        let _ = sink.send(progress);
    }
}

const SYSTEM: &str = "You are Huxley's assistant, inside Huxley, an automation platform for MSPs. You help \
the signed-in person build and debug workflows and answer questions about their organizations, \
integrations, workflows, runs, scripts, forms and datasets, using the tools you have. Everything you do \
runs with that person's permissions. Workflows are HWF documents: get_workflow_schema describes every \
step. Validate documents before saving them, test them with test_workflow_document (mocking steps that \
would reach real systems) when that helps, and save only drafts: a person publishes them. Tools that \
change something need the person's approval, so say what you're about to do and why. Be concise, and \
say plainly when something failed.";

fn db_err(e: impl std::fmt::Display) -> ApiError {
    ApiError::internal(&e)
}

/// A tool call as the user sees it.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ShownCall {
    pub id: String,
    pub name: String,
    #[schema(value_type = std::collections::HashMap<String, serde_json::Value>)]
    pub input: Value,
    /// Waits for the user's approval.
    pub needs_approval: bool,
}

/// A message as the user sees it.
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ShownMessage {
    /// `user`, `assistant` or `tool`.
    pub role: String,
    pub text: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ShownCall>,
    /// For `tool`: the call it answers, and whether it failed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub call_id: Option<String>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub is_error: bool,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Conversation {
    pub id: Uuid,
    pub org_id: Uuid,
    pub title: String,
    pub messages: Vec<ShownMessage>,
    /// Calls waiting for the user's approval (approve or deny each).
    pub pending: Vec<ShownCall>,
}

#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct ConversationSummary {
    pub id: Uuid,
    pub org_id: Uuid,
    pub title: String,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: time::OffsetDateTime,
}

/// Calls of the last turn waiting for the user, and results already in.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Pending {
    calls: Vec<ToolCall>,
    results: Vec<ToolResult>,
}

/// Whether a call changes something (and so waits for the person): the
/// changing tools, and every workflow tool (it runs a workflow).
fn changes(name: &str) -> bool {
    CHANGING.contains(&name) || name.starts_with("run_")
}

fn shown_call(call: &ToolCall) -> ShownCall {
    ShownCall {
        id: call.id.clone(),
        name: call.name.clone(),
        input: call.input.clone(),
        needs_approval: changes(&call.name),
    }
}

fn shown(messages: &[Message]) -> Vec<ShownMessage> {
    let mut out = Vec::new();
    for message in messages {
        match message {
            Message::User { text } => out.push(ShownMessage {
                role: "user".into(),
                text: text.clone(),
                tool_calls: Vec::new(),
                call_id: None,
                is_error: false,
            }),
            Message::Assistant {
                text, tool_calls, ..
            } => out.push(ShownMessage {
                role: "assistant".into(),
                text: text.clone(),
                tool_calls: tool_calls.iter().map(shown_call).collect(),
                call_id: None,
                is_error: false,
            }),
            Message::ToolResults { results } => out.extend(results.iter().map(|r| ShownMessage {
                role: "tool".into(),
                text: r.content.chars().take(4000).collect(),
                tool_calls: Vec::new(),
                call_id: Some(r.id.clone()),
                is_error: r.is_error,
            })),
        }
    }
    out
}

/// The conversation `id` of `user_id`, its messages and pending calls.
async fn load(
    conn: &mut PgConnection,
    user_id: Uuid,
    id: Uuid,
) -> ApiResult<(Uuid, String, Vec<Message>, Pending)> {
    let row: Option<(Uuid, String, Option<Value>)> = sqlx::query_as(
        "SELECT org_id, title, pending FROM assistant_conversations WHERE id = $1 AND user_id = $2",
    )
    .bind(id)
    .bind(user_id)
    .fetch_optional(&mut *conn)
    .await
    .map_err(db_err)?;
    let (org_id, title, pending) = row.ok_or_else(ApiError::not_found)?;
    let raw: Vec<Value> = sqlx::query_scalar(
        "SELECT message FROM assistant_messages WHERE conversation_id = $1 ORDER BY seq",
    )
    .bind(id)
    .fetch_all(&mut *conn)
    .await
    .map_err(db_err)?;
    let messages = raw
        .into_iter()
        .filter_map(|m| serde_json::from_value(m).ok())
        .collect();
    let pending = pending
        .and_then(|p| serde_json::from_value(p).ok())
        .unwrap_or_default();
    Ok((org_id, title, messages, pending))
}

/// The conversation as the user sees it.
pub async fn view(state: &AppState, user_id: Uuid, id: Uuid) -> ApiResult<Conversation> {
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    let (org_id, title, messages, pending) = load(&mut tx, user_id, id).await?;
    tx.commit().await.map_err(db_err)?;
    let answered: Vec<&str> = pending.results.iter().map(|r| r.id.as_str()).collect();
    Ok(Conversation {
        id,
        org_id,
        title,
        messages: shown(&messages),
        pending: pending
            .calls
            .iter()
            .filter(|c| !answered.contains(&c.id.as_str()))
            .map(shown_call)
            .collect(),
    })
}

/// Starts a conversation for `user_id` in `org_id`.
pub async fn create(state: &AppState, user_id: Uuid, org_id: Uuid, title: &str) -> ApiResult<Uuid> {
    let id = huxley_core::new_id();
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    sqlx::query(
        "INSERT INTO assistant_conversations (id, user_id, org_id, title) VALUES ($1, $2, $3, $4)",
    )
    .bind(id)
    .bind(user_id)
    .bind(org_id)
    .bind(title)
    .execute(&mut *tx)
    .await
    .map_err(db_err)?;
    tx.commit().await.map_err(db_err)?;
    Ok(id)
}

async fn append(state: &AppState, id: Uuid, org_id: Uuid, message: &Message) -> ApiResult<()> {
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    sqlx::query(
        "INSERT INTO assistant_messages (conversation_id, seq, org_id, message) \
         VALUES ($1, (SELECT coalesce(max(seq), 0) + 1 FROM assistant_messages WHERE conversation_id = $1), $2, $3)",
    )
    .bind(id)
    .bind(org_id)
    .bind(serde_json::to_value(message).unwrap_or_default())
    .execute(&mut *tx)
    .await
    .map_err(db_err)?;
    sqlx::query("UPDATE assistant_conversations SET updated_at = now() WHERE id = $1")
        .bind(id)
        .execute(&mut *tx)
        .await
        .map_err(db_err)?;
    tx.commit().await.map_err(db_err)?;
    Ok(())
}

async fn set_pending(state: &AppState, id: Uuid, pending: Option<&Pending>) -> ApiResult<()> {
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    sqlx::query("UPDATE assistant_conversations SET pending = $2 WHERE id = $1")
        .bind(id)
        .bind(pending.map(|p| serde_json::to_value(p).unwrap_or_default()))
        .execute(&mut *tx)
        .await
        .map_err(db_err)?;
    tx.commit().await.map_err(db_err)?;
    Ok(())
}

/// Runs `call` as the user.
async fn run_call(
    caller: &Caller<'_>,
    workflows: &[WorkflowTool],
    call: &ToolCall,
    sink: &Sink,
) -> ToolResult {
    report(
        sink,
        Progress::ToolStarted {
            id: call.id.clone(),
            name: call.name.clone(),
        },
    );
    let outcome = match workflows.iter().find(|t| t.name == call.name) {
        Some(tool) => Ok(caller.run_workflow(tool, &call.input).await),
        None => caller.call_tool(&call.name, &call.input).await,
    };
    let (content, is_error) = result_text(outcome);
    // Keep what goes back to the model (and into the history) bounded.
    let content = if content.chars().count() > 30_000 {
        format!(
            "{}… (cut off)",
            content.chars().take(30_000).collect::<String>()
        )
    } else {
        content
    };
    report(
        sink,
        Progress::ToolFinished {
            id: call.id.clone(),
            is_error,
        },
    );
    ToolResult {
        id: call.id.clone(),
        content,
        is_error,
    }
}

/// Lets the model work until it answers, or until a call needs approval.
async fn advance(
    state: &AppState,
    caller: &Caller<'_>,
    workflows: &[WorkflowTool],
    id: Uuid,
    org_id: Uuid,
    mut messages: Vec<Message>,
    sink: &Sink,
) -> ApiResult<()> {
    let on_text = sink.clone().map(|sink| {
        move |text: &str| {
            let _ = sink.send(Progress::Text {
                text: text.to_owned(),
            });
        }
    });
    let mut tools = tool_defs();
    tools.extend(workflows.iter().map(|t| huxley_ai::chat::ToolDef {
        name: t.name.clone(),
        description: t.description.clone(),
        input_schema: t.input_schema.clone(),
    }));
    for _ in 0..MAX_TURNS {
        let turn = match state
            .engine
            .ai()
            .turn(
                org_id,
                Purpose {
                    feature: "assistant",
                    run_id: None,
                },
                Some(SYSTEM),
                &messages,
                &tools,
                on_text.as_ref().map(|f| f as &(dyn Fn(&str) + Send + Sync)),
            )
            .await
        {
            Ok(turn) => turn,
            Err(e @ AiError::Internal(_)) => return Err(db_err(e)),
            Err(e) => {
                return Err(ApiError::new(
                    http::StatusCode::BAD_GATEWAY,
                    "ai-failed",
                    "The AI provider couldn't answer",
                )
                .with_detail(e.to_string()));
            }
        };
        append(state, id, org_id, &turn.message).await?;
        report(sink, Progress::TurnEnded);
        let calls = turn.tool_calls().to_vec();
        messages.push(turn.message);
        if calls.is_empty() {
            return Ok(());
        }
        // Read-only calls run now; if any call changes something, all
        // wait for the user (results go back to the model together).
        let mut results = Vec::new();
        for call in calls.iter().filter(|c| !changes(&c.name)) {
            results.push(run_call(caller, workflows, call, sink).await);
        }
        if calls.iter().any(|c| changes(&c.name)) {
            return set_pending(state, id, Some(&Pending { calls, results })).await;
        }
        let message = Message::ToolResults { results };
        append(state, id, org_id, &message).await?;
        messages.push(message);
    }
    let note = Message::Assistant {
        text: "I stopped after many steps without finishing. Tell me how to continue.".into(),
        tool_calls: Vec::new(),
        raw: json!([{ "type": "text", "text": "I stopped after many steps without finishing." }]),
    };
    append(state, id, org_id, &note).await
}

/// Adds the user's message and lets the assistant answer.
pub async fn say(
    state: &AppState,
    caller: &Caller<'_>,
    workflows: &[WorkflowTool],
    user_id: Uuid,
    id: Uuid,
    text: &str,
    sink: &Sink,
) -> ApiResult<()> {
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    let (org_id, _, mut messages, pending) = load(&mut tx, user_id, id).await?;
    tx.commit().await.map_err(db_err)?;
    if !pending.calls.is_empty() {
        return Err(ApiError::conflict(
            "approve or deny the waiting actions first",
        ));
    }
    let message = Message::User {
        text: text.to_owned(),
    };
    append(state, id, org_id, &message).await?;
    messages.push(message);
    advance(state, caller, workflows, id, org_id, messages, sink).await
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Decisions {
    /// Tool call id → approve (true) or deny (false).
    pub decisions: BTreeMap<String, bool>,
}

/// Applies the user's decisions on waiting calls, then lets the
/// assistant carry on.
pub async fn decide(
    state: &AppState,
    caller: &Caller<'_>,
    workflows: &[WorkflowTool],
    user_id: Uuid,
    id: Uuid,
    decisions: &Decisions,
    sink: &Sink,
) -> ApiResult<()> {
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    let (org_id, _, mut messages, pending) = load(&mut tx, user_id, id).await?;
    tx.commit().await.map_err(db_err)?;
    if pending.calls.is_empty() {
        return Err(ApiError::conflict("nothing is waiting for approval"));
    }
    let mut results = pending.results.clone();
    for call in pending.calls.iter().filter(|c| changes(&c.name)) {
        let result = match decisions.decisions.get(&call.id) {
            Some(true) => run_call(caller, workflows, call, sink).await,
            Some(false) => ToolResult {
                id: call.id.clone(),
                content: "The person declined this action.".into(),
                is_error: true,
            },
            None => {
                return Err(ApiError::validation()
                    .with_field_error("decisions", format!("decide on {}", call.id)));
            }
        };
        results.push(result);
    }
    // In the order the model asked.
    results.sort_by_key(|r| pending.calls.iter().position(|c| c.id == r.id));
    set_pending(state, id, None).await?;
    let message = Message::ToolResults { results };
    append(state, id, org_id, &message).await?;
    messages.push(message);
    advance(state, caller, workflows, id, org_id, messages, sink).await
}

/// `user_id`'s conversations, newest first.
pub async fn list(state: &AppState, user_id: Uuid) -> ApiResult<Vec<ConversationSummary>> {
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    let rows = sqlx::query_as(
        "SELECT id, org_id, title, updated_at FROM assistant_conversations WHERE user_id = $1 \
          ORDER BY updated_at DESC LIMIT 100",
    )
    .bind(user_id)
    .fetch_all(&mut *tx)
    .await
    .map_err(db_err)?;
    tx.commit().await.map_err(db_err)?;
    Ok(rows)
}

/// Deletes one of `user_id`'s conversations.
pub async fn delete(state: &AppState, user_id: Uuid, id: Uuid) -> ApiResult<()> {
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    let done = sqlx::query("DELETE FROM assistant_conversations WHERE id = $1 AND user_id = $2")
        .bind(id)
        .bind(user_id)
        .execute(&mut *tx)
        .await
        .map_err(db_err)?;
    tx.commit().await.map_err(db_err)?;
    if done.rows_affected() == 0 {
        return Err(ApiError::not_found());
    }
    Ok(())
}
