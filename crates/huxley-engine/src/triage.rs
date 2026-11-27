//! The AI triage steps (ADR-047): `core.triage.options` lists what a ticket
//! can be triaged to, and `core.triage.apply` records the AI's answer on a
//! ticket, or in an evaluation for a labeled sample.

use std::fmt::Write as _;

use huxley_core::{RequestContext, Via};
use huxley_db::{OrgScope, begin_scoped};
use huxley_servicedesk::{DeskError, triage};
use serde_json::{Value as Json, json};
use uuid::Uuid;

use crate::Engine;
use crate::walk::error;

fn internal(e: impl std::fmt::Display) -> Json {
    error("internal", e.to_string())
}

fn desk_error(e: DeskError) -> Json {
    match e {
        DeskError::NotFound => error("not_found", "not found here"),
        DeskError::Invalid { field, message } => error("input", format!("{field}: {message}")),
        DeskError::VersionConflict => error("conflict", "it changed while the step ran; retry it"),
        DeskError::Db(e) => internal(e),
        DeskError::Sqlx(e) => internal(e),
    }
}

/// An optional id input (empty: none).
fn id(input: &Json, key: &str) -> Result<Option<Uuid>, Json> {
    match input[key].as_str().map(str::trim).filter(|s| !s.is_empty()) {
        None => Ok(None),
        Some(s) => s
            .parse()
            .map(Some)
            .map_err(|_| error("input", format!("{key} isn't an id"))),
    }
}

fn text<'a>(input: &'a Json, key: &str) -> &'a str {
    input[key].as_str().unwrap_or_default()
}

/// The skill names an answer gave (a list, or one name), without "(none)".
fn skills_of(input: &Json) -> Vec<String> {
    let names: Vec<String> = match &input["skills"] {
        Json::Array(items) => items
            .iter()
            .filter_map(|v| v.as_str().map(str::to_owned))
            .collect(),
        Json::String(s) if !s.trim().is_empty() => vec![s.clone()],
        _ => Vec::new(),
    };
    names
        .into_iter()
        .map(|n| n.trim().to_owned())
        .filter(|n| !n.is_empty() && !n.eq_ignore_ascii_case(triage::NO_QUEUE))
        .take(10)
        .collect()
}

impl Engine {
    /// The skill catalog (name, description) when the workforce module is
    /// on at `org_id`; empty otherwise.
    async fn skill_catalog(&self, org_id: Uuid) -> Result<Vec<(String, String)>, Json> {
        let mut tx = begin_scoped(self.db(), &OrgScope::System)
            .await
            .map_err(internal)?;
        let on = huxley_modules::is_active_at(&mut tx, huxley_workforce::MODULE, org_id)
            .await
            .map_err(internal)?;
        let skills: Vec<(String, String)> = if on {
            sqlx::query_as("SELECT name, description FROM wf_skills WHERE archived_at IS NULL ORDER BY lower(name) LIMIT 100")
                .fetch_all(&mut *tx)
                .await
                .map_err(internal)?
        } else {
            Vec::new()
        };
        tx.commit().await.map_err(internal)?;
        Ok(skills)
    }

    /// Sets a ticket's needed skills (at working level) from their names,
    /// unless it has its own requirements already or the workforce module
    /// is off. Returns how many were set.
    async fn set_ticket_skills(
        &self,
        tx: &mut sqlx::PgConnection,
        ctx: &RequestContext,
        ticket: Uuid,
        names: &[String],
    ) -> Result<usize, Json> {
        if names.is_empty() {
            return Ok(0);
        }
        let org: Uuid = sqlx::query_scalar("SELECT org_id FROM sd_tickets WHERE id = $1")
            .bind(ticket)
            .fetch_one(&mut *tx)
            .await
            .map_err(internal)?;
        if !huxley_modules::is_active_at(&mut *tx, huxley_workforce::MODULE, org)
            .await
            .map_err(internal)?
        {
            return Ok(0);
        }
        let own = huxley_workforce::requirements::of(&mut *tx, "ticket", ticket)
            .await
            .map_err(internal)?;
        if !own.is_empty() {
            return Ok(0);
        }
        let lower: Vec<String> = names.iter().map(|n| n.to_lowercase()).collect();
        let ids: Vec<Uuid> = sqlx::query_scalar(
            "SELECT id FROM wf_skills WHERE archived_at IS NULL AND lower(name) = ANY($1)",
        )
        .bind(&lower)
        .fetch_all(&mut *tx)
        .await
        .map_err(internal)?;
        let wanted: Vec<huxley_workforce::requirements::Requirement> = ids
            .iter()
            .map(|id| huxley_workforce::requirements::Requirement {
                skill_id: Some(*id),
                min_level: Some(2),
                certification_id: None,
                name: None,
            })
            .collect();
        if wanted.is_empty() {
            return Ok(0);
        }
        huxley_workforce::requirements::set(&mut *tx, ctx, "ticket", ticket, &wanted)
            .await
            .map_err(internal)?;
        Ok(wanted.len())
    }

