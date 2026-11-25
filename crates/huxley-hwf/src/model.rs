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
    /// Always `usehuxley.io/v1`.
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
    /// Send an email to any address, through the instance's mail server.
    #[serde(rename = "core.email")]
    Email(EmailStep),
    /// Read rows of a dataset (the run's organization's and those below).
    #[serde(rename = "dataset.query")]
    DatasetQuery(DatasetQueryStep),
    /// Write rows to a dataset.
    #[serde(rename = "dataset.upsert")]
    DatasetUpsert(DatasetUpsertStep),
    /// Find contacts by email or external id (the `crm` module).
    #[serde(rename = "core.contact.find")]
    ContactFind(ContactFindStep),
    /// Create a contact, or update the one with the same email or external
    /// id (the `crm` module).
    #[serde(rename = "core.contact.upsert")]
    ContactUpsert(ContactUpsertStep),
    /// Read an organization's company details and sites (the `crm` module).
    #[serde(rename = "core.company.get")]
    CompanyGet(CompanyGetStep),
    /// Change an organization's company details (the `crm` module).
    #[serde(rename = "core.company.update")]
    CompanyUpdate(CompanyUpdateStep),
    /// Turn a received email into ticket work: a new ticket, or a reply on
    /// the ticket it belongs to (the `servicedesk` module).
    #[serde(rename = "core.ticket.from_email")]
    TicketFromEmail(TicketFromEmailStep),
    /// Open a ticket (the `servicedesk` module).
    #[serde(rename = "core.ticket.create")]
    TicketCreate(TicketCreateStep),
    /// Read a ticket, and optionally its notes (the `servicedesk` module).
    #[serde(rename = "core.ticket.get")]
    TicketGet(TicketGetStep),
    /// Find tickets (the `servicedesk` module).
    #[serde(rename = "core.ticket.find")]
    TicketFind(TicketFindStep),
    /// Change a ticket's fields or status (the `servicedesk` module).
    #[serde(rename = "core.ticket.update")]
    TicketUpdate(TicketUpdateStep),
    /// Add a note to a ticket (the `servicedesk` module).
    #[serde(rename = "core.ticket.note")]
    TicketNote(TicketNoteStep),
    /// Assign a ticket by its queue's dispatch strategy (the `servicedesk`
    /// module).
    #[serde(rename = "core.ticket.dispatch")]
    TicketDispatch(TicketDispatchStep),
    /// Fetch new mail from IMAP and Microsoft 365 mailboxes (the `servicedesk`
    /// module).
    #[serde(rename = "core.mail.fetch")]
    MailFetch(MailFetchStep),
    /// Email a public note to the ticket's requester (the `servicedesk`
    /// module).
    #[serde(rename = "core.ticket.email_reply")]
    TicketEmailReply(TicketEmailReplyStep),
    /// Ask the organization's AI provider; the answer is text.
    #[serde(rename = "ai.prompt")]
    AiPrompt(AiPromptStep),
    /// Have the AI pick one of a list of labels for some text.
    #[serde(rename = "ai.classify")]
    AiClassify(AiClassifyStep),
    /// Have the AI pull structured data (a JSON Schema) out of some text.
    #[serde(rename = "ai.extract")]
    AiExtract(AiExtractStep),
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
    /// Follow every page (the action's pagination, set in the integration's
    /// overlay): the output is `{items, pages, truncated}`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub all_pages: bool,
    /// The most pages followed with `all_pages` (default 20, at most 100).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_pages: Option<u32>,
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
    /// Environment variables for the script (names in upper case), e.g.
    /// from variables or secrets. Secret values are scrubbed from its
    /// output and logs.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, Value>,
    /// Connections whose credentials the script gets as environment
    /// variables: `NAME: <integration>` or `NAME: <integration>/<connection>`
    /// gives `NAME_URL` and `NAME_TOKEN` (OAuth 2.0, bearer), `NAME_KEY`
    /// (API key) or `NAME_USERNAME` and `NAME_PASSWORD` (basic), fetched
    /// when the script starts and scrubbed from its output and logs.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub connections: BTreeMap<String, String>,
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

