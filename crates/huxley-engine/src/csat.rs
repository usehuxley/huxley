//! The `cx` module's survey step (`core.csat.survey`, ADR-043).

use std::fmt::Write as _;

use huxley_core::{RequestContext, Via};
use huxley_cx::surveys::{self, Started};
use huxley_db::{OrgScope, begin_scoped};
use serde_json::{Value as Json, json};
use uuid::Uuid;

use crate::Engine;
use crate::walk::error;

fn internal(e: impl std::fmt::Display) -> Json {
    error("internal", e.to_string())
}

/// The rating links for a scale: `(rating, label)`.
fn choices(scale: &str) -> Vec<(i16, &'static str)> {
    if scale == "thumbs" {
        vec![(5, "👍 Good"), (1, "👎 Not good")]
    } else {
        vec![
            (5, "5 · Great"),
            (4, "4 · Good"),
            (3, "3 · OK"),
            (2, "2 · Poor"),
            (1, "1 · Bad"),
        ]
    }
}

/// The survey email: a rating link per choice (`{url}?rating=…`).
fn email(
    to: &str,
    first_name: &str,
    reference: &str,
    title: &str,
    url: &str,
    scale: &str,
) -> huxley_email::Email {
    let esc = huxley_email::escape_html;
    let links: Vec<(String, &str)> = choices(scale)
        .into_iter()
        .map(|(r, label)| (format!("{url}?rating={r}"), label))
        .collect();
    let greeting = if first_name.is_empty() {
        "Hello,".to_owned()
    } else {
        format!("Hello {first_name},")
    };
    let question =
        format!("Your ticket {reference}, \u{201c}{title}\u{201d}, is resolved. How did we do?");
    let text = format!(
        "{greeting}\n\n{question}\n\n{}\n\nOne click answers; you can add a comment on the page it opens.\n",
        links
            .iter()
            .map(|(l, label)| format!("{label}: {l}"))
            .collect::<Vec<_>>()
            .join("\n")
    );
    let mut buttons = String::new();
    for (l, label) in &links {
        let _ = write!(
            buttons,
            "<a href=\"{}\" style=\"display:inline-block;margin:0 6px 6px 0;padding:8px 12px;border:1px solid #d0d7de;\
             border-radius:6px;color:#1f2328;text-decoration:none\">{}</a>",
            esc(l),
            esc(label)
        );
    }
    let html = format!(
        "<!doctype html><html><body style=\"font-family:system-ui,sans-serif;color:#1f2328;line-height:1.5\">\
         <p>{}</p><p>{}</p><p>{buttons}</p>\
         <p style=\"color:#57606a;font-size:13px\">One click answers; you can add a comment on the page it opens.</p>\
         </body></html>",
        esc(&greeting),
        esc(&question)
    );
    huxley_email::Email {
        to: to.to_owned(),
        subject: format!("How did we do? {reference}"),
        text,
        html,
        reply: None,
        attachments: Vec::new(),
    }
}

impl Engine {
    /// `core.csat.survey`: asks a resolved ticket's requester how it went,
    /// emailing one-click rating links when email and a portal address are
    /// set up. Output: `{ sent, skipped, survey, emailed }`.
    pub(crate) async fn csat_survey(&self, org_id: Uuid, input: &Json) -> Result<Json, Json> {
        let ticket = input["ticket"]
            .as_str()
            .and_then(|t| t.parse::<Uuid>().ok())
            .ok_or_else(|| error("input", "ticket is a ticket's id"))?;
        let subtree = self.subtree(org_id).await?;
        let mut tx = begin_scoped(self.db(), &OrgScope::Orgs(subtree))
            .await
            .map_err(internal)?;
        let found = huxley_servicedesk::tickets::get(&mut tx, ticket)
            .await
            .map_err(|_| error("not_found", "there's no such ticket here"))?;
        tx.commit().await.map_err(internal)?;
        let mut tx = begin_scoped(self.db(), &OrgScope::System)
            .await
            .map_err(internal)?;
        if !huxley_modules::is_active_at(&mut tx, huxley_cx::MODULE, found.org_id)
            .await
            .map_err(internal)?
        {
            return Err(error(
                "module_disabled",
                "The customer experience module is turned off",
            ));
        }
        let roll =
            f64::from(u16::try_from(Uuid::new_v4().as_u128() % 10_000).unwrap_or(0)) / 10_000.0;
        let ctx = RequestContext::system(Via::Workflow);
        let started = surveys::start(&mut tx, &ctx, ticket, roll)
            .await
            .map_err(internal)?;
        let Started::Survey { survey, token, to } = started else {
            let Started::Skipped(reason) = started else {
                unreachable!()
            };
            tx.commit().await.map_err(internal)?;
            return Ok(
                json!({ "sent": false, "skipped": reason, "survey": null, "emailed": false }),
            );
        };
        let mut emailed = false;
        let mailer = self.mailer.as_ref().filter(|m| m.enabled());
        let base = match (mailer, to.email.as_deref()) {
            (Some(_), Some(_)) => self
                .portal_base(&mut tx, survey.org_id)
                .await
                .map_err(internal)?,
            _ => None,
        };
        if let (Some(mailer), Some(address), Some(base)) = (mailer, to.email.as_deref(), base) {
            let url = format!("{}/survey/{token}", base.trim_end_matches('/'));
            mailer.send_later(email(
                address,
                &to.first_name,
                &survey.reference,
                &survey.title,
                &url,
                &survey.scale,
            ));
            surveys::mark_emailed(&mut tx, survey.id)
                .await
                .map_err(internal)?;
            emailed = true;
        }
        tx.commit().await.map_err(internal)?;
        Ok(json!({ "sent": true, "skipped": null, "survey": survey.id, "emailed": emailed }))
    }
}

