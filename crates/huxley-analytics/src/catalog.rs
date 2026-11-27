//! The sources a query can ask about (ADR-041). Each is a curated view of
//! one kind of record: its `FROM` (always joining the owning organization
//! as `o`), the module and permission that govern it, and its fields.

use std::borrow::Cow;

use serde::Serialize;
use utoipa::ToSchema;

/// Text that's usually written in the catalog, or made for a dataset.
pub type Text = Cow<'static, str>;

/// What a field holds, which decides the operators, measures and time
/// buckets it allows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Text,
    Number,
    Bool,
    /// A calendar date.
    Date,
    /// A moment (timestamptz).
    Time,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = AnalyticsField)]
pub struct Field {
    #[schema(value_type = String)]
    pub key: Text,
    #[schema(value_type = String)]
    pub label: Text,
    pub kind: Kind,
    #[serde(skip)]
    pub sql: Text,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = AnalyticsSource)]
pub struct Source {
    #[schema(value_type = String)]
    pub key: Text,
    #[schema(value_type = String)]
    pub label: Text,
    #[schema(value_type = String)]
    pub description: Text,
    /// The module it belongs to, if any.
    pub module: Option<&'static str>,
    /// The permission that reads it.
    pub permission: &'static str,
    /// The time field most queries bucket by.
    #[schema(value_type = String)]
    pub time_field: Text,
    #[schema(value_type = Vec<Field>)]
    pub fields: Cow<'static, [Field]>,
    #[serde(skip)]
    pub from: Text,
}

impl Source {
    pub fn field(&self, key: &str) -> Option<&Field> {
        self.fields.iter().find(|f| f.key == key)
    }

    /// Whether it's a dataset's (made at run time, not in the catalog).
    pub fn is_dataset(key: &str) -> bool {
        key.starts_with(DATASET_PREFIX)
    }
}

const fn f(key: &'static str, label: &'static str, kind: Kind, sql: &'static str) -> Field {
    Field {
        key: Cow::Borrowed(key),
        label: Cow::Borrowed(label),
        kind,
        sql: Cow::Borrowed(sql),
    }
}

const CLIENT: Field = f("client", "Client", Kind::Text, "o.name");

pub const TICKETS: Source = Source {
    key: Cow::Borrowed("tickets"),
    label: Cow::Borrowed("Tickets"),
    description: Cow::Borrowed(
        "Service desk tickets, with their status, queue, assignee and SLA outcome.",
    ),
    module: Some("servicedesk"),
    permission: "ticket:read",
    time_field: Cow::Borrowed("created_at"),
    from: Cow::Borrowed(
        "sd_tickets t JOIN organizations o ON o.id = t.org_id \
        LEFT JOIN sd_statuses s ON s.id = t.status_id LEFT JOIN sd_queues q ON q.id = t.queue_id \
        LEFT JOIN sd_ticket_types ty ON ty.id = t.type_id LEFT JOIN users au ON au.id = t.assignee_user_id \
        LEFT JOIN teams tm ON tm.id = t.assignee_team_id LEFT JOIN sd_ticket_sla sla ON sla.ticket_id = t.id",
    ),
    fields: Cow::Borrowed(&[
        CLIENT,
        f("status", "Status", Kind::Text, "s.name"),
        f(
            "status_category",
            "Status category",
            Kind::Text,
            "s.category",
        ),
        f(
            "open",
            "Open",
            Kind::Bool,
            "(s.category NOT IN ('resolved', 'closed'))",
        ),
        f("priority", "Priority", Kind::Text, "t.priority"),
        f("type", "Type", Kind::Text, "ty.name"),
        f("queue", "Queue", Kind::Text, "q.name"),
        f("assignee", "Assignee", Kind::Text, "au.display_name"),
        f("team", "Team", Kind::Text, "tm.name"),
        f("source", "Source", Kind::Text, "t.source"),
        f("created_at", "Created", Kind::Time, "t.created_at"),
        f(
            "first_response_at",
            "First response",
            Kind::Time,
            "t.first_response_at",
        ),
        f("resolved_at", "Resolved", Kind::Time, "t.resolved_at"),
        f("closed_at", "Closed", Kind::Time, "t.closed_at"),
        f(
            "response_minutes",
            "Minutes to first response",
            Kind::Number,
            "(extract(epoch FROM t.first_response_at - t.created_at) / 60)",
        ),
        f(
            "resolution_hours",
            "Hours to resolve",
            Kind::Number,
            "(extract(epoch FROM t.resolved_at - t.created_at) / 3600)",
        ),
        f(
            "response_met",
            "Response SLA met",
            Kind::Bool,
            "sla.response_met",
        ),
        f(
            "resolution_met",
            "Resolution SLA met",
            Kind::Bool,
            "sla.resolution_met",
        ),
    ]),
};