    /// `core.triage.options`: `{ queues, queue_names, priority_keys, guide }`
    /// for a ticket's organization (or the run's).
    pub(crate) async fn triage_options(&self, org_id: Uuid, input: &Json) -> Result<Json, Json> {
        self.require_desk(org_id).await?;
        let subtree = self.subtree(org_id).await?;
        let mut tx = begin_scoped(self.db(), &OrgScope::Orgs(subtree))
            .await
            .map_err(internal)?;
        let at = match id(input, "ticket_id")? {
            Some(ticket) => sqlx::query_scalar("SELECT org_id FROM sd_tickets WHERE id = $1")
                .bind(ticket)
                .fetch_optional(&mut *tx)
                .await
                .map_err(internal)?
                .ok_or_else(|| error("not_found", "No such ticket here"))?,
            None => org_id,
        };
        let options = triage::options(&mut tx, at).await.map_err(desk_error)?;
        tx.commit().await.map_err(internal)?;
        let mut out = json!(options);
        // The skills it may need, while the workforce module is on there.
        let skills = self.skill_catalog(at).await?;
        let mut names: Vec<Json> = skills.iter().map(|(name, _)| json!(name)).collect();
        names.push(json!(triage::NO_QUEUE));
        if !skills.is_empty() {
            let mut guide = out["guide"].as_str().unwrap_or_default().to_owned();
            guide.push_str("Skills:\n");
            for (name, about) in &skills {
                let _ = if about.is_empty() {
                    writeln!(guide, "- {name}")
                } else {
                    writeln!(guide, "- {name}: {about}")
                };
            }
            out["guide"] = json!(guide);
        }
        out["skill_names"] = json!(names);
        Ok(out)
    }

    /// `core.triage.apply`: with `ticket_id`, records the suggestion and
    /// applies it while no one has worked the ticket; with `evaluation_id`
    /// and `sample_id`, records the answer in the evaluation. Output: `{
    /// mode, applied, queue_id, priority }`.
    pub(crate) async fn triage_apply(&self, org_id: Uuid, input: &Json) -> Result<Json, Json> {
        self.require_desk(org_id).await?;
        let subtree = self.subtree(org_id).await?;
        let (queue, priority, reason) = (
            text(input, "queue"),
            text(input, "priority"),
            text(input, "reason"),
        );
        let mut tx = begin_scoped(self.db(), &OrgScope::Orgs(subtree))
            .await
            .map_err(internal)?;
        let mut pending_skills: Option<(Uuid, Vec<String>)> = None;
        let out = if let Some(ticket) = id(input, "ticket_id")? {
            let ctx = RequestContext::system(Via::Workflow);
            let skills = skills_of(input);
            let answer = triage::Answer {
                queue,
                priority,
                reason,
                skills: &skills,
            };
            let applied = triage::apply(&mut tx, &ctx, ticket, &answer)
                .await
                .map_err(desk_error)?;
            // Needed skills go in after the ticket's checked to be here (the
            // skill catalog and module settings are read across scopes).
            let set_skills = if applied.apply_skills {
                Some((ticket, skills.clone()))
            } else {
                None
            };
            pending_skills = set_skills;
            json!({
                "mode": "ticket", "applied": applied.applied, "suggest_only": applied.suggest_only,
                "queue_id": applied.queue_id, "priority": applied.priority, "skills": skills,
            })
        } else {
            let (Some(evaluation), Some(sample)) =
                (id(input, "evaluation_id")?, id(input, "sample_id")?)
            else {
                return Err(error(
                    "input",
                    "give ticket_id, or evaluation_id and sample_id",
                ));
            };
            triage::answer(&mut tx, evaluation, sample, queue, priority, reason)
                .await
                .map_err(desk_error)?;
            json!({ "mode": "evaluation", "applied": false, "queue_id": null, "priority": priority })
        };
        tx.commit().await.map_err(internal)?;
        let mut out = out;
        if let Some((ticket, skills)) = pending_skills {
            let ctx = RequestContext::system(Via::Workflow);
            let mut tx = begin_scoped(self.db(), &OrgScope::System)
                .await
                .map_err(internal)?;
            let set = self
                .set_ticket_skills(&mut tx, &ctx, ticket, &skills)
                .await?;
            tx.commit().await.map_err(internal)?;
            out["skills_set"] = json!(set);
        }
        Ok(out)
    }
}

