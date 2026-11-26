//! The sources a query can ask about (ADR-041). Each is a curated view of
//! one kind of record: its `FROM` (always joining the owning organization
//! as `o`), the module and permission that govern it, and its fields.

use serde::Serialize;
use utoipa::ToSchema;

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
    pub key: &'static str,
    pub label: &'static str,
    pub kind: Kind,
    #[serde(skip)]
    pub sql: &'static str,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = AnalyticsSource)]
pub struct Source {
    pub key: &'static str,
    pub label: &'static str,
    pub description: &'static str,
    /// The module it belongs to, if any.
    pub module: Option<&'static str>,
    /// The permission that reads it.
    pub permission: &'static str,
    /// The time field most queries bucket by.
    pub time_field: &'static str,
    pub fields: &'static [Field],
    #[serde(skip)]
    pub from: &'static str,
}

impl Source {
    pub fn field(&self, key: &str) -> Option<&Field> {
        self.fields.iter().find(|f| f.key == key)
    }
}

const fn f(key: &'static str, label: &'static str, kind: Kind, sql: &'static str) -> Field {
    Field {
        key,
        label,
        kind,
        sql,
    }
}

const CLIENT: Field = f("client", "Client", Kind::Text, "o.name");

pub const TICKETS: Source = Source {
    key: "tickets",
    label: "Tickets",
    description: "Service desk tickets, with their status, queue, assignee and SLA outcome.",
    module: Some("servicedesk"),
    permission: "ticket:read",
    time_field: "created_at",
    from: "sd_tickets t JOIN organizations o ON o.id = t.org_id \
        LEFT JOIN sd_statuses s ON s.id = t.status_id LEFT JOIN sd_queues q ON q.id = t.queue_id \
        LEFT JOIN sd_ticket_types ty ON ty.id = t.type_id LEFT JOIN users au ON au.id = t.assignee_user_id \
        LEFT JOIN teams tm ON tm.id = t.assignee_team_id LEFT JOIN sd_ticket_sla sla ON sla.ticket_id = t.id",
    fields: &[
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
    ],
};

pub const TIME_ENTRIES: Source = Source {
    key: "time_entries",
    label: "Time entries",
    description: "Time logged on tickets: who, for which client, what kind of work, and whether it's billable.",
    module: Some("servicedesk"),
    permission: "ticket:read",
    time_field: "worked_on",
    from: "sd_time_entries e JOIN organizations o ON o.id = e.org_id LEFT JOIN users u ON u.id = e.user_id \
        LEFT JOIN sd_work_types w ON w.id = e.work_type_id LEFT JOIN sd_tickets t ON t.id = e.ticket_id",
    fields: &[
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
    ],
};

pub const ASSETS: Source = Source {
    key: "assets",
    label: "Assets",
    description: "Configuration items: devices and other assets, with their type, status and warranty.",
    module: Some("cmdb"),
    permission: "asset:read",
    time_field: "created_at",
    from: "assets a JOIN organizations o ON o.id = a.org_id LEFT JOIN cmdb_asset_types ty ON ty.id = a.type_id \
        LEFT JOIN crm_sites si ON si.id = a.site_id",
    fields: &[
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
    ],
};

pub const RUNS: Source = Source {
    key: "runs",
    label: "Workflow runs",
    description: "Workflow runs: which workflow, how it was triggered, how it ended and how long it took.",
    module: None,
    permission: "workflow:read",
    time_field: "created_at",
    from: "runs r JOIN organizations o ON o.id = r.org_id LEFT JOIN workflows wf ON wf.id = r.workflow_id",
    fields: &[
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
    ],
};

/// Every source, in display order.
pub const SOURCES: &[Source] = &[TICKETS, TIME_ENTRIES, ASSETS, RUNS];

pub fn source(key: &str) -> Option<&'static Source> {
    SOURCES.iter().find(|s| s.key == key)
}
