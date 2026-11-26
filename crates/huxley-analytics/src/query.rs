//! The structured query spec and its compiler (ADR-041). Identifiers in the
//! SQL come only from the catalog; every value is a bound parameter.

use std::fmt::Write as _;

use serde::{Deserialize, Serialize};
use serde_json::Value as Json;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::catalog::{self, Field, Kind, Source};
use crate::{AnalyticsError, AnalyticsResult};

pub const MAX_DIMENSIONS: usize = 3;
pub const MAX_MEASURES: usize = 6;
pub const MAX_FILTERS: usize = 20;
pub const MAX_LIMIT: u32 = 1000;

/// What to aggregate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Aggregate {
    Count,
    CountDistinct,
    Sum,
    Avg,
    Min,
    Max,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Measure {
    #[serde(rename = "fn")]
    pub aggregate: Aggregate,
    /// The field (not for `count`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub field: Option<String>,
}

/// A time bucket for a date or time dimension.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum Granularity {
    Hour,
    Day,
    Week,
    Month,
    Quarter,
    Year,
}

impl Granularity {
    fn sql(self) -> &'static str {
        match self {
            Self::Hour => "hour",
            Self::Day => "day",
            Self::Week => "week",
            Self::Month => "month",
            Self::Quarter => "quarter",
            Self::Year => "year",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Dimension {
    pub field: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub granularity: Option<Granularity>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Op {
    Eq,
    Neq,
    In,
    NotIn,
    Gt,
    Gte,
    Lt,
    Lte,
    /// `[from, to]`, both included.
    Between,
    /// Text containing the value (any case).
    Contains,
    IsNull,
    NotNull,
    /// A date or time within a named range, e.g. `last_30_days`.
    Within,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Filter {
    pub field: String,
    pub op: Op,
    #[serde(default, skip_serializing_if = "Json::is_null")]
    pub value: Json,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum Direction {
    Asc,
    Desc,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Sort {
    /// A column's key: a dimension's field, or a measure's key (`count`,
    /// `sum_hours`…).
    pub by: String,
    #[serde(default = "asc")]
    pub dir: Direction,
}

fn asc() -> Direction {
    Direction::Asc
}

/// A query: what to count or add up, by what, for which records.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct QuerySpec {
    pub source: String,
    #[serde(default)]
    pub measures: Vec<Measure>,
    #[serde(default)]
    pub dimensions: Vec<Dimension>,
    #[serde(default)]
    pub filters: Vec<Filter>,
    #[serde(default)]
    pub sort: Vec<Sort>,
    /// Rows to return (default 100, at most 1,000).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
    /// Only this organization and those below it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub org_id: Option<Uuid>,
    /// For time buckets and named ranges (default `UTC`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timezone: Option<String>,
}

/// A result column.
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = AnalyticsColumn)]
pub struct Column {
    pub key: String,
    pub label: String,
    pub kind: Kind,
}

/// A bound value.
#[derive(Debug, Clone, PartialEq)]
pub enum Bind {
    Text(String),
    Texts(Vec<String>),
    Uuid(Uuid),
    Uuids(Vec<Uuid>),
}

/// Which organizations the caller may see for the source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Scope {
    All,
    Orgs(Vec<Uuid>),
}

/// Compiled SQL: each row is one JSON object with keys `c0`, `c1`…
#[derive(Debug, Clone)]
pub struct Compiled {
    pub sql: String,
    pub binds: Vec<Bind>,
    pub columns: Vec<Column>,
    pub limit: u32,
}

/// Named ranges, as `[start, end)` over the local time `L`.
const RANGES: &[(&str, &str, &str)] = &[
    (
        "today",
        "date_trunc('day', L)",
        "date_trunc('day', L) + interval '1 day'",
    ),
    (
        "yesterday",
        "date_trunc('day', L) - interval '1 day'",
        "date_trunc('day', L)",
    ),
    (
        "last_7_days",
        "date_trunc('day', L) - interval '6 days'",
        "date_trunc('day', L) + interval '1 day'",
    ),
    (
        "last_30_days",
        "date_trunc('day', L) - interval '29 days'",
        "date_trunc('day', L) + interval '1 day'",
    ),
    (
        "last_90_days",
        "date_trunc('day', L) - interval '89 days'",
        "date_trunc('day', L) + interval '1 day'",
    ),
    (
        "this_week",
        "date_trunc('week', L)",
        "date_trunc('week', L) + interval '1 week'",
    ),
    (
        "last_week",
        "date_trunc('week', L) - interval '1 week'",
        "date_trunc('week', L)",
    ),
    (
        "this_month",
        "date_trunc('month', L)",
        "date_trunc('month', L) + interval '1 month'",
    ),
    (
        "last_month",
        "date_trunc('month', L) - interval '1 month'",
        "date_trunc('month', L)",
    ),
    (
        "this_quarter",
        "date_trunc('quarter', L)",
        "date_trunc('quarter', L) + interval '3 months'",
    ),
    (
        "last_quarter",
        "date_trunc('quarter', L) - interval '3 months'",
        "date_trunc('quarter', L)",
    ),
    (
        "this_year",
        "date_trunc('year', L)",
        "date_trunc('year', L) + interval '1 year'",
    ),
    (
        "last_year",
        "date_trunc('year', L) - interval '1 year'",
        "date_trunc('year', L)",
    ),
    (
        "last_12_months",
        "date_trunc('month', L) - interval '11 months'",
        "date_trunc('month', L) + interval '1 month'",
    ),
];

/// The named ranges `within` accepts.
pub fn ranges() -> impl Iterator<Item = &'static str> {
    RANGES.iter().map(|(k, _, _)| *k)
}

struct Builder {
    binds: Vec<Bind>,
}

impl Builder {
    fn bind(&mut self, b: Bind) -> String {
        self.binds.push(b);
        format!("${}", self.binds.len())
    }
}

fn bad(at: impl Into<String>, message: impl Into<String>) -> AnalyticsError {
    AnalyticsError::Invalid(at.into(), message.into())
}

fn valid_timezone(tz: &str) -> bool {
    tz == "UTC"
        || (tz.len() <= 64
            && tz.contains('/')
            && tz.split('/').all(|part| {
                !part.is_empty()
                    && part
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || "_+-".contains(c))
            }))
}

