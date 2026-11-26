//! How time falls under agreements (ADR-039), computed on demand.
//!
//! Each time entry on a ticket is evaluated against one agreement, or
//! none: the agreements in force on its day at the ticket's organization or
//! above, nearest first, then by priority. The first that covers it wins;
//! else the first time-and-materials or project one; else the first. Covered
//! time draws down the agreement's allowance (a calendar month's minutes,
//! or a block for the term) in order: by day, then when it was logged.

use std::collections::{HashMap, HashSet};

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sqlx::PgConnection;
use time::{Date, OffsetDateTime};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::agreements::AgreementKind;
use crate::catalog::plain;
use crate::{ContractsError, ContractsResult};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
#[schema(as = CoverageOutcome)]
pub enum Outcome {
    /// Marked non-billable.
    NonBillable,
    /// Within coverage and allowance.
    Covered,
    /// Within coverage, using the last of the allowance.
    PartlyCovered,
    /// Within coverage, allowance used up.
    Overage,
    /// Outside coverage, or a time-and-materials or project agreement.
    Billable,
    /// No agreement applies.
    NoAgreement,
}

/// A time entry, evaluated.
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = TimeCoverage)]
pub struct Evaluation {
    pub time_entry_id: Uuid,
    pub ticket_id: Option<Uuid>,
    pub ticket_reference: Option<String>,
    pub user_name: String,
    pub work_type_id: Uuid,
    pub work_type_name: String,
    #[serde(with = "crate::date")]
    #[schema(value_type = String)]
    pub worked_on: Date,
    pub minutes: i32,
    pub outcome: Outcome,
    pub agreement_id: Option<Uuid>,
    pub agreement_name: Option<String>,
    pub covered_minutes: i32,
    /// Rounded up to the agreement's billing increment.
    pub billable_minutes: i32,
    #[serde(serialize_with = "plain")]
    #[schema(value_type = Option<String>)]
    pub hourly_rate: Option<Decimal>,
    #[serde(serialize_with = "plain")]
    #[schema(value_type = Option<String>)]
    pub amount: Option<Decimal>,
    pub currency: Option<String>,
    /// Why, in a few words.
    pub reason: String,
}

/// An agreement, as evaluation needs it.
#[derive(Debug, Clone)]
pub(crate) struct Terms {
    pub id: Uuid,
    pub name: String,
    /// Its organization's path (`ltree` as text).
    pub path: String,
    pub depth: i32,
    pub kind: AgreementKind,
    pub priority: i32,
    pub created_at: OffsetDateTime,
    pub starts_on: Date,
    pub ends_on: Option<Date>,
    pub covers_all: bool,
    pub work_types: HashSet<Uuid>,
    pub asset_types: HashSet<Uuid>,
    pub monthly_minutes: Option<i32>,
    pub block_minutes: Option<i32>,
    pub increment: i32,
    pub currency: String,
    pub rates: HashMap<Option<Uuid>, Decimal>,
}

impl Terms {
    fn in_force(&self, day: Date) -> bool {
        self.starts_on <= day && self.ends_on.is_none_or(|end| day <= end)
    }

    fn above(&self, path: &str) -> bool {
        path == self.path
            || path
                .strip_prefix(self.path.as_str())
                .is_some_and(|rest| rest.starts_with('.'))
    }

    fn covers(&self, work_type: Uuid, asset_types: &HashSet<Uuid>) -> bool {
        self.kind.covers()
            && (self.covers_all || self.work_types.contains(&work_type))
            && (self.asset_types.is_empty() || !self.asset_types.is_disjoint(asset_types))
    }

