//! The auto-documentation steps (ADR-051): `core.kb.draft` drafts a
//! knowledge article (into review) from a ticket, and `core.time.suggest`
//! suggests time for whoever resolved it.

use huxley_core::{RequestContext, Via};
use huxley_db::{OrgScope, begin_scoped};
use serde_json::{Value as Json, json};
use uuid::Uuid;

use crate::Engine;
use crate::walk::error;

fn internal(e: impl std::fmt::Display) -> Json {
    error("internal", e.to_string())
}

fn id(input: &Json, key: &str) -> Result<Option<Uuid>, Json> {
    match input[key].as_str().map(str::trim).filter(|s| !s.is_empty()) {
        None => Ok(None),
        Some(s) => s
            .parse()
            .map(Some)
            .map_err(|_| error("input", format!("{key} isn't an id"))),
    }
}

impl Engine {
    /// `core.kb.draft`: a new article at `org_id` (default the run's), in
    /// review unless `submit` is false, linked to `ticket_id` if given.
    /// Output: `{ article_id, status }`.
    pub(crate) async fn kb_draft(&self, org_id: Uuid, input: &Json) -> Result<Json, Json> {
        let at = id(input, "org_id")?.unwrap_or(org_id);
        let subtree = self.subtree(org_id).await?;
        if !subtree.contains(&at) {
            return Err(error("input", "org_id is outside the run's organization"));
        }
        let mut tx = begin_scoped(self.db(), &OrgScope::System)
            .await
            .map_err(internal)?;
        if !huxley_modules::is_active_at(&mut tx, huxley_knowledge::MODULE, at)
            .await
            .map_err(internal)?
        {
            return Err(error(
                "module_disabled",
                "The knowledge base is turned off for this organization",
            ));
        }
        let ctx = RequestContext::system(Via::Workflow);
        let kb = |e: huxley_knowledge::KbError| error("input", e.to_string());
        let article = huxley_knowledge::articles::create(
            &mut tx,
            &ctx,
            &huxley_knowledge::articles::ArticleInput {
                org_id: at,
                title: input["title"].as_str().unwrap_or_default().to_owned(),
                body: input["body"].as_str().map(str::to_owned),
                category_id: None,
                visibility: None,
                tags: Some(vec!["ai-draft".into()]),
            },
        )
        .await
        .map_err(kb)?;
        let id_ = article.summary.id;
        let mut status = article.summary.status.clone();
        if input["submit"].as_bool().unwrap_or(true) {
            status = huxley_knowledge::articles::submit(&mut tx, &ctx, id_)
                .await
                .map_err(kb)?
                .summary
                .status;
        }
        if let Some(ticket) = id(input, "ticket_id")? {
            huxley_knowledge::links::link(&mut tx, &ctx, id_, ticket)
                .await
                .map_err(kb)?;
        }
        tx.commit().await.map_err(internal)?;
        Ok(json!({ "article_id": id_, "status": status }))
    }

    /// `core.time.suggest`: suggests `minutes` on a ticket (in the run's
    /// organization or below) for `user_id` (default its assignee), unless
    /// they logged time on it already. Output: `{ suggestion }` (none:
    /// skipped).
    pub(crate) async fn time_suggest(&self, org_id: Uuid, input: &Json) -> Result<Json, Json> {
        self.require_desk(org_id).await?;
        let ticket =
            id(input, "ticket_id")?.ok_or_else(|| error("input", "ticket_id is required"))?;
        let minutes = input["minutes"]
            .as_i64()
            .or_else(|| {
                input["minutes"]
                    .as_str()
                    .and_then(|s| s.trim().parse().ok())
            })
            .and_then(|m| i32::try_from(m).ok())
            .ok_or_else(|| error("input", "minutes is 1 to 1440"))?;
        let subtree = self.subtree(org_id).await?;
        let mut tx = begin_scoped(self.db(), &OrgScope::Orgs(subtree))
            .await
            .map_err(internal)?;
        let made = huxley_servicedesk::suggestions::suggest(
            &mut tx,
            ticket,
            id(input, "user_id")?,
            minutes,
            input["notes"].as_str().unwrap_or_default(),
        )
        .await
        .map_err(|e| match e {
            huxley_servicedesk::DeskError::NotFound => error("not_found", "No such ticket here"),
            huxley_servicedesk::DeskError::Invalid { field, message } => {
                error("input", format!("{field}: {message}"))
            }
            other => internal(other),
        })?;
        tx.commit().await.map_err(internal)?;
        Ok(json!({ "suggestion": made }))
    }
}