fn cast(kind: Kind) -> &'static str {
    match kind {
        Kind::Text => "text",
        Kind::Number => "numeric",
        Kind::Bool => "boolean",
        Kind::Date => "date",
        Kind::Time => "timestamptz",
    }
}

/// A filter or `between` value as text, checked for the field's kind.
fn scalar(field: &Field, at: &str, value: &Json) -> AnalyticsResult<String> {
    let text = match value {
        Json::String(s) => s.clone(),
        Json::Number(n) => n.to_string(),
        Json::Bool(b) => b.to_string(),
        _ => return Err(bad(at, "a text, number or true/false value")),
    };
    let ok = match field.kind {
        Kind::Text => text.chars().count() <= 500,
        Kind::Number => text.parse::<f64>().is_ok_and(f64::is_finite),
        Kind::Bool => matches!(text.as_str(), "true" | "false"),
        Kind::Date => is_date(&text),
        Kind::Time => is_time(&text),
    };
    if ok {
        Ok(text)
    } else {
        Err(bad(
            at,
            match field.kind {
                Kind::Text => "at most 500 characters",
                Kind::Number => "a number",
                Kind::Bool => "true or false",
                Kind::Date => "a date (YYYY-MM-DD)",
                Kind::Time => "a date (YYYY-MM-DD) or RFC 3339 time",
            },
        ))
    }
}

fn is_date(s: &str) -> bool {
    let b = s.as_bytes();
    s.len() == 10
        && b[4] == b'-'
        && b[7] == b'-'
        && b.iter()
            .enumerate()
            .all(|(i, c)| i == 4 || i == 7 || c.is_ascii_digit())
}

fn is_time(s: &str) -> bool {
    is_date(s)
        || (s.len() >= 19
            && s.len() <= 35
            && is_date(&s[..10])
            && matches!(s.as_bytes()[10], b'T' | b't' | b' ')
            && s[11..]
                .chars()
                .all(|c| c.is_ascii_digit() || ":.+-Zz".contains(c)))
}

