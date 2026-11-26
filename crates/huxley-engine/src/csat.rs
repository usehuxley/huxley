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