pub const TIME_ENTRIES: Source = Source {
    key: Cow::Borrowed("time_entries"),
    label: Cow::Borrowed("Time entries"),
    description: Cow::Borrowed(
        "Time logged on tickets: who, for which client, what kind of work, and whether it's billable.",
    ),
    module: Some("servicedesk"),
    permission: "ticket:read",
    time_field: Cow::Borrowed("worked_on"),
    from: Cow::Borrowed(
        "sd_time_entries e JOIN organizations o ON o.id = e.org_id LEFT JOIN users u ON u.id = e.user_id \
        LEFT JOIN sd_work_types w ON w.id = e.work_type_id LEFT JOIN sd_tickets t ON t.id = e.ticket_id",
    ),
    fields: Cow::Borrowed(&[
        CLIENT,
        f("technician", "Technician", Kind::Text, "u.display_name"),
        f("work_type", "Work type", Kind::Text, "w.name"),
        f("worked_on", "Worked on", Kind::Date, "e.worked_on"),
        f("hours", "Hours", Kind::Number, "(e.minutes / 60.0)"),
        f("minutes", "Minutes", Kind::Number, "e.minutes"),
        f("billable", "Billable", Kind::Bool, "e.billable"),
        f(
            "ticket",
            "Ticket",
            Kind::Text,
            "(t.number_prefix || '-' || t.number)",
        ),
    ]),
};

pub const ASSETS: Source = Source {
    key: Cow::Borrowed("assets"),
    label: Cow::Borrowed("Assets"),
    description: Cow::Borrowed(
        "Configuration items: devices and other assets, with their type, status and warranty.",
    ),
    module: Some("cmdb"),
    permission: "asset:read",
    time_field: Cow::Borrowed("created_at"),
    from: Cow::Borrowed(
        "assets a JOIN organizations o ON o.id = a.org_id LEFT JOIN cmdb_asset_types ty ON ty.id = a.type_id \
        LEFT JOIN crm_sites si ON si.id = a.site_id",
    ),
    fields: Cow::Borrowed(&[
        CLIENT,
        f("type", "Type", Kind::Text, "ty.name"),
        f("status", "Status", Kind::Text, "a.status"),
        f(
            "manufacturer",
            "Manufacturer",
            Kind::Text,
            "nullif(a.manufacturer, '')",
        ),
        f("model", "Model", Kind::Text, "nullif(a.model, '')"),
        f("site", "Site", Kind::Text, "si.name"),
        f("purchased_on", "Purchased", Kind::Date, "a.purchased_on"),
        f(
            "warranty_ends_on",
            "Warranty ends",
            Kind::Date,
            "a.warranty_ends_on",
        ),
        f(
            "in_warranty",
            "In warranty",
            Kind::Bool,
            "(a.warranty_ends_on >= current_date)",
        ),
        f("created_at", "Added", Kind::Time, "a.created_at"),
    ]),
};

pub const RUNS: Source = Source {
    key: Cow::Borrowed("runs"),
    label: Cow::Borrowed("Workflow runs"),
    description: Cow::Borrowed(
        "Workflow runs: which workflow, how it was triggered, how it ended and how long it took.",
    ),
    module: None,
    permission: "workflow:read",
    time_field: Cow::Borrowed("created_at"),
    from: Cow::Borrowed(
        "runs r JOIN organizations o ON o.id = r.org_id LEFT JOIN workflows wf ON wf.id = r.workflow_id",
    ),
    fields: Cow::Borrowed(&[
        f("organization", "Organization", Kind::Text, "o.name"),
        f("workflow", "Workflow", Kind::Text, "wf.name"),
        f("status", "Status", Kind::Text, "r.status::text"),
        f("trigger", "Trigger", Kind::Text, "r.trigger_type"),
        f("created_at", "Started", Kind::Time, "r.created_at"),
        f("finished_at", "Finished", Kind::Time, "r.finished_at"),
        f(
            "duration_seconds",
            "Seconds taken",
            Kind::Number,
            "(extract(epoch FROM r.finished_at - r.started_at))",
        ),
    ]),
};