fn measure_key(m: &Measure) -> String {
    match (&m.aggregate, &m.field) {
        (Aggregate::Count, _) => "count".into(),
        (a, Some(f)) => format!(
            "{}_{f}",
            serde_json::to_value(a)
                .ok()
                .and_then(|v| v.as_str().map(str::to_owned))
                .unwrap_or_default()
        ),
        (_, None) => String::new(),
    }
}

fn measure(source: &Source, m: &Measure, at: &str) -> AnalyticsResult<(String, Column)> {
    if m.aggregate == Aggregate::Count {
        return Ok((
            "count(*)".into(),
            Column {
                key: "count".into(),
                label: "Count".into(),
                kind: Kind::Number,
            },
        ));
    }
    let name = m
        .field
        .as_deref()
        .ok_or_else(|| bad(at, "this measure needs a field"))?;
    let field = source.field(name).ok_or_else(|| {
        bad(
            format!("{at}/field"),
            format!("{name:?} isn't a field of {}", source.key),
        )
    })?;
    let e = field.sql;
    let (sql, label, kind) = match m.aggregate {
        Aggregate::Count => unreachable!("handled above"),
        Aggregate::CountDistinct => (
            format!("count(DISTINCT {e})"),
            format!("Distinct {}", field.label),
            Kind::Number,
        ),
        Aggregate::Sum | Aggregate::Avg if field.kind != Kind::Number => {
            return Err(bad(
                format!("{at}/field"),
                "sum and avg need a number field",
            ));
        }
        Aggregate::Sum => (
            format!("sum({e})"),
            format!("Total {}", field.label),
            Kind::Number,
        ),
        Aggregate::Avg => (
            format!("round((avg({e}))::numeric, 2)"),
            format!("Average {}", field.label),
            Kind::Number,
        ),
        Aggregate::Min | Aggregate::Max if matches!(field.kind, Kind::Text | Kind::Bool) => {
            return Err(bad(
                format!("{at}/field"),
                "min and max need a number, date or time field",
            ));
        }
        Aggregate::Min => (
            format!("min({e})"),
            format!("Earliest or lowest {}", field.label),
            field.kind,
        ),
        Aggregate::Max => (
            format!("max({e})"),
            format!("Latest or highest {}", field.label),
            field.kind,
        ),
    };
    Ok((
        sql,
        Column {
            key: measure_key(m),
            label,
            kind,
        },
    ))
}

fn dimension(
    field: &Field,
    d: &Dimension,
    at: &str,
    tz: &str,
) -> AnalyticsResult<(String, Column)> {
    let e = field.sql;
    let Some(g) = d.granularity else {
        return Ok((
            e.to_owned(),
            Column {
                key: field.key.into(),
                label: field.label.into(),
                kind: field.kind,
            },
        ));
    };
    let sql = match (field.kind, g) {
        (Kind::Time, Granularity::Hour) => format!("date_trunc('hour', ({e}) AT TIME ZONE {tz})"),
        (Kind::Time, g) => format!("(date_trunc('{}', ({e}) AT TIME ZONE {tz}))::date", g.sql()),
        (Kind::Date, Granularity::Hour) => {
            return Err(bad(format!("{at}/granularity"), "a date has no hours"));
        }
        (Kind::Date, g) => format!("(date_trunc('{}', {e}))::date", g.sql()),
        _ => {
            return Err(bad(
                format!("{at}/granularity"),
                "only dates and times have time buckets",
            ));
        }
    };
    let kind = if g == Granularity::Hour {
        Kind::Time
    } else {
        Kind::Date
    };
    Ok((
        sql,
        Column {
            key: field.key.into(),
            label: field.label.into(),
            kind,
        },
    ))
}