    /// Which allowance window a covered entry on `day` draws from, if any.
    fn window(&self, day: Date) -> Option<(Date, Date, i32)> {
        match (self.kind, self.monthly_minutes, self.block_minutes) {
            (AgreementKind::Managed, Some(minutes), _) => {
                let first = day.replace_day(1).ok()?;
                let last = first
                    .replace_day(time::util::days_in_month(first.month(), first.year()))
                    .ok()?;
                Some((first, last, minutes))
            }
            (AgreementKind::BlockHours, _, Some(minutes)) => {
                Some((self.starts_on, self.ends_on.unwrap_or(Date::MAX), minutes))
            }
            _ => None,
        }
    }

    fn rate(&self, work_type: Uuid) -> Option<Decimal> {
        self.rates
            .get(&Some(work_type))
            .or_else(|| self.rates.get(&None))
            .copied()
    }
}

/// A time entry, as evaluation needs it.
#[derive(Debug, Clone, sqlx::FromRow)]
pub(crate) struct Entry {
    pub id: Uuid,
    pub ticket_id: Option<Uuid>,
    pub ticket_reference: Option<String>,
    /// The ticket's organization's path.
    pub org_path: Option<String>,
    pub user_name: String,
    pub work_type_id: Uuid,
    pub work_type_name: String,
    pub worked_on: Date,
    pub minutes: i32,
    pub billable: bool,
    pub created_at: OffsetDateTime,
}

/// The agreement an entry falls under, and whether it covers it.
pub(crate) fn choose<'a>(
    entry: &Entry,
    terms: &'a [Terms],
    asset_types: &HashSet<Uuid>,
) -> Option<(&'a Terms, bool)> {
    let path = entry.org_path.as_deref()?;
    let mut candidates: Vec<&Terms> = terms
        .iter()
        .filter(|t| t.above(path) && t.in_force(entry.worked_on))
        .collect();
    candidates.sort_by(|a, b| {
        b.depth
            .cmp(&a.depth)
            .then(a.priority.cmp(&b.priority))
            .then(a.created_at.cmp(&b.created_at))
            .then(a.id.cmp(&b.id))
    });
    if let Some(t) = candidates
        .iter()
        .find(|t| t.covers(entry.work_type_id, asset_types))
    {
        return Some((t, true));
    }
    candidates
        .iter()
        .find(|t| {
            matches!(
                t.kind,
                AgreementKind::TimeMaterials | AgreementKind::Project
            )
        })
        .or_else(|| candidates.first())
        .map(|t| (*t, false))
}

fn round_up(minutes: i32, increment: i32) -> i32 {
    if minutes <= 0 {
        0
    } else {
        (minutes + increment - 1) / increment * increment
    }
}

/// Builds an entry's evaluation, given the agreement it falls under and,
/// for covered time with an allowance, how much of the allowance was left
/// before it.
pub(crate) fn evaluate_one(
    entry: &Entry,
    chosen: Option<(&Terms, bool)>,
    left_before: Option<i32>,
) -> Evaluation {
    let mut e = Evaluation {
        time_entry_id: entry.id,
        ticket_id: entry.ticket_id,
        ticket_reference: entry.ticket_reference.clone(),
        user_name: entry.user_name.clone(),
        work_type_id: entry.work_type_id,
        work_type_name: entry.work_type_name.clone(),
        worked_on: entry.worked_on,
        minutes: entry.minutes,
        outcome: Outcome::NoAgreement,
        agreement_id: chosen.map(|(t, _)| t.id),
        agreement_name: chosen.map(|(t, _)| t.name.clone()),
        covered_minutes: 0,
        billable_minutes: 0,
        hourly_rate: None,
        amount: None,
        currency: chosen.map(|(t, _)| t.currency.clone()),
        reason: String::new(),
    };
    if !entry.billable {
        e.outcome = Outcome::NonBillable;
        e.reason = "marked non-billable".into();
        return e;
    }
    let Some((terms, covered)) = chosen else {
        e.billable_minutes = entry.minutes;
        e.reason = if entry.ticket_id.is_some() {
            "no agreement in force for this client".into()
        } else {
            "not on a ticket".into()
        };
        return e;
    };
    let uncovered = if covered {
        match left_before {
            None => {
                e.outcome = Outcome::Covered;
                e.covered_minutes = entry.minutes;
                e.reason = "covered".into();
                0
            }
            Some(left) if left >= entry.minutes => {
                e.outcome = Outcome::Covered;
                e.covered_minutes = entry.minutes;
                e.reason = format!(
                    "covered; {} min of the allowance left after",
                    left - entry.minutes
                );
                0
            }
            Some(left) if left > 0 => {
                e.outcome = Outcome::PartlyCovered;
                e.covered_minutes = left;
                e.reason = "the allowance ran out during it".into();
                entry.minutes - left
            }
            Some(_) => {
                e.outcome = Outcome::Overage;
                e.reason = "covered work, but the allowance is used up".into();
                entry.minutes
            }
        }
    } else {
        e.outcome = Outcome::Billable;
        e.reason = if terms.kind.covers() {
            "outside what the agreement covers".into()
        } else {
            "billed at the agreed rates".into()
        };
        entry.minutes
    };
    e.billable_minutes = round_up(uncovered, terms.increment);
    if e.billable_minutes > 0 {
        e.hourly_rate = terms.rate(entry.work_type_id);
        e.amount = e
            .hourly_rate
            .map(|rate| (Decimal::from(e.billable_minutes) * rate / Decimal::from(60)).round_dp(2));
        if e.hourly_rate.is_none() {
            e.reason.push_str("; no rate set for this work type");
        }
    }
    e
}

