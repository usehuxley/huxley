//! The `crm` module's steps (`core.contact.*`, `core.company.*`). Like the
//! dataset steps, a run works with its own organization and those below it,
//! and secrets don't go into records. Each step needs the module active at
//! the organization it works on.

use huxley_core::{RequestContext, Via};
use huxley_crm::CrmError;
use huxley_crm::companies::{self, CompanyInput};
use huxley_crm::contacts::{self, ContactInput};
use huxley_db::{OrgScope, begin_scoped};
use serde_json::{Map, Value as Json, json};
use uuid::Uuid;

use crate::Engine;
use crate::walk::error;

fn crm_error(e: CrmError) -> Json {
    match e {
        CrmError::NotFound => error("not_found", "not found"),
        CrmError::Invalid { field, message } => error("input", format!("{field}: {message}")),
        CrmError::VersionConflict => error("conflict", "it changed while the step ran; retry it"),
        CrmError::Db(e) => error("internal", e.to_string()),
        CrmError::Sqlx(e) => error("internal", e.to_string()),
    }
}

fn internal(e: impl std::fmt::Display) -> Json {
    error("internal", e.to_string())
}

/// An optional text input: a string, a number (as text), or nothing.
fn text(input: &Json, field: &str) -> Result<Option<String>, Json> {
    match &input[field] {
        Json::Null => Ok(None),
        Json::String(s) => Ok(Some(s.clone())),
        Json::Number(n) => Ok(Some(n.to_string())),
        _ => Err(error("input", format!("{field} is text"))),
    }
}

/// An external id: `{ <system>: <id> }` with one entry.
fn external_id(input: &Json) -> Result<Option<(String, String)>, Json> {
    match &input["external_id"] {
        Json::Null => Ok(None),
        Json::Object(map) if map.len() == 1 => {
            let (system, id) = map.iter().next().expect("one entry");
            let id = id
                .as_str()
                .ok_or_else(|| error("input", "external_id's value is text"))?;
            Ok(Some((system.clone(), id.to_owned())))
        }
        _ => Err(error("input", "external_id is one { system: id } pair")),
    }
}

fn context() -> RequestContext {
    RequestContext::system(Via::Workflow)
}

impl Engine {
    /// Fails unless the `crm` module is active at `org_id`.
    async fn require_crm(&self, org_id: Uuid) -> Result<(), Json> {
        let mut tx = begin_scoped(self.db(), &OrgScope::System)
            .await
            .map_err(internal)?;
        let active = huxley_modules::is_active_at(&mut tx, huxley_crm::MODULE, org_id)
            .await
            .map_err(internal)?;
        tx.commit().await.map_err(internal)?;
        if active {
            Ok(())
        } else {
            Err(error(
                "module_disabled",
                "Clients and contacts (the crm module) is turned off for this organization",
            ))
        }
    }

    /// The organization a step works on (`org_id`, or the run's), the
    /// run's subtree, after checking the module is on there.
    async fn crm_target(&self, org_id: Uuid, input: &Json) -> Result<(Uuid, Vec<Uuid>), Json> {
        let subtree = self.subtree(org_id).await?;
        let target = Self::org_in(input, &subtree)?.unwrap_or(org_id);
        self.require_crm(target).await?;
        Ok((target, subtree))
    }

    /// Runs one of the `crm` module's steps.
    pub(crate) async fn crm_step(
        &self,
        step_type: &str,
        org_id: Uuid,
        input: &Json,
        tainted: bool,
    ) -> Result<Json, Json> {
        let writes = matches!(step_type, "core.contact.upsert" | "core.company.update");
        if writes && tainted {
            return Err(error(
                "input",
                "contacts and company details can't contain secrets or values derived from them",
            ));
        }
        let (target, subtree) = self.crm_target(org_id, input).await?;
        let mut tx = begin_scoped(self.db(), &OrgScope::Orgs(subtree))
            .await
            .map_err(internal)?;
        let output = match step_type {
            "core.contact.find" => find(&mut tx, input).await,
            "core.contact.upsert" => upsert(&mut tx, target, input).await,
            "core.company.get" => company(&mut tx, target).await,
            "core.company.update" => update_company(&mut tx, target, input).await,
            other => Err(error("unsupported", format!("{other} isn't a crm step"))),
        }?;
        tx.commit().await.map_err(internal)?;
        Ok(output)
    }
}