fn filter(
    source: &Source,
    flt: &Filter,
    at: &str,
    tz: &str,
    binder: &mut Builder,
) -> AnalyticsResult<String> {
    let field = source.field(&flt.field).ok_or_else(|| {
        bad(
            format!("{at}/field"),
            format!("{:?} isn't a field of {}", flt.field, source.key),
        )
    })?;
    let expr = field.sql;
    let ty = cast(field.kind);
    let ordered = matches!(field.kind, Kind::Number | Kind::Date | Kind::Time);
    let value_at = format!("{at}/value");
    Ok(match flt.op {
        Op::IsNull => format!("({expr}) IS NULL"),
        Op::NotNull => format!("({expr}) IS NOT NULL"),
        Op::Eq | Op::Neq => {
            let p = binder.bind(Bind::Text(scalar(field, &value_at, &flt.value)?));
            let op = if flt.op == Op::Eq {
                "="
            } else {
                "IS DISTINCT FROM"
            };
            format!("({expr}) {op} {p}::{ty}")
        }
        Op::In | Op::NotIn => {
            let items = flt
                .value
                .as_array()
                .filter(|a| !a.is_empty() && a.len() <= 100)
                .ok_or_else(|| bad(&value_at, "a list of 1 to 100 values"))?;
            let texts = items
                .iter()
                .map(|v| scalar(field, &value_at, v))
                .collect::<AnalyticsResult<Vec<_>>>()?;
            let p = binder.bind(Bind::Texts(texts));
            let not = if flt.op == Op::NotIn { "NOT " } else { "" };
            format!("{not}(({expr}) = ANY ({p}::{ty}[]))")
        }
        Op::Gt | Op::Gte | Op::Lt | Op::Lte => {
            if !ordered {
                return Err(bad(
                    format!("{at}/op"),
                    "comparisons need a number, date or time field",
                ));
            }
            let p = binder.bind(Bind::Text(scalar(field, &value_at, &flt.value)?));
            let op = match flt.op {
                Op::Gt => ">",
                Op::Gte => ">=",
                Op::Lt => "<",
                _ => "<=",
            };
            format!("({expr}) {op} {p}::{ty}")
        }
        Op::Between => {
            if !ordered {
                return Err(bad(
                    format!("{at}/op"),
                    "between needs a number, date or time field",
                ));
            }
            let pair = flt
                .value
                .as_array()
                .filter(|a| a.len() == 2)
                .ok_or_else(|| bad(&value_at, "[from, to]"))?;
            let lo = binder.bind(Bind::Text(scalar(field, &value_at, &pair[0])?));
            let hi = binder.bind(Bind::Text(scalar(field, &value_at, &pair[1])?));
            format!("({expr}) BETWEEN {lo}::{ty} AND {hi}::{ty}")
        }
        Op::Contains => {
            if field.kind != Kind::Text {
                return Err(bad(format!("{at}/op"), "contains needs a text field"));
            }
            let p = binder.bind(Bind::Text(scalar(field, &value_at, &flt.value)?));
            format!(
                "({expr}) ILIKE '%' || replace(replace(replace({p}, '\\', '\\\\'), '%', '\\%'), '_', '\\_') || '%'"
            )
        }
        Op::Within => {
            let name = flt.value.as_str().unwrap_or_default();
            let (_, start, end) = RANGES.iter().find(|(k, _, _)| *k == name).ok_or_else(|| {
                bad(
                    &value_at,
                    format!("one of {}", ranges().collect::<Vec<_>>().join(", ")),
                )
            })?;
            let local = format!("(now() AT TIME ZONE {tz})");
            let (start, end) = (start.replace('L', &local), end.replace('L', &local));
            match field.kind {
                Kind::Time => format!(
                    "(({expr}) AT TIME ZONE {tz}) >= {start} AND (({expr}) AT TIME ZONE {tz}) < {end}"
                ),
                Kind::Date => format!("({expr}) >= ({start})::date AND ({expr}) < ({end})::date"),
                _ => return Err(bad(format!("{at}/op"), "within needs a date or time field")),
            }
        }
    })
}