const ENTRY: &str = "SELECT e.id, e.ticket_id, t.number_prefix || '-' || t.number AS ticket_reference, o.path::text AS org_path, \
    u.display_name AS user_name, e.work_type_id, w.name AS work_type_name, e.worked_on, e.minutes, \
    e.billable, e.created_at \
    FROM sd_time_entries e \
    JOIN users u ON u.id = e.user_id \
    JOIN sd_work_types w ON w.id = e.work_type_id \
    LEFT JOIN sd_tickets t ON t.id = e.ticket_id \
    LEFT JOIN organizations o ON o.id = e.org_id";

/// Every active agreement (evaluation looks across organizations: run it
/// in system scope, after checking access to what's asked about).
async fn all_terms(conn: &mut PgConnection) -> ContractsResult<Vec<Terms>> {
    #[derive(sqlx::FromRow)]
    struct Row {
        id: Uuid,
        name: String,
        path: String,
        depth: i32,
        kind: AgreementKind,
        priority: i32,
        created_at: OffsetDateTime,
        starts_on: Date,
        ends_on: Option<Date>,
        covers_all_work_types: bool,
        monthly_minutes: Option<i32>,
        block_minutes: Option<i32>,
        billing_increment: i32,
        currency: String,
    }
    let rows: Vec<Row> = sqlx::query_as(
        "SELECT a.id, a.name, o.path::text AS path, nlevel(o.path) AS depth, a.kind, a.priority, \
                a.created_at, a.starts_on, a.ends_on, a.covers_all_work_types, a.monthly_minutes, \
                a.block_minutes, a.billing_increment, a.currency \
           FROM contract_agreements a JOIN organizations o ON o.id = a.org_id WHERE a.status = 'active'",
    )
    .fetch_all(&mut *conn)
    .await?;
    let ids: Vec<Uuid> = rows.iter().map(|r| r.id).collect();
    let work_types: Vec<(Uuid, Uuid)> = sqlx::query_as(
        "SELECT agreement_id, work_type_id FROM contract_agreement_work_types WHERE agreement_id = ANY($1)",
    )
    .bind(&ids)
    .fetch_all(&mut *conn)
    .await?;
    let asset_types: Vec<(Uuid, Uuid)> = sqlx::query_as(
        "SELECT agreement_id, asset_type_id FROM contract_agreement_asset_types WHERE agreement_id = ANY($1)",
    )
    .bind(&ids)
    .fetch_all(&mut *conn)
    .await?;
    let rates: Vec<(Uuid, Option<Uuid>, Decimal)> = sqlx::query_as(
        "SELECT agreement_id, work_type_id, hourly_rate FROM contract_rates WHERE agreement_id = ANY($1)",
    )
    .bind(&ids)
    .fetch_all(&mut *conn)
    .await?;
    let mut terms: Vec<Terms> = rows
        .into_iter()
        .map(|r| Terms {
            id: r.id,
            name: r.name,
            path: r.path,
            depth: r.depth,
            kind: r.kind,
            priority: r.priority,
            created_at: r.created_at,
            starts_on: r.starts_on,
            ends_on: r.ends_on,
            covers_all: r.covers_all_work_types,
            work_types: HashSet::new(),
            asset_types: HashSet::new(),
            monthly_minutes: r.monthly_minutes,
            block_minutes: r.block_minutes,
            increment: r.billing_increment,
            currency: r.currency,
            rates: HashMap::new(),
        })
        .collect();
    let mut by: HashMap<Uuid, &mut Terms> = terms.iter_mut().map(|t| (t.id, t)).collect();
    for (a, w) in work_types {
        if let Some(t) = by.get_mut(&a) {
            t.work_types.insert(w);
        }
    }
    for (a, at) in asset_types {
        if let Some(t) = by.get_mut(&a) {
            t.asset_types.insert(at);
        }
    }
    for (a, w, r) in rates {
        if let Some(t) = by.get_mut(&a) {
            t.rates.insert(w, r);
        }
    }
    Ok(terms)
}

