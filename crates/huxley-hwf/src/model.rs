//! HWF v1 document types.
//!
//! Field names are `snake_case`, except `apiVersion`. Every struct rejects
//! unknown fields, so a typo is an error rather than a silently ignored
//! setting. Strings that may hold Jinja (`{{ … }}`) are plain `String`s or
//! JSON values here; the expression engine checks them.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A workflow document.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(title = "Huxley Workflow Format v1")]
pub struct Workflow {
    /// Always `usehuxley.com/v1`.
    #[serde(rename = "apiVersion")]
    pub api_version: String,
    pub kind: DocumentKind,
    pub metadata: Metadata,
    pub spec: Spec,
    /// Canvas layout (node positions, notes). Ignored by the engine, so code
    /// edits and canvas edits don't conflict.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ui: Option<Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum DocumentKind {
    Workflow,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Metadata {
    /// Stable identifier, unique per owning organization: lowercase letters,
    /// digits and hyphens.
    pub key: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Spec {
    /// A JSON Schema (type `object`) for the run's inputs. It also drives the
    /// generated form for manual runs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inputs: Option<Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub triggers: Vec<Trigger>,
    pub steps: Vec<Step>,
    /// The run's outputs, usually expressions over step outputs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outputs: Option<BTreeMap<String, Value>>,
    /// Steps run when the run fails and nothing caught the error. `error` is
    /// in scope.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub on_error: Vec<Step>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settings: Option<Settings>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    /// Fail the run if it takes longer than this, e.g. `1h`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout: Option<String>,
    /// At most this many runs of this workflow at once per organization.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub concurrency: Option<u32>,
}

// ------------------------------------------------------------------ triggers

/// What starts a run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Trigger {
    /// From the app, the API or the CLI.
    Manual(ManualTrigger),
    /// On a cron schedule or at an interval.
    Schedule(ScheduleTrigger),
    /// An HTTP request to the workflow's webhook URL.
    Webhook(WebhookTrigger),
    /// A domain event, such as `ticket.created`.
    Event(EventTrigger),
    /// New (or changed) items from an integration's list action, checked at
    /// an interval.
    Poll(PollTrigger),
    /// A form submission.
    Form(FormTrigger),
    /// Exposed as a tool to MCP clients and the assistant.
    McpTool(McpToolTrigger),
}

impl Trigger {
    pub fn id(&self) -> Option<&str> {
        match self {
            Self::Manual(t) => t.id.as_deref(),
            Self::Schedule(t) => t.id.as_deref(),
            Self::Webhook(t) => t.id.as_deref(),
            Self::Event(t) => t.id.as_deref(),
            Self::Poll(t) => t.id.as_deref(),
            Self::Form(t) => t.id.as_deref(),
            Self::McpTool(t) => t.id.as_deref(),
        }
    }