pub const CSAT: Source = Source {
    key: Cow::Borrowed("csat"),
    label: Cow::Borrowed("Satisfaction surveys"),
    description: Cow::Borrowed(
        "Surveys sent when tickets were resolved, and their answers: ratings, comments, who handled the ticket.",
    ),
    module: Some("cx"),
    permission: "ticket:read",
    time_field: Cow::Borrowed("sent_at"),
    from: Cow::Borrowed(
        "cx_surveys v JOIN organizations o ON o.id = v.org_id JOIN sd_tickets t ON t.id = v.ticket_id \
         LEFT JOIN users u ON u.id = v.assignee_user_id LEFT JOIN sd_queues q ON q.id = t.queue_id",
    ),
    fields: Cow::Borrowed(&[
        CLIENT,
        f("technician", "Technician", Kind::Text, "u.display_name"),
        f("queue", "Queue", Kind::Text, "q.name"),
        f(
            "answered",
            "Answered",
            Kind::Bool,
            "(v.responded_at IS NOT NULL)",
        ),
        f("rating", "Rating", Kind::Number, "v.rating"),
        f(
            "satisfaction",
            "Satisfaction %",
            Kind::Number,
            "(CASE WHEN v.rating IS NULL THEN NULL WHEN (v.scale = 'thumbs' AND v.rating = 5) OR (v.scale = 'five' AND v.rating >= 4) THEN 100 ELSE 0 END)",
        ),
        f("channel", "Answered by", Kind::Text, "v.channel"),
        f("sent_at", "Sent", Kind::Time, "v.sent_at"),
        f("responded_at", "Answered", Kind::Time, "v.responded_at"),
    ]),
};

pub const NPS: Source = Source {
    key: Cow::Borrowed("nps"),
    label: Cow::Borrowed("Net promoter answers"),
    description: Cow::Borrowed(
        "Net promoter questions sent to contacts, and their answers (0 to 10).",
    ),
    module: Some("cx"),
    permission: "crm:read",
    time_field: Cow::Borrowed("sent_at"),
    from: Cow::Borrowed(
        "cx_nps n JOIN organizations o ON o.id = n.org_id JOIN crm_contacts c ON c.id = n.contact_id",
    ),
    fields: Cow::Borrowed(&[
        CLIENT,
        f(
            "contact",
            "Contact",
            Kind::Text,
            "btrim(c.first_name || ' ' || c.last_name)",
        ),
        f("campaign", "Round", Kind::Text, "n.campaign"),
        f(
            "answered",
            "Answered",
            Kind::Bool,
            "(n.responded_at IS NOT NULL)",
        ),
        f("score", "Score", Kind::Number, "n.score"),
        f(
            "category",
            "Promoter, passive or detractor",
            Kind::Text,
            "(CASE WHEN n.score IS NULL THEN NULL WHEN n.score >= 9 THEN 'promoter' WHEN n.score >= 7 THEN 'passive' ELSE 'detractor' END)",
        ),
        f(
            "nps",
            "NPS",
            Kind::Number,
            "(CASE WHEN n.score IS NULL THEN NULL WHEN n.score >= 9 THEN 100 WHEN n.score >= 7 THEN 0 ELSE -100 END)",
        ),
        f("sent_at", "Asked", Kind::Time, "n.sent_at"),
        f("responded_at", "Answered", Kind::Time, "n.responded_at"),
    ]),
};

pub const HEALTH: Source = Source {
    key: Cow::Borrowed("health"),
    label: Cow::Borrowed("Client health"),
    description: Cow::Borrowed("Each client's daily health score, 0 to 100."),
    module: Some("cx"),
    permission: "crm:read",
    time_field: Cow::Borrowed("computed_on"),
    from: Cow::Borrowed("cx_health h JOIN organizations o ON o.id = h.org_id"),
    fields: Cow::Borrowed(&[
        CLIENT,
        f("score", "Health score", Kind::Number, "h.score"),
        f("computed_on", "Day", Kind::Date, "h.computed_on"),
    ]),
};

pub const SENTIMENT: Source = Source {
    key: Cow::Borrowed("sentiment"),
    label: Cow::Borrowed("Client sentiment"),
    description: Cow::Borrowed(
        "The tone of each judged client message, -2 (very negative) to 2 (very positive).",
    ),
    module: Some("cx"),
    permission: "ticket:read",
    time_field: Cow::Borrowed("at"),
    from: Cow::Borrowed("cx_sentiment s JOIN organizations o ON o.id = s.org_id"),
    fields: Cow::Borrowed(&[
        CLIENT,
        f("score", "Sentiment", Kind::Number, "s.score"),
        f("at", "Judged at", Kind::Time, "s.at"),
    ]),
};

/// Dataset sources' keys: `dataset:<dataset key>`.
/// First-year value: one-time plus twelve months of recurring.
const FIRST_YEAR: &str = "(op.amount + 12 * op.recurring)";