/// The asset types each ticket is about (its linked assets' types).
async fn ticket_asset_types(
    conn: &mut PgConnection,
    tickets: &[Uuid],
) -> ContractsResult<HashMap<Uuid, HashSet<Uuid>>> {
    let rows: Vec<(Uuid, Uuid)> = sqlx::query_as(
        "SELECT l.ticket_id, a.type_id FROM cmdb_asset_tickets l JOIN assets a ON a.id = l.asset_id \
          WHERE l.ticket_id = ANY($1)",
    )
    .bind(tickets)
    .fetch_all(conn)
    .await?;
    let mut out: HashMap<Uuid, HashSet<Uuid>> = HashMap::new();
    for (ticket, asset_type) in rows {
        out.entry(ticket).or_default().insert(asset_type);
    }
    Ok(out)
}

async fn assets_for(
    conn: &mut PgConnection,
    entries: &[Entry],
    known: &mut HashMap<Uuid, HashSet<Uuid>>,
) -> ContractsResult<()> {
    let missing: Vec<Uuid> = entries
        .iter()
        .filter_map(|e| e.ticket_id)
        .filter(|t| !known.contains_key(t))
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    if !missing.is_empty() {
        let found = ticket_asset_types(conn, &missing).await?;
        for t in missing {
            known.insert(t, found.get(&t).cloned().unwrap_or_default());
        }
    }
    Ok(())
}