async fn find(tx: &mut sqlx::PgConnection, input: &Json) -> Result<Json, Json> {
    let email = text(input, "email")?;
    let external = external_id(input)?;
    if email.is_none() && external.is_none() {
        return Err(error("input", "give an email, an external_id, or both"));
    }
    let found = contacts::search(
        tx,
        email.as_deref(),
        external.as_ref().map(|(s, i)| (s.as_str(), i.as_str())),
        100,
    )
    .await
    .map_err(crm_error)?;
    Ok(json!({ "total": found.len(), "contacts": found }))
}

async fn upsert(tx: &mut sqlx::PgConnection, target: Uuid, input: &Json) -> Result<Json, Json> {
    let match_on = text(input, "match")?.unwrap_or_else(|| "email".into());
    let email = text(input, "email")?;
    let mut external_ids: Map<String, Json> = match &input["external_ids"] {
        Json::Null => Map::new(),
        Json::Object(map) => map.clone(),
        _ => {
            return Err(error(
                "input",
                "external_ids is an object of { system: id }",
            ));
        }
    };
    let (by_email, by_external) = if match_on == "email" {
        let email = email
            .as_deref()
            .ok_or_else(|| error("input", "matching on email needs an email"))?;
        (Some(email.to_owned()), None)
    } else {
        let id = external_ids
            .get(&match_on)
            .and_then(Json::as_str)
            .ok_or_else(|| {
                error(
                    "input",
                    format!("matching on {match_on} needs external_ids.{match_on}"),
                )
            })?;
        (None, Some((match_on.clone(), id.to_owned())))
    };
    let existing = contacts::find_one(
        tx,
        target,
        by_email.as_deref(),
        by_external.as_ref().map(|(s, i)| (s.as_str(), i.as_str())),
    )
    .await
    .map_err(crm_error)?;
    let ctx = context();
    let given = |field: &str| text(input, field);
    let (contact, created) = if let Some(id) = existing {
        {
            let current = contacts::get(tx, id).await.map_err(crm_error)?;
            // Fields left out keep their values; external ids are merged.
            if let Json::Object(have) = &current.external_ids {
                for (k, v) in have {
                    external_ids.entry(k.clone()).or_insert_with(|| v.clone());
                }
            }
            let job_title = current
                .companies
                .iter()
                .find(|c| c.is_primary)
                .and_then(|c| c.job_title.clone());
            let update = ContactInput {
                first_name: given("first_name")?.unwrap_or(current.first_name.clone()),
                last_name: given("last_name")?.unwrap_or(current.last_name.clone()),
                email: given("email")?.or(current.email.clone()),
                phone: given("phone")?.or(current.phone.clone()),
                mobile: given("mobile")?.or(current.mobile.clone()),
                job_title: given("job_title")?.or(job_title),
                external_ids: Some(Json::Object(external_ids)),
                site_id: current.site_id,
                version: Some(current.version),
            };
            (
                contacts::update(tx, &ctx, id, &update)
                    .await
                    .map_err(crm_error)?,
                false,
            )
        }
    } else {
        {
            let create = ContactInput {
                first_name: given("first_name")?.unwrap_or_default(),
                last_name: given("last_name")?.unwrap_or_default(),
                email,
                phone: given("phone")?,
                mobile: given("mobile")?,
                job_title: given("job_title")?,
                external_ids: Some(Json::Object(external_ids)),
                site_id: None,
                version: None,
            };
            (
                contacts::create(tx, &ctx, target, &create)
                    .await
                    .map_err(crm_error)?,
                true,
            )
        }
    };
    Ok(json!({ "contact": contact, "created": created }))
}

async fn company(tx: &mut sqlx::PgConnection, target: Uuid) -> Result<Json, Json> {
    let details = companies::get(tx, target).await.map_err(crm_error)?;
    let sites = huxley_crm::sites::list(tx, target, false)
        .await
        .map_err(crm_error)?;
    Ok(json!({ "company": details, "sites": sites }))
}

async fn update_company(
    tx: &mut sqlx::PgConnection,
    target: Uuid,
    input: &Json,
) -> Result<Json, Json> {
    let current = companies::get(tx, target).await.map_err(crm_error)?;
    let change = CompanyInput {
        relationship: text(input, "relationship")?.unwrap_or(current.relationship.clone()),
        status: text(input, "status")?.unwrap_or(current.status.clone()),
        website: text(input, "website")?.or(current.website.clone()),
        phone: text(input, "phone")?.or(current.phone.clone()),
        notes: text(input, "notes")?.or(current.notes.clone()),
        version: current.version,
    };
    let saved = companies::update(tx, &context(), target, &change)
        .await
        .map_err(crm_error)?;
    Ok(json!({ "company": saved }))
}