step!(EmailStep {
    #[serde(rename = "with")]
    pub email: EmailMessage,
});

step!(DatasetQueryStep {
    #[serde(rename = "with")]
    pub query: DatasetQuery,
});

step!(DatasetUpsertStep {
    #[serde(rename = "with")]
    pub upsert: DatasetUpsert,
});

step!(ContactFindStep {
    #[serde(rename = "with")]
    pub find: ContactFind,
});

step!(ContactUpsertStep {
    #[serde(rename = "with")]
    pub contact: Box<ContactUpsert>,
});

step!(CompanyGetStep {
    #[serde(default, rename = "with")]
    pub company: CompanyRef,
});

step!(CompanyUpdateStep {
    #[serde(rename = "with")]
    pub company: Box<CompanyUpdate>,
});

step!(TicketFromEmailStep {
    #[serde(rename = "with")]
    pub email: TicketFromEmail,
});

step!(TicketEmailReplyStep {
    #[serde(rename = "with")]
    pub reply: TicketEmailReply,
});

step!(TicketCreateStep {
    #[serde(rename = "with")]
    pub ticket: TicketCreate,
});

step!(TicketGetStep {
    #[serde(rename = "with")]
    pub ticket: TicketGet,
});

step!(TicketFindStep {
    #[serde(rename = "with")]
    pub ticket: TicketFind,
});

step!(TicketUpdateStep {
    #[serde(rename = "with")]
    pub ticket: TicketUpdate,
});

step!(TicketNoteStep {
    #[serde(rename = "with")]
    pub ticket: TicketNote,
});

step!(TicketDispatchStep {
    #[serde(rename = "with")]
    pub ticket: TicketDispatch,
});

step!(MailFetchStep {
    #[serde(default, rename = "with")]
    pub fetch: MailFetch,
});

step!(AiPromptStep {
    #[serde(rename = "with")]
    pub ai: AiPrompt,
});

step!(AiClassifyStep {
    #[serde(rename = "with")]
    pub ai: AiClassify,
});

step!(AiExtractStep {
    #[serde(rename = "with")]
    pub ai: AiExtract,
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
            Notify,
            Email,
            DatasetQuery,
            DatasetUpsert,
            ContactFind,
            ContactUpsert,
            CompanyGet,
            CompanyUpdate,
            TicketFromEmail,
            MailFetch,
            TicketEmailReply,
            TicketCreate,
            TicketGet,
            TicketFind,
            TicketUpdate,
            TicketNote,
            TicketDispatch,
            AiPrompt,
            AiClassify,
            AiExtract
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
            Self::Email(_) => "core.email",
            Self::DatasetQuery(_) => "dataset.query",
            Self::DatasetUpsert(_) => "dataset.upsert",
            Self::ContactFind(_) => "core.contact.find",
            Self::ContactUpsert(_) => "core.contact.upsert",
            Self::CompanyGet(_) => "core.company.get",
            Self::CompanyUpdate(_) => "core.company.update",
            Self::TicketFromEmail(_) => "core.ticket.from_email",
            Self::MailFetch(_) => "core.mail.fetch",
            Self::TicketCreate(_) => "core.ticket.create",
            Self::TicketGet(_) => "core.ticket.get",
            Self::TicketFind(_) => "core.ticket.find",
            Self::TicketUpdate(_) => "core.ticket.update",
            Self::TicketNote(_) => "core.ticket.note",
            Self::TicketDispatch(_) => "core.ticket.dispatch",
            Self::TicketEmailReply(_) => "core.ticket.email_reply",
            Self::AiPrompt(_) => "ai.prompt",
            Self::AiClassify(_) => "ai.classify",
            Self::AiExtract(_) => "ai.extract",
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

/// Rows to read (`dataset.query`). Output: `{ rows, total }`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DatasetQuery {
    /// The dataset's key: the run's organization's, or the nearest above.
    pub dataset: String,
    /// Column values rows must equal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filters: Option<Value>,
    /// Only this organization's rows (it must be the run's or below it).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub org_id: Option<String>,
    /// A column to sort by; `-name` sorts descending.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub order_by: Option<String>,
    /// At most this many rows (default 100, at most 1000).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<Value>,
}

/// Rows to write (`dataset.upsert`). Output: `{ written }`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DatasetUpsert {
    pub dataset: String,
    /// A list of rows (objects of column values), usually an expression.
    pub rows: Value,
    /// The organization the rows belong to (default: the run's).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub org_id: Option<String>,
    /// Replace all of that organization's rows (a full refresh).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replace: Option<Value>,
    /// Build each row from an item of `rows`: column name → dot path into
    /// the item (e.g. `upn: userPrincipalName`, `city: address.city`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub map: Option<Value>,
}