/// The NPS email: a link per score, 0 to 10 (`{url}?score=…`).
fn nps_email(to: &str, first_name: &str, company: &str, url: &str) -> huxley_email::Email {
    let esc = huxley_email::escape_html;
    let greeting = if first_name.is_empty() {
        "Hello,".to_owned()
    } else {
        format!("Hello {first_name},")
    };
    let question = "How likely are you to recommend us to a friend or colleague? 0 is not at all, 10 is extremely.";
    let text = format!(
        "{greeting}\n\n{question}\n\n{}\n\nOne click answers; you can add a comment on the page it opens.\n",
        (0..=10)
            .map(|n| format!("{n}: {url}?score={n}"))
            .collect::<Vec<_>>()
            .join("\n")
    );
    let mut buttons = String::new();
    for n in 0..=10 {
        let _ = write!(
            buttons,
            "<a href=\"{}\" style=\"display:inline-block;min-width:28px;margin:0 4px 6px 0;padding:8px 0;text-align:center;\
             border:1px solid #d0d7de;border-radius:6px;color:#1f2328;text-decoration:none\">{n}</a>",
            esc(&format!("{url}?score={n}"))
        );
    }
    let html = format!(
        "<!doctype html><html><body style=\"font-family:system-ui,sans-serif;color:#1f2328;line-height:1.5\">\
         <p>{}</p><p>{}</p><p>{buttons}</p>\
         <p style=\"color:#57606a;font-size:13px\">One click answers; you can add a comment on the page it opens.</p>\
         </body></html>",
        esc(&greeting),
        esc(question)
    );
    huxley_email::Email {
        to: to.to_owned(),
        subject: format!("A quick question from {company}"),
        text,
        html,
        reply: None,
        attachments: Vec::new(),
    }
}

impl Engine {
    /// `core.nps.send`: asks contacts under the run's organization how likely
    /// they are to recommend you (those who raised a ticket lately and
    /// weren't asked lately; companies can opt out). Output: `{ asked,
    /// emailed, campaign }`.
    pub(crate) async fn nps_send(&self, org_id: Uuid, input: &Json) -> Result<Json, Json> {
        let limit = input["limit"].as_i64().unwrap_or(500).clamp(1, 5000);
        let campaign = input["campaign"]
            .as_str()
            .map(str::trim)
            .filter(|c| !c.is_empty() && c.len() <= 60)
            .map_or_else(
                || {
                    let today = time::OffsetDateTime::now_utc().date();
                    format!("{}-{:02}", today.year(), u8::from(today.month()))
                },
                str::to_owned,
            );
        let mut tx = begin_scoped(self.db(), &OrgScope::System)
            .await
            .map_err(internal)?;
        if !huxley_modules::is_active_at(&mut tx, huxley_cx::MODULE, org_id)
            .await
            .map_err(internal)?
        {
            return Err(error(
                "module_disabled",
                "The customer experience module is turned off",
            ));
        }
        let candidates = huxley_cx::nps::candidates(&mut tx, org_id, limit)
            .await
            .map_err(internal)?;
        let ctx = RequestContext::system(Via::Workflow);
        let mailer = self.mailer.as_ref().filter(|m| m.enabled());
        let (mut asked, mut emailed) = (0, 0);
        for who in &candidates {
            let (survey, token) = huxley_cx::nps::start(&mut tx, &ctx, who, &campaign)
                .await
                .map_err(internal)?;
            asked += 1;
            let Some(mailer) = mailer else { continue };
            let Some(base) = self
                .portal_base(&mut tx, survey.org_id)
                .await
                .map_err(internal)?
            else {
                continue;
            };
            let company = huxley_orgs::settings::value(
                &mut tx,
                survey.org_id,
                huxley_orgs::settings::BRAND_NAME,
            )
            .await
            .ok()
            .flatten()
            .unwrap_or_else(|| "your service desk".to_owned());
            let url = format!("{}/nps/{token}", base.trim_end_matches('/'));
            mailer.send_later(nps_email(&who.email, &who.first_name, &company, &url));
            huxley_cx::nps::mark_emailed(&mut tx, survey.id)
                .await
                .map_err(internal)?;
            emailed += 1;
        }
        tx.commit().await.map_err(internal)?;
        Ok(json!({ "asked": asked, "emailed": emailed, "campaign": campaign }))
    }
}