impl Engine {
    /// `core.ticket.insight`: records a ticket's AI summary, entities and
    /// next step (a ticket in the run's organization or below). Output: the
    /// insight.
    pub(crate) async fn ticket_insight(&self, org_id: Uuid, input: &Json) -> Result<Json, Json> {
        self.require_desk(org_id).await?;
        let ticket =
            id(input, "ticket_id")?.ok_or_else(|| error("input", "ticket_id is required"))?;
        let subtree = self.subtree(org_id).await?;
        let mut tx = begin_scoped(self.db(), &OrgScope::Orgs(subtree))
            .await
            .map_err(internal)?;
        let saved = huxley_servicedesk::insights::record(
            &mut tx,
            ticket,
            text(input, "summary"),
            &input["entities"],
            input["next_step"].as_str(),
        )
        .await
        .map_err(desk_error)?;
        tx.commit().await.map_err(internal)?;
        Ok(json!(saved))
    }
}

impl Engine {
    /// `core.change.decide`: records the approval decision on a submitted
    /// change (in the run's organization or below). Output: the change.
    pub(crate) async fn change_decide(&self, org_id: Uuid, input: &Json) -> Result<Json, Json> {
        self.require_desk(org_id).await?;
        let mut tx = begin_scoped(self.db(), &OrgScope::System)
            .await
            .map_err(internal)?;
        let on = huxley_modules::is_active_at(&mut tx, "change", org_id)
            .await
            .map_err(internal)?;
        tx.commit().await.map_err(internal)?;
        if !on {
            return Err(error(
                "module_disabled",
                "Change and problem management is turned off",
            ));
        }
        let ticket =
            id(input, "ticket_id")?.ok_or_else(|| error("input", "ticket_id is required"))?;
        let approved = match &input["decision"] {
            Json::Bool(b) => *b,
            Json::String(s) if s == "approved" => true,
            Json::String(s) if s == "rejected" => false,
            _ => return Err(error("input", "decision is approved or rejected")),
        };
        let by = input["by"]
            .as_str()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or("Change approvals");
        let subtree = self.subtree(org_id).await?;
        let mut tx = begin_scoped(self.db(), &OrgScope::Orgs(subtree))
            .await
            .map_err(internal)?;
        let ctx = RequestContext::system(Via::Workflow);
        let change = huxley_servicedesk::changes::decide(
            &mut tx,
            &ctx,
            ticket,
            approved,
            by,
            input["comment"].as_str(),
        )
        .await
        .map_err(desk_error)?;
        tx.commit().await.map_err(internal)?;
        Ok(json!(change))
    }