    pub const fn type_name(&self) -> &'static str {
        match self {
            Self::Manual(_) => "manual",
            Self::Schedule(_) => "schedule",
            Self::Webhook(_) => "webhook",
            Self::Event(_) => "event",
            Self::Poll(_) => "poll",
            Self::Form(_) => "form",
            Self::McpTool(_) => "mcp_tool",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ManualTrigger {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ScheduleTrigger {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// Five-field cron expression. Set this or `interval`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cron: Option<String>,
    /// e.g. `15m`. Set this or `cron`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interval: Option<String>,
    /// IANA time zone for `cron`; defaults to the organization's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timezone: Option<String>,
    /// Inputs for each scheduled run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inputs: Option<Value>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WebhookTrigger {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth: Option<WebhookAuth>,
    #[serde(default)]
    pub response: WebhookResponse,
    /// How long a `sync` webhook waits for the run, e.g. `30s` (at most 5m).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum WebhookResponse {
    /// `202 Accepted` with the run id, immediately.
    #[default]
    Async,
    /// Wait for the run and respond with its outputs.
    Sync,
}

/// How webhook callers prove who they are. Secrets are expressions, usually
/// a secret organization variable: `{{ vars.webhook_secret }}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum WebhookAuth {
    /// An HMAC of the body in a header.
    Hmac {
        secret: String,
        #[serde(default = "default_signature_header")]
        header: String,
        #[serde(default)]
        algorithm: HmacAlgorithm,
        /// Text before the hex digest, e.g. `sha256=`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        prefix: Option<String>,
        /// A signed timestamp header; requests older than 5 minutes are refused.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timestamp_header: Option<String>,
    },
    /// `Authorization: Bearer <token>`.
    Bearer { token: String },
    /// HTTP basic authentication.
    Basic { username: String, password: String },
}

fn default_signature_header() -> String {
    "X-Signature".into()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum HmacAlgorithm {
    #[default]
    Sha256,
    Sha512,
    Sha1,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EventTrigger {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// e.g. `ticket.created`.
    pub event: String,
    /// Start a run only when this expression is true. `event` is in scope.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filter: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PollTrigger {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// The list action: `<integration key>/<action key>`.
    pub action: String,
    /// A connection key; default: the nearest one, as for action steps.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connection: Option<String>,
    /// The action's inputs, passed as they are (not expressions).
    #[serde(default, rename = "with", skip_serializing_if = "Option::is_none")]
    pub inputs: Option<Value>,
    /// How often to check, e.g. `5m` (at least `1m`).
    pub interval: String,
    /// Where the list is in the response, as a dotted path (e.g. `value`);
    /// by default the response is the list.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub items: Option<String>,
    /// The field that identifies an item (dotted path; default `id`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item_id: Option<String>,
    /// Start runs for new items only (default), or for changed ones too.
    #[serde(default)]
    pub on: PollOn,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PollOn {
    #[default]
    New,
    NewOrChanged,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FormTrigger {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// The form's key.
    pub form: String,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct McpToolTrigger {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// What the tool does, for the model. Defaults to the workflow's description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

// --------------------------------------------------------------------- steps

/// A step: the fields every step has, plus its type's own.
///
/// Control-flow steps (`if`, `switch`, `foreach`, `parallel`, `try`) contain
/// nested step lists; there are no free-form edges (ADR-020).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type")]
pub enum Step {
    /// Evaluate an expression into a value.
    #[serde(rename = "transform")]
    Transform(TransformStep),
    /// Set run-scoped variables (`run.vars`).
    #[serde(rename = "set_var")]
    SetVar(SetVarStep),
    /// Make an HTTP request.
    #[serde(rename = "http.request")]
    HttpRequest(HttpRequestStep),
    /// Call an action of an imported integration through a connection.
    #[serde(rename = "integration.action")]
    IntegrationAction(IntegrationActionStep),
    /// Run `then` or `else` depending on a condition.
    #[serde(rename = "if")]
    If(IfStep),
    /// Run the first case whose `when` equals `value`.
    #[serde(rename = "switch")]
    Switch(SwitchStep),
    /// Run steps for each item of a list.
    #[serde(rename = "foreach")]
    Foreach(ForeachStep),
    /// Run branches at the same time.
    #[serde(rename = "parallel")]
    Parallel(ParallelStep),
    /// Handle errors from a block of steps.
    #[serde(rename = "try")]
    Try(TryStep),
    /// Pause for a duration or until a time.
    #[serde(rename = "wait")]
    Wait(WaitStep),
    /// Run another workflow.
    #[serde(rename = "subworkflow")]
    Subworkflow(SubworkflowStep),
    /// Park until an external system calls a single-use URL.
    #[serde(rename = "wait_for_callback")]
    WaitForCallback(WaitForCallbackStep),
    /// Park until one of the approvers approves or rejects.
    #[serde(rename = "approval")]
    Approval(ApprovalStep),
    /// Run a library script in a sandboxed runner.
    #[serde(rename = "script")]
    Script(ScriptStep),
    /// End the run successfully.
    #[serde(rename = "return")]
    Return(ReturnStep),
    /// End the run with an error.
    #[serde(rename = "fail")]
    Fail(FailStep),
    /// Send a notification to people.
    #[serde(rename = "core.notify")]
    Notify(NotifyStep),
}

/// Declares a step struct with the common fields first.
macro_rules! step {
    ($(#[$meta:meta])* $name:ident { $($(#[$fmeta:meta])* pub $field:ident : $ty:ty,)* }) => {
        $(#[$meta])*
        #[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
        #[serde(deny_unknown_fields)]
        pub struct $name {
            /// Unique in the workflow: lowercase letters, digits, underscores.
            /// Other steps read this step's output as `steps.<id>.output`.
            pub id: String,
            #[serde(default, skip_serializing_if = "Option::is_none")]
            pub name: Option<String>,
            #[serde(default, skip_serializing_if = "Option::is_none")]
            pub description: Option<String>,
            /// Skip the step unless this expression is true.
            #[serde(default, rename = "if", skip_serializing_if = "Option::is_none")]
            pub when: Option<String>,
            #[serde(default, skip_serializing_if = "Option::is_none")]
            pub retry: Option<RetryPolicy>,
            /// Fail the attempt after this long, e.g. `30s`.
            #[serde(default, skip_serializing_if = "Option::is_none")]
            pub timeout: Option<String>,
            /// Record the error as the step's output and carry on.
            #[serde(default, skip_serializing_if = "std::ops::Not::not")]
            pub continue_on_error: bool,
            $($(#[$fmeta])* pub $field: $ty,)*
        }
    };
}

step!(TransformStep {
    /// Any JSON value; strings may be expressions.
    pub output: Value,
});

step!(SetVarStep {
    pub vars: BTreeMap<String, Value>,
});

step!(HttpRequestStep {
    #[serde(rename = "with")]
    pub request: HttpRequest,
});

step!(IfStep {
    pub condition: String,
    #[serde(default)]
    pub then: Vec<Step>,
    #[serde(default, rename = "else", skip_serializing_if = "Vec::is_empty")]
    pub otherwise: Vec<Step>,
});

step!(SwitchStep {
    /// The expression to match.
    pub value: String,
    pub cases: Vec<SwitchCase>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub default: Vec<Step>,
});

step!(ForeachStep {
    /// An expression that evaluates to a list.
    pub items: String,
    /// The loop variable's name (default `item`); `loop.index` is also set.
    #[serde(default, rename = "as", skip_serializing_if = "Option::is_none")]
    pub item_name: Option<String>,
    /// How many items run at once (default 1, at most 50).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub concurrency: Option<u32>,
    pub steps: Vec<Step>,
});

step!(ParallelStep {
    pub branches: Vec<Branch>,
    #[serde(default)]
    pub join: Join,
});

step!(TryStep {
    pub steps: Vec<Step>,
    /// Run if a step in `steps` fails; `error` is in scope.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub catch: Vec<Step>,
    /// Run afterwards either way.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub finally: Vec<Step>,
});

step!(WaitStep {
    /// e.g. `10m`. Set this or `until`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration: Option<String>,
    /// An RFC 3339 timestamp, usually an expression. Set this or `duration`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub until: Option<String>,
});

step!(IntegrationActionStep {
    /// `<integration key>/<action key>`, e.g. `tickets-api/createTicket`.
    pub action: String,
    /// A connection key. Default: the nearest connection to the
    /// integration that the run's organization can use.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connection: Option<String>,
    /// The action's inputs: its parameters by name, and `body`.
    #[serde(default, rename = "with", skip_serializing_if = "Option::is_none")]
    pub inputs: Option<Value>,
});

step!(WaitForCallbackStep {
    /// How long the URL works, e.g. `2h` (default `24h`). After that the
    /// step fails with `kind: timeout`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires: Option<String>,
    /// Tells the other system where to call back, once the URL exists:
    /// `callback.url` is in scope here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub send: Option<CallbackSend>,
});

/// How a `wait_for_callback` step hands its URL over: an integration action
/// or a plain HTTP request (set one).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CallbackSend {
    /// `<integration key>/<action key>`, as in `integration.action`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connection: Option<String>,
    /// The action's inputs.
    #[serde(default, rename = "with", skip_serializing_if = "Option::is_none")]
    pub inputs: Option<Value>,
    /// Or an HTTP request, as in `http.request`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request: Option<HttpRequest>,
}

step!(ScriptStep {
    /// The library script's key (this organization's or the nearest one
    /// above that has it).
    pub script: String,
    /// Its parameters.
    #[serde(default, rename = "with", skip_serializing_if = "Option::is_none")]
    pub inputs: Option<Value>,
});

step!(ApprovalStep {
    /// What's being decided; shown to approvers.
    pub title: String,
    /// More detail for approvers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// Who may decide: `user:<id>`, `team:<id>`, `role:<key>` (holders at the
    /// run's organization). The first decision counts.
    pub approvers: Value,
    /// How long approvers have, e.g. `3d` (default `7d`). After that the
    /// step fails with `kind: timeout`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires: Option<String>,
});

step!(SubworkflowStep {
    /// The other workflow's key.
    pub workflow: String,
    /// Its inputs.
    #[serde(default, rename = "with", skip_serializing_if = "Option::is_none")]
    pub inputs: Option<Value>,
    /// Wait for it and use its outputs (default), or start it and carry on.
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub wait: bool,
});

step!(ReturnStep {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outputs: Option<BTreeMap<String, Value>>,
});

step!(FailStep {
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
});

step!(NotifyStep {
    #[serde(rename = "with")]
    pub notification: Notification,
});

const fn yes() -> bool {
    true
}

#[allow(clippy::trivially_copy_pass_by_ref)] // serde's skip_serializing_if signature
const fn is_true(value: &bool) -> bool {
    *value
}

impl Step {
    /// The fields every step has.
    pub fn common(&self) -> Common<'_> {
        macro_rules! common {
            ($($variant:ident),*) => {
                match self {
                    $(Self::$variant(s) => Common {
                        id: &s.id,
                        when: s.when.as_deref(),
                        retry: s.retry.as_ref(),
                        timeout: s.timeout.as_deref(),
                        continue_on_error: s.continue_on_error,
                    },)*
                }
            };
        }
        common!(
            Transform,
            SetVar,
            HttpRequest,
            IntegrationAction,
            If,
            Switch,
            Foreach,
            Parallel,
            Try,
            Wait,
            Subworkflow,
            WaitForCallback,
            Approval,
            Script,
            Return,
            Fail,
            Notify
        )
    }

    pub fn id(&self) -> &str {
        self.common().id
    }

    /// The `type` as written in HWF.
    pub const fn type_name(&self) -> &'static str {
        match self {
            Self::Transform(_) => "transform",
            Self::SetVar(_) => "set_var",
            Self::HttpRequest(_) => "http.request",
            Self::IntegrationAction(_) => "integration.action",
            Self::If(_) => "if",
            Self::Switch(_) => "switch",
            Self::Foreach(_) => "foreach",
            Self::Parallel(_) => "parallel",
            Self::Try(_) => "try",
            Self::Wait(_) => "wait",
            Self::Subworkflow(_) => "subworkflow",
            Self::WaitForCallback(_) => "wait_for_callback",
            Self::Approval(_) => "approval",
            Self::Script(_) => "script",
            Self::Return(_) => "return",
            Self::Fail(_) => "fail",
            Self::Notify(_) => "core.notify",
        }
    }

    /// Nested step lists, labelled by field (`then`, `cases[0]`, `branches[1]`…).
    pub fn blocks(&self) -> Vec<(String, &[Step])> {
        match self {
            Self::If(s) => vec![
                ("then".into(), &s.then[..]),
                ("else".into(), &s.otherwise[..]),
            ],
            Self::Switch(s) => {
                let mut blocks: Vec<(String, &[Step])> = s
                    .cases
                    .iter()
                    .enumerate()
                    .map(|(i, c)| (format!("cases[{i}].steps"), &c.steps[..]))
                    .collect();
                blocks.push(("default".into(), &s.default[..]));
                blocks
            }
            Self::Foreach(s) => vec![("steps".into(), &s.steps[..])],
            Self::Parallel(s) => s
                .branches
                .iter()
                .enumerate()
                .map(|(i, b)| (format!("branches[{i}].steps"), &b.steps[..]))
                .collect(),
            Self::Try(s) => vec![
                ("steps".into(), &s.steps[..]),
                ("catch".into(), &s.catch[..]),
                ("finally".into(), &s.finally[..]),
            ],
            _ => Vec::new(),
        }
    }

    /// Whether control never continues past this step in its block.
    pub const fn ends_block(&self) -> bool {
        match self {
            Self::Return(_) => true,
            Self::Fail(s) => !s.continue_on_error,
            _ => false,
        }
    }
}

/// The fields every step has, borrowed.
#[derive(Debug, Clone, Copy)]
pub struct Common<'a> {
    pub id: &'a str,
    pub when: Option<&'a str>,
    pub retry: Option<&'a RetryPolicy>,
    pub timeout: Option<&'a str>,
    pub continue_on_error: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SwitchCase {
    /// Compared for equality with the switch's `value`.
    pub when: Value,
    pub steps: Vec<Step>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Branch {
    /// Unique within the step.
    pub id: String,
    pub steps: Vec<Step>,
}

/// When a `parallel` step finishes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Join {
    /// When every branch has finished (default). Any failure fails the step.
    #[default]
    All,
    /// When the first branch succeeds; the others are cancelled.
    Any,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RetryPolicy {
    /// Attempts in total, including the first (2 to 20).
    pub max_attempts: u32,
    #[serde(default)]
    pub backoff: Backoff,
    /// Delay before the first retry (default `1s`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delay: Option<String>,
    /// Longest delay between attempts (default `5m`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_delay: Option<String>,
    /// Which failures to retry: HTTP statuses (`429`), classes (`5xx`), or
    /// `timeout` / `network`. Empty retries every failure.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub on: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Backoff {
    Fixed,
    #[default]
    Exponential,
    /// Exponential with full jitter.
    Jitter,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HttpRequest {
    #[serde(default)]
    pub method: HttpMethod,
    pub url: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub headers: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub query: BTreeMap<String, Value>,
    /// Sent as JSON, unless `body_type` says otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<Value>,
    #[serde(default)]
    pub body_type: BodyType,
    /// Treat 4xx/5xx responses as success (they fail the step by default).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub allow_error_status: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "UPPERCASE")]
pub enum HttpMethod {
    #[default]
    Get,
    Post,
    Put,
    Patch,
    Delete,
    Head,
    Options,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum BodyType {
    #[default]
    Json,
    /// `application/x-www-form-urlencoded` from an object.
    Form,
    /// A string, sent as is.
    Text,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Notification {
    /// Who: `user:<id>`, `team:<id>`, `role:<key>`, or a list of those.
    pub to: Value,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// A link in the app.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}