impl Engine {
    /// `core.health.compute`: scores the client companies in the run's
    /// organization and below for today. Output: `{ companies, declining:
    /// [{ org_id, name, score, previous, reasons }], sentiment_falling:
    /// [{ org_id, name, recent, previous, explanations }] }`.
    pub(crate) async fn health_compute(&self, org_id: Uuid) -> Result<Json, Json> {
        let mut tx = begin_scoped(self.db(), &OrgScope::System)
            .await
            .map_err(internal)?;
        if !huxley_modules::is_active_at(&mut tx, huxley_cx::MODULE, org_id)
            .await
            .map_err(internal)?
        {
            return Err(error(
                "module_disabled",
                "The customer experience module is turned off",
            ));
        }
        let ctx = RequestContext::system(Via::Workflow);
        let today = time::OffsetDateTime::now_utc().date();
        let computed = huxley_cx::health::compute(&mut tx, &ctx, org_id, today)
            .await
            .map_err(internal)?;
        tx.commit().await.map_err(internal)?;
        Ok(json!(computed))
    }
}

impl Engine {
    /// `core.sentiment.record`: stores a client message's sentiment (a
    /// ticket in the run's organization or below). Output: `{ id, org_id,
    /// ticket_id, score }`.
    pub(crate) async fn sentiment_record(&self, org_id: Uuid, input: &Json) -> Result<Json, Json> {
        let id = |key: &str| -> Result<Option<Uuid>, Json> {
            match input[key].as_str().map(str::trim).filter(|s| !s.is_empty()) {
                None => Ok(None),
                Some(s) => s
                    .parse()
                    .map(Some)
                    .map_err(|_| error("input", format!("{key} isn't an id"))),
            }
        };
        let ticket = id("ticket_id")?.ok_or_else(|| error("input", "ticket_id is required"))?;
        let note = id("note_id")?;
        let score = input["score"]
            .as_i64()
            .or_else(|| input["score"].as_str().and_then(|s| s.trim().parse().ok()))
            .ok_or_else(|| error("input", "score is -2 to 2"))?;
        let reason = input["reason"].as_str().unwrap_or_default();
        let subtree = self.subtree(org_id).await?;
        let mut tx = begin_scoped(self.db(), &OrgScope::System)
            .await
            .map_err(internal)?;
        if !huxley_modules::is_active_at(&mut tx, huxley_cx::MODULE, org_id)
            .await
            .map_err(internal)?
        {
            return Err(error(
                "module_disabled",
                "The customer experience module is turned off",
            ));
        }
        tx.commit().await.map_err(internal)?;
        let mut tx = begin_scoped(self.db(), &OrgScope::Orgs(subtree))
            .await
            .map_err(internal)?;
        let entry = huxley_cx::sentiment::record(&mut tx, ticket, note, score, reason)
            .await
            .map_err(|e| match e {
                huxley_cx::CxError::NotFound => error("not_found", "No such ticket here"),
                huxley_cx::CxError::Invalid { field, message } => {
                    error("input", format!("{field}: {message}"))
                }
                other => internal(other),
            })?;
        tx.commit().await.map_err(internal)?;
        Ok(
            json!({ "id": entry.id, "org_id": entry.org_id, "ticket_id": entry.ticket_id, "score": entry.score }),
        )
    }
}