    /// The scope a scoping step names, checked to be in the run's
    /// organization or below and the sales module on there.
    async fn scope_in(&self, org_id: Uuid, input: &Json) -> Result<Uuid, Json> {
        let mut tx = begin_scoped(self.db(), &OrgScope::System)
            .await
            .map_err(internal)?;
        let on = huxley_modules::is_active_at(&mut tx, huxley_sales::MODULE, org_id)
            .await
            .map_err(internal)?;
        if !on {
            return Err(error("module_disabled", "Sales is turned off"));
        }
        let scope = id(input, "scope_id")?.ok_or_else(|| error("input", "scope_id is required"))?;
        let under: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM sales_scopes s JOIN organizations so ON so.id = s.org_id \
               JOIN organizations o ON o.id = $2 WHERE s.id = $1 AND o.path @> so.path)",
        )
        .bind(scope)
        .bind(org_id)
        .fetch_one(&mut *tx)
        .await
        .map_err(internal)?;
        tx.commit().await.map_err(internal)?;
        if under {
            Ok(scope)
        } else {
            Err(error("not_found", "no such scope here"))
        }
    }

    /// `core.scope.context`: what drafting a scope reads.
    pub(crate) async fn scope_context(&self, org_id: Uuid, input: &Json) -> Result<Json, Json> {
        let scope = self.scope_in(org_id, input).await?;
        let mut tx = begin_scoped(self.db(), &OrgScope::System)
            .await
            .map_err(internal)?;
        let context = huxley_sales::scoping::context(&mut tx, scope)
            .await
            .map_err(internal)?;
        tx.commit().await.map_err(internal)?;
        Ok(json!(context))
    }

    /// `core.scope.write`: keeps a drafted Scope of Work (or that drafting failed).
    pub(crate) async fn scope_write(&self, org_id: Uuid, input: &Json) -> Result<Json, Json> {
        let scope = self.scope_in(org_id, input).await?;
        let mut tx = begin_scoped(self.db(), &OrgScope::System)
            .await
            .map_err(internal)?;
        if let Some(why) = input["error"].as_str().filter(|e| !e.trim().is_empty()) {
            huxley_sales::scoping::draft_failed(&mut tx, scope, why)
                .await
                .map_err(internal)?;
            tx.commit().await.map_err(internal)?;
            return Ok(json!({ "written": 0 }));
        }
        let draft: huxley_sales::scoping::Draft = serde_json::from_value(json!({
            "sections": input.get("sections").cloned().unwrap_or(Json::Null).as_array().cloned().unwrap_or_default(),
            "questions": input.get("questions").cloned().unwrap_or(Json::Null).as_array().cloned().unwrap_or_default(),
        }))
        .map_err(|e| error("input", format!("the draft isn't readable: {e}")))?;
        let ctx = RequestContext::system(Via::Workflow);
        let written = huxley_sales::scoping::write_draft(&mut tx, &ctx, scope, &draft)
            .await
            .map_err(internal)?;
        tx.commit().await.map_err(internal)?;
        Ok(json!({ "written": written }))
    }

    /// `core.sales.log_email`: logs a sales mailbox's email on the contacts
    /// and leads it's to or from.
    pub(crate) async fn sales_log_email(&self, org_id: Uuid, input: &Json) -> Result<Json, Json> {
        let mut tx = begin_scoped(self.db(), &OrgScope::System)
            .await
            .map_err(internal)?;
        let on = huxley_modules::is_active_at(&mut tx, huxley_sales::MODULE, org_id)
            .await
            .map_err(internal)?;
        if !on {
            return Err(error("module_disabled", "Sales is turned off"));
        }
        let email = id(input, "email_id")?.ok_or_else(|| error("input", "email_id is required"))?;
        let under: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM sd_inbound_emails e JOIN organizations eo ON eo.id = e.org_id \
               JOIN organizations o ON o.id = $2 WHERE e.id = $1 AND o.path @> eo.path)",
        )
        .bind(email)
        .bind(org_id)
        .fetch_one(&mut *tx)
        .await
        .map_err(internal)?;
        if !under {
            return Err(error("not_found", "no such email here"));
        }
        let ctx = RequestContext::system(Via::Workflow);
        let logged = huxley_sales::activities::log_email(&mut tx, &ctx, email)
            .await
            .map_err(|e| match e {
                huxley_sales::SalesError::NotFound => error("not_found", "no such email here"),
                other => internal(other),
            })?;
        tx.commit().await.map_err(internal)?;
        Ok(json!(logged))
    }

    /// `core.quote.review`: records a quote review's decision (on a quote
    /// in the run's organization or below). Output: the review state.
    pub(crate) async fn quote_review(&self, org_id: Uuid, input: &Json) -> Result<Json, Json> {
        let mut tx = begin_scoped(self.db(), &OrgScope::System)
            .await
            .map_err(internal)?;
        let on = huxley_modules::is_active_at(&mut tx, huxley_sales::MODULE, org_id)
            .await
            .map_err(internal)?;
        tx.commit().await.map_err(internal)?;
        if !on {
            return Err(error("module_disabled", "Sales is turned off"));
        }
        let review =
            id(input, "review_id")?.ok_or_else(|| error("input", "review_id is required"))?;
        let decision = match &input["decision"] {
            Json::Bool(true) => huxley_sales::review::Decision::Approved,
            Json::Bool(false) => huxley_sales::review::Decision::Rejected,
            Json::String(s) => huxley_sales::review::Decision::parse(s).ok_or_else(|| {
                error(
                    "input",
                    "decision is approved, changes_requested or rejected",
                )
            })?,
            _ => {
                return Err(error(
                    "input",
                    "decision is approved, changes_requested or rejected",
                ));
            }
        };
        let by = input["by"]
            .as_str()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or("Quote review");
        let subtree = self.subtree(org_id).await?;
        let mut tx = begin_scoped(self.db(), &OrgScope::Orgs(subtree))
            .await
            .map_err(internal)?;
        let ctx = RequestContext::system(Via::Workflow);
        let state = huxley_sales::review::decide(
            &mut tx,
            &ctx,
            review,
            decision,
            by,
            input["comment"].as_str(),
        )
        .await
        .map_err(|e| match e {
            huxley_sales::SalesError::NotFound => error("not_found", "no such quote review here"),
            huxley_sales::SalesError::Conflict(m) => error("conflict", &m),
            huxley_sales::SalesError::Invalid { field, message } => {
                error("input", format!("{field}: {message}"))
            }
            other => internal(other),
        })?;
        tx.commit().await.map_err(internal)?;
        Ok(json!(state))
    }

    /// `core.project.scope.decide`: records the decision on a submitted
    /// scope change (on a project in the run's organization or below).
    /// Output: the scope change.
    pub(crate) async fn scope_decide(&self, org_id: Uuid, input: &Json) -> Result<Json, Json> {
        self.require_projects(org_id).await?;
        let change = id(input, "scope_change_id")?
            .ok_or_else(|| error("input", "scope_change_id is required"))?;
        let approved = match &input["decision"] {
            Json::Bool(b) => *b,
            Json::String(s) if s == "approved" => true,
            Json::String(s) if s == "rejected" => false,
            _ => return Err(error("input", "decision is approved or rejected")),
        };
        let by = input["by"]
            .as_str()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or("Scope approvals");
        let subtree = self.subtree(org_id).await?;
        let mut tx = begin_scoped(self.db(), &OrgScope::Orgs(subtree))
            .await
            .map_err(internal)?;
        let ctx = RequestContext::system(Via::Workflow);
        let decided = huxley_servicedesk::project_scope::decide(
            &mut tx,
            &ctx,
            change,
            approved,
            by,
            input["comment"].as_str(),
        )
        .await
        .map_err(desk_error)?;
        tx.commit().await.map_err(internal)?;
        Ok(json!(decided))
    }

    /// The projects module, on at the run's organization.
    async fn require_projects(&self, org_id: Uuid) -> Result<(), Json> {
        self.require_desk(org_id).await?;
        let mut tx = begin_scoped(self.db(), &OrgScope::System)
            .await
            .map_err(internal)?;
        let on = huxley_modules::is_active_at(&mut tx, "projects", org_id)
            .await
            .map_err(internal)?;
        tx.commit().await.map_err(internal)?;
        if on {
            Ok(())
        } else {
            Err(error("module_disabled", "Projects are turned off"))
        }
    }

    /// `core.project.status`: what happened on one project, or on every
    /// active one in the run's organization and below.
    pub(crate) async fn project_status(&self, org_id: Uuid, input: &Json) -> Result<Json, Json> {
        self.require_projects(org_id).await?;
        let project = id(input, "project_id")?;
        let days = input["days"].as_i64().unwrap_or(7);
        let subtree = self.subtree(org_id).await?;
        let mut tx = begin_scoped(self.db(), &OrgScope::Orgs(subtree))
            .await
            .map_err(internal)?;
        let facts = huxley_servicedesk::project_status::active_facts(&mut tx, project, days)
            .await
            .map_err(desk_error)?;
        tx.commit().await.map_err(internal)?;
        if project.is_some() && facts.is_empty() {
            return Err(error("not_found", "There's no such project here"));
        }
        Ok(json!({ "projects": facts }))
    }

    /// `core.project.report`: saves a status report on a project.
    pub(crate) async fn project_report(&self, org_id: Uuid, input: &Json) -> Result<Json, Json> {
        self.require_projects(org_id).await?;
        let project =
            id(input, "project_id")?.ok_or_else(|| error("input", "project_id is required"))?;
        let text = |k: &str| {
            input[k]
                .as_str()
                .filter(|s| !s.trim().is_empty())
                .map(str::to_owned)
        };
        let report = huxley_servicedesk::project_status::NewStatusReport {
            body: text("body").ok_or_else(|| error("input", "body is required"))?,
            client_visible: match &input["client_visible"] {
                Json::Bool(b) => *b,
                Json::String(s) => s == "true",
                _ => false,
            },
            period_start: text("period_start"),
            period_end: text("period_end"),
        };
        let author = text("author").unwrap_or_else(|| "AI status reports".into());
        let subtree = self.subtree(org_id).await?;
        let mut tx = begin_scoped(self.db(), &OrgScope::Orgs(subtree))
            .await
            .map_err(internal)?;
        let ctx = RequestContext::system(Via::Workflow);
        let saved =
            huxley_servicedesk::project_status::save(&mut tx, &ctx, project, &report, &author)
                .await
                .map_err(desk_error)?;
        tx.commit().await.map_err(internal)?;
        Ok(json!(saved))
    }
}