/// A received email to turn into ticket work (`core.ticket.from_email`):
/// skipped if it's automatic mail or a loop, added as a reply to the ticket
/// it belongs to (by its `[HX#…]` subject token or the messages it answers;
/// a resolved ticket reopens), or opened as a new ticket for its sender, per
/// its mailbox's rules. Its attachments go on the ticket. Output:
/// `{ outcome, detail, ticket_id, created }`; `outcome` is `ticket_created`,
/// `note_added`, `ticket_reopened`, `ignored_automatic`, `ignored_loop` or
/// `ignored_unknown_sender`. Running it again for the same email changes
/// nothing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TicketFromEmail {
    /// The email's id (from `email.received`).
    pub email_id: Value,
}

/// A public note to email to its ticket's requester
/// (`core.ticket.email_reply`), from the mailbox the ticket's mail came to
/// (else the nearest one at its organization or above). The subject carries
/// the ticket's `[HX#…]` token and the message continues the email thread,
/// so the requester's answer comes back to the ticket. Notes that aren't
/// for the requester (internal, written by them, already sent) are skipped.
/// Output: `{ sent: true, to, message_id }` or `{ sent: false, reason }`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TicketEmailReply {
    /// The note's id (from `ticket.note_added`).
    pub note_id: Value,
}

/// A ticket to open (`core.ticket.create`): at `org_id` (default: the run's
/// organization), with `title` and optionally `description`, `type` (a
/// ticket type's key, default `incident`), `impact` and `urgency` (1 high …
/// 3 low), `priority` (`p1`…`p4`, instead of the matrix), `queue_id`,
/// `requester_id` (a contact there), `assignee_user_id`,
/// `assignee_team_id` and `source` (default `workflow`). The SLA applies as
/// usual. Output: `{ ticket }`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TicketCreate {
    pub title: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub org_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<Value>,
    #[serde(default, rename = "type", skip_serializing_if = "Option::is_none")]
    pub ticket_type: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub impact: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub urgency: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub priority: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub queue_id: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requester_id: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assignee_user_id: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assignee_team_id: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<Value>,
}

/// A ticket to read (`core.ticket.get`), with its notes if `notes` is
/// true. Output: `{ ticket, notes }`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TicketGet {
    pub ticket_id: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<Value>,
}

/// Tickets to find (`core.ticket.find`) among the run's organization and
/// those below it (or `org_id`'s): by `queue_id`, `assignee` (a person's
/// id, or `none`), `state` (`open`, the default, `resolved`, `closed` or
/// `all`), `search` (title or number) and `priority` (e.g. `p1,p2`), most
/// urgent first, at most
/// `limit` (default 50). Output: `{ tickets, total }`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TicketFind {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub org_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub queue_id: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assignee: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub search: Option<Value>,
    /// Only these priorities, comma-separated: e.g. `p1,p2`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub priority: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<Value>,
}

/// Changes to a ticket (`core.ticket.update`): any of `title`,
/// `description`, `impact`, `urgency`, `priority` (or `auto`), and
/// `queue_id`, `requester_id`, `assignee_user_id`, `assignee_team_id`
/// (`null` clears); then `status` (a status's key), with `resolution` where
/// the status needs one. Applied to the ticket as it is now. Output:
/// `{ ticket }`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TicketUpdate {
    pub ticket_id: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub impact: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub urgency: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub priority: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub queue_id: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requester_id: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assignee_user_id: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assignee_team_id: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolution: Option<Value>,
}

/// A note to add (`core.ticket.note`): `body`, and `kind`: `internal` (the
/// default), `public` or `resolution`. Output: `{ note }`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TicketNote {
    pub ticket_id: Value,
    pub body: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<Value>,
}