/// Evaluates the given entries (system scope; see [`all_terms`]), in the
/// order given.
pub async fn evaluate(
    conn: &mut PgConnection,
    entry_ids: &[Uuid],
) -> ContractsResult<Vec<Evaluation>> {
    let entries: Vec<Entry> =
        sqlx::query_as(sqlx::AssertSqlSafe(format!("{ENTRY} WHERE e.id = ANY($1)")))
            .bind(entry_ids)
            .fetch_all(&mut *conn)
            .await?;
    let terms = all_terms(&mut *conn).await?;
    let mut assets: HashMap<Uuid, HashSet<Uuid>> = HashMap::new();
    assets_for(&mut *conn, &entries, &mut assets).await?;
    let none = HashSet::new();
    let asset_types = |e: &Entry, assets: &HashMap<Uuid, HashSet<Uuid>>| {
        e.ticket_id
            .and_then(|t| assets.get(&t))
            .cloned()
            .unwrap_or_else(|| none.clone())
    };

    // What's left of each allowance before each covered entry that draws on one.
    let mut left_before: HashMap<Uuid, i32> = HashMap::new();
    let mut windows: HashSet<(Uuid, Date, Date)> = HashSet::new();
    for e in &entries {
        if let Some((t, true)) = choose(e, &terms, &asset_types(e, &assets))
            && let Some((from, to, _)) = t.window(e.worked_on)
        {
            windows.insert((t.id, from, to));
        }
    }
    for (agreement, from, to) in windows {
        let t = terms
            .iter()
            .find(|t| t.id == agreement)
            .ok_or(ContractsError::NotFound)?;
        let Some((_, _, allowance)) = t.window(from) else {
            continue;
        };
        let mut window: Vec<Entry> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "{ENTRY} WHERE e.billable AND e.ticket_id IS NOT NULL AND e.worked_on BETWEEN $2 AND $3 \
               AND o.path <@ $1::ltree"
        )))
        .bind(&t.path)
        .bind(from)
        .bind(to)
        .fetch_all(&mut *conn)
        .await?;
        window.sort_by(|a, b| {
            a.worked_on
                .cmp(&b.worked_on)
                .then(a.created_at.cmp(&b.created_at))
                .then(a.id.cmp(&b.id))
        });
        assets_for(&mut *conn, &window, &mut assets).await?;
        let mut left = allowance;
        for w in &window {
            if let Some((chosen, true)) = choose(w, &terms, &asset_types(w, &assets))
                && chosen.id == agreement
            {
                left_before.insert(w.id, left);
                left = (left - w.minutes).max(0);
            }
        }
    }

    let mut by_id: HashMap<Uuid, Evaluation> = entries
        .iter()
        .map(|e| {
            let chosen = choose(e, &terms, &asset_types(e, &assets));
            (
                e.id,
                evaluate_one(e, chosen, left_before.get(&e.id).copied()),
            )
        })
        .collect();
    Ok(entry_ids.iter().filter_map(|id| by_id.remove(id)).collect())
}

/// A ticket's time entries, evaluated, oldest first.
pub async fn for_ticket(
    conn: &mut PgConnection,
    ticket_id: Uuid,
) -> ContractsResult<Vec<Evaluation>> {
    let ids: Vec<Uuid> = sqlx::query_scalar(
        "SELECT id FROM sd_time_entries WHERE ticket_id = $1 ORDER BY worked_on, created_at, id",
    )
    .bind(ticket_id)
    .fetch_all(&mut *conn)
    .await?;
    evaluate(conn, &ids).await
}

/// What fell under an agreement between two days, and its totals.
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = AgreementUsage)]
pub struct Usage {
    /// Only time on approved timesheets.
    pub approved_only: bool,
    #[serde(with = "crate::date")]
    #[schema(value_type = String)]
    pub from: Date,
    #[serde(with = "crate::date")]
    #[schema(value_type = String)]
    pub to: Date,
    pub entries: Vec<Evaluation>,
    pub covered_minutes: i32,
    pub billable_minutes: i32,
    /// What the billable time comes to (entries without a rate left out).
    #[serde(serialize_with = "plain")]
    #[schema(value_type = Option<String>)]
    pub amount: Option<Decimal>,
    pub currency: String,
}