pub const OPPORTUNITIES: Source = Source {
    key: Cow::Borrowed("opportunities"),
    label: Cow::Borrowed("Opportunities"),
    description: Cow::Borrowed(
        "Deals in the pipeline: stage, owner, value, forecast category, and whether they were won or lost.",
    ),
    module: Some("sales"),
    permission: "sales:read",
    time_field: Cow::Borrowed("closed_at"),
    from: Cow::Borrowed(
        "sales_opportunities op JOIN organizations o ON o.id = op.org_id \
        LEFT JOIN sales_stages st ON st.id = op.stage_id LEFT JOIN users u ON u.id = op.owner_id \
        LEFT JOIN sales_reasons rs ON rs.id = op.reason_id",
    ),
    fields: Cow::Borrowed(&[
        CLIENT,
        f("name", "Name", Kind::Text, "op.name"),
        f("owner", "Owner", Kind::Text, "u.display_name"),
        f("stage", "Stage", Kind::Text, "st.name"),
        f("status", "Status", Kind::Text, "op.status"),
        f("forecast", "Forecast category", Kind::Text, "op.forecast"),
        f("source", "Source", Kind::Text, "op.source"),
        f("reason", "Won or lost because", Kind::Text, "rs.name"),
        f("won", "Won", Kind::Bool, "(op.status = 'won')"),
        f("open", "Open", Kind::Bool, "(op.status = 'open')"),
        f("amount", "One-time value", Kind::Number, "op.amount"),
        f("recurring", "Monthly value", Kind::Number, "op.recurring"),
        f("first_year", "First-year value", Kind::Number, FIRST_YEAR),
        f(
            "weighted",
            "Weighted value",
            Kind::Number,
            "((op.amount + 12 * op.recurring) * op.probability / 100.0)",
        ),
        f(
            "probability",
            "Probability %",
            Kind::Number,
            "op.probability",
        ),
        f("close_date", "Expected close", Kind::Date, "op.close_date"),
        f("closed_at", "Closed", Kind::Time, "op.closed_at"),
        f("created_at", "Created", Kind::Time, "op.created_at"),
    ]),
};

pub const QUOTES: Source = Source {
    key: Cow::Borrowed("quotes"),
    label: Cow::Borrowed("Quotes"),
    description: Cow::Borrowed(
        "Quotes: who they're for, their status and when they were sent and answered.",
    ),
    module: Some("sales"),
    permission: "sales:read",
    time_field: Cow::Borrowed("created_at"),
    from: Cow::Borrowed(
        "sales_quotes q JOIN organizations o ON o.id = q.org_id LEFT JOIN users u ON u.id = q.owner_id",
    ),
    fields: Cow::Borrowed(&[
        CLIENT,
        f("title", "Title", Kind::Text, "q.title"),
        f("owner", "Owner", Kind::Text, "u.display_name"),
        f("status", "Status", Kind::Text, "q.status"),
        f(
            "accepted",
            "Accepted",
            Kind::Bool,
            "(q.status = 'accepted')",
        ),
        f("revision", "Revision", Kind::Number, "q.revision"),
        f("sent_at", "Sent", Kind::Time, "q.sent_at"),
        f("decided_at", "Answered", Kind::Time, "q.decided_at"),
        f(
            "days_to_answer",
            "Days to answer",
            Kind::Number,
            "(extract(epoch FROM q.decided_at - q.sent_at) / 86400.0)",
        ),
        f("created_at", "Created", Kind::Time, "q.created_at"),
    ]),
};

pub const SALES_ACTIVITIES: Source = Source {
    key: Cow::Borrowed("sales_activities"),
    label: Cow::Borrowed("Sales activities"),
    description: Cow::Borrowed(
        "Tasks, calls, meetings, notes and logged emails on deals and leads.",
    ),
    module: Some("sales"),
    permission: "sales:read",
    time_field: Cow::Borrowed("created_at"),
    from: Cow::Borrowed(
        "sales_activities a JOIN organizations o ON o.id = a.org_id LEFT JOIN users u ON u.id = a.owner_id",
    ),
    fields: Cow::Borrowed(&[
        CLIENT,
        f("kind", "Kind", Kind::Text, "a.kind"),
        f("owner", "Owner", Kind::Text, "u.display_name"),
        f("done", "Done", Kind::Bool, "(a.done_at IS NOT NULL)"),
        f("direction", "Email direction", Kind::Text, "a.direction"),
        f("minutes", "Minutes", Kind::Number, "a.duration_minutes"),
        f("due_at", "Due", Kind::Time, "a.due_at"),
        f("created_at", "Logged", Kind::Time, "a.created_at"),
    ]),
};

pub const DATASET_PREFIX: &str = "dataset:";

/// Every source, in display order.
pub const SOURCES: &[Source] = &[
    TICKETS,
    TIME_ENTRIES,
    ASSETS,
    RUNS,
    CSAT,
    NPS,
    HEALTH,
    SENTIMENT,
    OPPORTUNITIES,
    QUOTES,
    SALES_ACTIVITIES,
];

pub fn source(key: &str) -> Option<&'static Source> {
    SOURCES.iter().find(|s| s.key == key)
}
