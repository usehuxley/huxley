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

impl Engine {
    /// `core.report.send`: a shared or prebuilt dashboard, with the numbers
    /// of the run's organization and below, emailed as a CSV, XLSX or PDF
    /// to people (`user:`, `team:` or `role:` entries). Output: `{ sent,
    /// file_name }`.
    pub(crate) async fn report_send(&self, org_id: Uuid, input: &Json) -> Result<Json, Json> {
        use huxley_analytics::export;
        let mailer = self
            .mailer
            .as_ref()
            .filter(|m| m.enabled())
            .ok_or_else(|| error("email_disabled", "Email isn't set up on this instance"))?;
        let format = export::Format::parse(input["format"].as_str().unwrap_or("pdf"))
            .ok_or_else(|| error("input", "format is csv, xlsx or pdf"))?;
        let key = input["dashboard_id"]
            .as_str()
            .unwrap_or_default()
            .trim()
            .to_owned();
        let subtree = self.subtree(org_id).await?;
        let mut tx = begin_scoped(self.db(), &OrgScope::System)
            .await
            .map_err(internal)?;
        let dashboard = report_dashboard(&mut tx, &key, org_id).await?;
        let range = input["range"]
            .as_str()
            .filter(|r| huxley_analytics::query::ranges().any(|k| k == *r))
            .map_or_else(|| dashboard.range.clone(), str::to_owned);
        let scope = huxley_analytics::Scope::Orgs(subtree);
        let mut resolved = export::Resolved::new();
        for source_key in export::sources(&dashboard) {
            let found = if huxley_analytics::Source::is_dataset(&source_key) {
                huxley_analytics::datasets::source(&mut tx, &scope, &source_key)
                    .await
                    .map_err(internal)?
            } else {
                huxley_analytics::catalog::source(&source_key).cloned()
            };
            resolved.insert(
                source_key,
                found
                    .map(|s| (s, scope.clone()))
                    .ok_or_else(|| "No such source here.".to_owned()),
            );
        }
        let users = crate::notify::recipients(&mut tx, org_id, &input["to"])
            .await
            .map_err(internal)??;
        let emails: Vec<String> =
            sqlx::query_scalar("SELECT email FROM users WHERE id = ANY($1) AND status = 'active'")
                .bind(&users)
                .fetch_all(&mut *tx)
                .await
                .map_err(internal)?;
        tx.commit().await.map_err(internal)?;
        let cache = huxley_analytics::Cache::default();
        let tables = export::tables(self.db(), &cache, &dashboard, &range, &resolved).await;
        let bytes = export::render(format, &dashboard.name, &range, &tables).map_err(internal)?;
        let file_name = export::file_name(&dashboard.name, format);
        let subject = input["subject"]
            .as_str()
            .filter(|s| !s.trim().is_empty())
            .map_or_else(
                || format!("{} ({})", dashboard.name, export::range_label(&range)),
                str::to_owned,
            );
        let message = input["message"].as_str().unwrap_or_default();
        let text = format!("{message}\n\nAttached: {file_name}.")
            .trim()
            .to_owned();
        let html = format!(
            "<p>{}</p><p>Attached: {file_name}.</p>",
            message.replace('&', "&amp;").replace('<', "&lt;")
        );
        let sent = send_all(
            mailer,
            &emails,
            &subject,
            (&text, &html),
            (&file_name, format, &bytes),
        )
        .await?;
        Ok(json!({ "sent": sent, "file_name": file_name }))
    }
}

/// The dashboard a report is of: prebuilt by key, or shared at the run's
/// organization, above it or below it.
async fn report_dashboard(
    tx: &mut sqlx::PgConnection,
    key: &str,
    org_id: Uuid,
) -> Result<huxley_analytics::dashboards::Dashboard, Json> {
    let found = match key.parse::<Uuid>() {
        Err(_) => huxley_analytics::dashboards::builtin()
            .into_iter()
            .find(|d| d.id == key),
        Ok(id) => {
            // Shared at the run's organization, above it or below it.
            let related: bool = sqlx::query_scalar(
                "SELECT EXISTS (SELECT 1 FROM dashboards d JOIN organizations o ON o.id = d.org_id \
                  JOIN organizations r ON r.id = $2 \
                  WHERE d.id = $1 AND d.owner_user_id IS NULL AND (o.path @> r.path OR o.path <@ r.path))",
            )
            .bind(id)
            .bind(org_id)
            .fetch_one(&mut *tx)
            .await
            .map_err(internal)?;
            if related {
                huxley_analytics::dashboards::get(
                    &mut *tx,
                    &huxley_analytics::dashboards::Viewer {
                        manager: true,
                        ..Default::default()
                    },
                    id,
                )
                .await
                .map_err(internal)?
            } else {
                None
            }
        }
    };
    found.ok_or_else(|| error("not_found", "No such shared dashboard here"))
}

/// Emails the report to each address. Returns how many were sent.
async fn send_all(
    mailer: &huxley_email::Mailer,
    emails: &[String],
    subject: &str,
    (text, html): (&str, &str),
    (file_name, format, bytes): (&str, huxley_analytics::export::Format, &[u8]),
) -> Result<usize, Json> {
    let mut sent = 0;
    for to in emails {
        mailer
            .send(huxley_email::Email {
                to: to.clone(),
                subject: subject.to_owned(),
                text: text.to_owned(),
                html: html.to_owned(),
                reply: None,
                attachments: vec![huxley_email::Attachment {
                    filename: file_name.to_owned(),
                    content_type: format.content_type().to_owned(),
                    bytes: bytes.to_vec(),
                }],
            })
            .await
            .map_err(|e| error("email", e.to_string()))?;
        sent += 1;
    }
    Ok(sent)
}