/// Entries on tickets at the agreement's organization or below, between
/// `from` and `to`, that fall under it. With `approved_only`, only time on
/// approved timesheets (a billing preview that bills only approved time).
/// Allowances are still drawn down by all covered time, in order.
pub async fn usage(
    conn: &mut PgConnection,
    agreement_id: Uuid,
    from: Date,
    to: Date,
    approved_only: bool,
) -> ContractsResult<Usage> {
    let (path, currency): (String, String) = sqlx::query_as(
        "SELECT o.path::text, a.currency FROM contract_agreements a JOIN organizations o ON o.id = a.org_id \
          WHERE a.id = $1",
    )
    .bind(agreement_id)
    .fetch_optional(&mut *conn)
    .await?
    .ok_or(ContractsError::NotFound)?;
    let ids: Vec<Uuid> = sqlx::query_scalar(
        "SELECT e.id FROM sd_time_entries e JOIN organizations o ON o.id = e.org_id \
          WHERE e.ticket_id IS NOT NULL AND e.worked_on BETWEEN $2 AND $3 AND o.path <@ $1::ltree \
            AND (NOT $4 OR EXISTS (SELECT 1 FROM wf_timesheets s WHERE s.user_id = e.user_id \
                   AND s.status = 'approved' AND e.worked_on BETWEEN s.period_start AND s.period_end)) \
          ORDER BY e.worked_on, e.created_at, e.id",
    )
    .bind(&path)
    .bind(from)
    .bind(to)
    .bind(approved_only)
    .fetch_all(&mut *conn)
    .await?;
    let entries: Vec<Evaluation> = evaluate(conn, &ids)
        .await?
        .into_iter()
        .filter(|e| e.agreement_id == Some(agreement_id))
        .collect();
    let amounts: Vec<Decimal> = entries.iter().filter_map(|e| e.amount).collect();
    Ok(Usage {
        approved_only,
        from,
        to,
        covered_minutes: entries.iter().map(|e| e.covered_minutes).sum(),
        billable_minutes: entries.iter().map(|e| e.billable_minutes).sum(),
        amount: (!amounts.is_empty()).then(|| amounts.iter().sum()),
        currency,
        entries,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::{date, datetime};

    fn terms(kind: AgreementKind, path: &str) -> Terms {
        Terms {
            id: Uuid::now_v7(),
            name: format!("{kind:?}"),
            path: path.into(),
            depth: i32::try_from(path.split('.').count()).unwrap(),
            kind,
            priority: 0,
            created_at: datetime!(2026-01-01 0:00 UTC),
            starts_on: date!(2026 - 01 - 01),
            ends_on: Some(date!(2026 - 12 - 31)),
            covers_all: true,
            work_types: HashSet::new(),
            asset_types: HashSet::new(),
            monthly_minutes: None,
            block_minutes: None,
            increment: 1,
            currency: "USD".into(),
            rates: HashMap::from([(None, Decimal::from(120))]),
        }
    }

    fn entry(path: &str, work_type: Uuid, minutes: i32) -> Entry {
        Entry {
            id: Uuid::now_v7(),
            ticket_id: Some(Uuid::now_v7()),
            ticket_reference: Some("T-1".into()),
            org_path: Some(path.into()),
            user_name: "Ada".into(),
            work_type_id: work_type,
            work_type_name: "Remote".into(),
            worked_on: date!(2026 - 03 - 10),
            minutes,
            billable: true,
            created_at: datetime!(2026-03-10 9:00 UTC),
        }
    }

    #[test]
    fn the_nearest_covering_agreement_wins() {
        let remote = Uuid::now_v7();
        let onsite = Uuid::now_v7();
        let mut msp_tm = terms(AgreementKind::TimeMaterials, "root");
        msp_tm.name = "MSP T&M".into();
        let mut client_managed = terms(AgreementKind::Managed, "root.acme");
        client_managed.covers_all = false;
        client_managed.work_types.insert(remote);
        let all = vec![msp_tm.clone(), client_managed.clone()];
        let none = HashSet::new();

        let (t, covered) = choose(&entry("root.acme", remote, 30), &all, &none).unwrap();
        assert_eq!((t.id, covered), (client_managed.id, true));
        // Not covered: the time-and-materials one, above, bills it.
        let (t, covered) = choose(&entry("root.acme", onsite, 30), &all, &none).unwrap();
        assert_eq!((t.id, covered), (msp_tm.id, false));
        // Another client: only the MSP's.
        let (t, _) = choose(&entry("root.globex", remote, 30), &all, &none).unwrap();
        assert_eq!(t.id, msp_tm.id);
        // A sibling whose name starts the same isn't below.
        assert!(
            choose(
                &entry("root.acmecorp", remote, 30),
                &[client_managed.clone()],
                &none
            )
            .is_none()
        );
        // Out of term: nothing.
        let mut late = entry("root.acme", remote, 30);
        late.worked_on = date!(2027 - 01 - 05);
        assert!(choose(&late, &all, &none).is_none());
    }

    #[test]
    fn asset_types_limit_coverage() {
        let wt = Uuid::now_v7();
        let servers = Uuid::now_v7();
        let mut managed = terms(AgreementKind::Managed, "root.acme");
        managed.asset_types.insert(servers);
        let all = vec![managed];
        assert!(
            !choose(&entry("root.acme", wt, 30), &all, &HashSet::new())
                .unwrap()
                .1
        );
        assert!(
            choose(&entry("root.acme", wt, 30), &all, &HashSet::from([servers]))
                .unwrap()
                .1
        );
    }

    #[test]
    fn allowances_rounding_and_rates() {
        let wt = Uuid::now_v7();
        let mut block = terms(AgreementKind::BlockHours, "root.acme");
        block.block_minutes = Some(60);
        block.increment = 15;
        block.rates.insert(Some(wt), Decimal::from(150));
        let e = entry("root.acme", wt, 50);
        let covered = evaluate_one(&e, Some((&block, true)), Some(60));
        assert_eq!(
            (
                covered.outcome,
                covered.covered_minutes,
                covered.billable_minutes
            ),
            (Outcome::Covered, 50, 0)
        );
        let partly = evaluate_one(&e, Some((&block, true)), Some(10));
        // 40 uncovered minutes round up to 45, at the work type's 150/h.
        assert_eq!(
            (
                partly.outcome,
                partly.covered_minutes,
                partly.billable_minutes
            ),
            (Outcome::PartlyCovered, 10, 45)
        );
        assert_eq!(partly.amount, Some(Decimal::new(11250, 2)));
        let over = evaluate_one(&e, Some((&block, true)), Some(0));
        assert_eq!(
            (over.outcome, over.billable_minutes),
            (Outcome::Overage, 60)
        );
        // Unlimited coverage.
        let managed = terms(AgreementKind::Managed, "root.acme");
        assert_eq!(
            evaluate_one(&e, Some((&managed, true)), None).outcome,
            Outcome::Covered
        );
        // Outside coverage: billable at the default rate.
        let out = evaluate_one(&e, Some((&managed, false)), None);
        assert_eq!(
            (out.outcome, out.billable_minutes, out.amount),
            (Outcome::Billable, 50, Some(Decimal::from(100)))
        );
        // Non-billable and no agreement.
        let mut free = e.clone();
        free.billable = false;
        assert_eq!(
            evaluate_one(&free, Some((&managed, true)), None).outcome,
            Outcome::NonBillable
        );
        let lone = evaluate_one(&e, None, None);
        assert_eq!(
            (lone.outcome, lone.billable_minutes, lone.amount),
            (Outcome::NoAgreement, 50, None)
        );
    }

    #[test]
    fn monthly_windows_follow_the_calendar() {
        let mut managed = terms(AgreementKind::Managed, "root");
        managed.monthly_minutes = Some(600);
        assert_eq!(
            managed.window(date!(2026 - 02 - 14)),
            Some((date!(2026 - 02 - 01), date!(2026 - 02 - 28), 600))
        );
        let mut block = terms(AgreementKind::BlockHours, "root");
        block.block_minutes = Some(1200);
        assert_eq!(
            block.window(date!(2026 - 07 - 01)),
            Some((date!(2026 - 01 - 01), date!(2026 - 12 - 31), 1200))
        );
        assert_eq!(
            terms(AgreementKind::TimeMaterials, "root").window(date!(2026 - 07 - 01)),
            None
        );
    }
}