/// A ticket to assign (`core.ticket.dispatch`) by its queue's strategy
/// (`round_robin`, `load` or `skills`), or `strategy` to use another. With
/// the `workforce` module on, people missing a required certification, away
/// or outside their hours are skipped. A ticket that's already assigned is
/// left alone unless `reassign` is true. Output: `{ assigned, assignee,
/// strategy, reason, considered }`; `reason` says why nobody was chosen
/// (`assigned`, `no_queue`, `manual`, `nobody_available`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TicketDispatch {
    pub ticket_id: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub strategy: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reassign: Option<Value>,
}

/// Mail to fetch (`core.mail.fetch`): new messages from the enabled IMAP
/// and Microsoft 365 mailboxes at the run's organization and below, or just `mailbox_id`, up
/// to `limit` each (default 50). Each new message is kept and announced with
/// `email.received`. A mailbox that can't be reached is reported (and shown
/// on it) without failing the step. Output: `{ mailboxes: [{ mailbox_id,
/// address, fetched, new } | { mailbox_id, address, error }] }`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MailFetch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mailbox_id: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<Value>,
}

/// Contacts to find (`core.contact.find`): by `email`, by `external_id`
/// (e.g. `{ entra: "<object id>" }`), or both. Among the run's organization
/// and those below it, or `org_id` and those below it. Output:
/// `{ contacts, total }`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ContactFind {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<Value>,
    /// One external id: `{ <system>: <id> }`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub external_id: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub org_id: Option<String>,
}

/// A contact to create or update (`core.contact.upsert`). It's matched by
/// `match` (`email`, the default, or an external id's system, e.g.
/// `entra`) among the contacts of `org_id` (default: the run's
/// organization); a match is updated with the fields given, otherwise a
/// contact is created there. Output: `{ contact, created }`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ContactUpsert {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub org_id: Option<String>,
    /// What identifies the contact: `email` or an external id's system.
    #[serde(default, rename = "match", skip_serializing_if = "Option::is_none")]
    pub match_on: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_name: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_name: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phone: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mobile: Option<Value>,
    /// At the contact's primary company.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub job_title: Option<Value>,
    /// Identities elsewhere, merged into the contact's: `{ entra: "…" }`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub external_ids: Option<Value>,
}

/// Which organization (`core.company.get`): `org_id`, default the run's.
/// Output: `{ company, sites }`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CompanyRef {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub org_id: Option<String>,
}

/// Company details to change (`core.company.update`); fields left out stay
/// as they are. Output: `{ company }`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CompanyUpdate {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub org_id: Option<String>,
    /// `client`, `vendor`, `partner` or `prospect`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relationship: Option<Value>,
    /// `active`, `inactive`, `onboarding` or `offboarding`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub website: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phone: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<Value>,
}

/// A prompt (`ai.prompt`). Output: `{ text, model }`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AiPrompt {
    pub prompt: String,
    /// Instructions for the model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system: Option<String>,
    /// Another model than the provider's (e.g. a faster one).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<Value>,
}

/// Picking a label (`ai.classify`). Output: `{ label, model }`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AiClassify {
    /// What to classify.
    pub text: String,
    /// The labels to choose from (a list, or an expression giving one).
    pub labels: Value,
    /// What the labels mean, or how to choose.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

/// Pulling out data (`ai.extract`). Output: the data, as the schema says.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AiExtract {
    /// What to extract from.
    pub text: String,
    /// A JSON Schema (an object) of what to extract.
    pub schema: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

/// An email to send (`core.email`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EmailMessage {
    /// An address or a list of them (at most 20); each gets their own copy.
    pub to: Value,
    pub subject: String,
    /// The plain-text body.
    pub text: String,
    /// An HTML body, sent alongside the text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub html: Option<String>,
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
    /// The kind of notification, for people's channel preferences:
    /// `workflow.notification` (the default), `ticket.assigned`,
    /// `ticket.replied` or `ticket.sla`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// People not to notify (user ids), e.g. whoever made the change.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exclude: Option<Value>,
}