/// Compiles a spec for a caller who may see `scope`.
pub fn compile(spec: &QuerySpec, scope: &Scope) -> AnalyticsResult<Compiled> {
    let source = catalog::source(&spec.source)
        .ok_or_else(|| bad("source", format!("{:?} isn't a source", spec.source)))?;
    if spec.measures.is_empty() && spec.dimensions.is_empty() {
        return Err(bad("measures", "ask for at least one measure or dimension"));
    }
    if spec.dimensions.len() > MAX_DIMENSIONS {
        return Err(bad("dimensions", format!("at most {MAX_DIMENSIONS}")));
    }
    if spec.measures.len() > MAX_MEASURES {
        return Err(bad("measures", format!("at most {MAX_MEASURES}")));
    }
    if spec.filters.len() > MAX_FILTERS {
        return Err(bad("filters", format!("at most {MAX_FILTERS}")));
    }
    let limit = spec.limit.unwrap_or(100).clamp(1, MAX_LIMIT);
    let tz_name = spec.timezone.clone().unwrap_or_else(|| "UTC".into());
    if !valid_timezone(&tz_name) {
        return Err(bad("timezone", "an IANA time zone such as Europe/Berlin"));
    }
    let mut b = Builder { binds: Vec::new() };
    let tz = b.bind(Bind::Text(tz_name));
    let mut select = Vec::new();
    let mut columns: Vec<Column> = Vec::new();
    for (i, d) in spec.dimensions.iter().enumerate() {
        let at = format!("dimensions/{i}");
        let field = source.field(&d.field).ok_or_else(|| {
            bad(
                format!("{at}/field"),
                format!("{:?} isn't a field of {}", d.field, source.key),
            )
        })?;
        if columns.iter().any(|c| c.key == field.key) {
            return Err(bad(&at, "each field once"));
        }
        let (sql, column) = dimension(field, d, &at, &tz)?;
        select.push(sql);
        columns.push(column);
    }
    for (i, m) in spec.measures.iter().enumerate() {
        let (sql, column) = measure(source, m, &format!("measures/{i}"))?;
        if columns.iter().any(|c| c.key == column.key) {
            return Err(bad(format!("measures/{i}"), "each measure once"));
        }
        select.push(sql);
        columns.push(column);
    }
    let mut wh: Vec<String> = Vec::new();
    match scope {
        Scope::All => {}
        Scope::Orgs(orgs) => {
            let p = b.bind(Bind::Uuids(orgs.clone()));
            wh.push(format!("o.id = ANY ({p})"));
        }
    }
    if let Some(org) = spec.org_id {
        let p = b.bind(Bind::Uuid(org));
        wh.push(format!(
            "o.path <@ (SELECT path FROM organizations WHERE id = {p})"
        ));
    }
    for (i, f) in spec.filters.iter().enumerate() {
        wh.push(filter(source, f, &format!("filters/{i}"), &tz, &mut b)?);
    }
    let mut sql = String::from("SELECT to_jsonb(q) FROM (SELECT ");
    let list: Vec<String> = select
        .iter()
        .enumerate()
        .map(|(i, s)| format!("{s} AS c{i}"))
        .collect();
    sql.push_str(&list.join(", "));
    let _ = write!(sql, " FROM {}", source.from);
    if !wh.is_empty() {
        let _ = write!(sql, " WHERE {}", wh.join(" AND "));
    }
    let dims = spec.dimensions.len();
    if dims > 0 && !spec.measures.is_empty() {
        let groups: Vec<String> = (1..=dims).map(|i| i.to_string()).collect();
        let _ = write!(sql, " GROUP BY {}", groups.join(", "));
    }
    let order = order_by(spec, &columns, dims)?;
    if !order.is_empty() {
        let _ = write!(sql, " ORDER BY {}", order.join(", "));
    }
    // One extra row says whether the result was cut short.
    let _ = write!(sql, " LIMIT {}) q", limit + 1);
    Ok(Compiled {
        sql,
        binds: b.binds,
        columns,
        limit,
    })
}

fn order_by(spec: &QuerySpec, columns: &[Column], dims: usize) -> AnalyticsResult<Vec<String>> {
    let mut order = Vec::new();
    for (i, s) in spec.sort.iter().enumerate() {
        let index = columns
            .iter()
            .position(|c| c.key == s.by)
            .ok_or_else(|| bad(format!("sort/{i}/by"), format!("{:?} isn't a column", s.by)))?;
        let dir = if s.dir == Direction::Desc {
            "DESC NULLS LAST"
        } else {
            "ASC NULLS LAST"
        };
        order.push(format!("{} {dir}", index + 1));
    }
    if order.is_empty() {
        // Time series by time; otherwise biggest first.
        let time = spec.dimensions.iter().position(|d| d.granularity.is_some());
        if let Some(t) = time {
            order.push(format!("{} ASC", t + 1));
        } else if columns.len() > dims {
            order.push(format!("{} DESC NULLS LAST", dims + 1));
        }
    }
    Ok(order)
}
