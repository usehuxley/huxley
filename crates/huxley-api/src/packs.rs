//! Installing packs (ADR-028): what a pack would add to an organization
//! (the review), and adding it. Upgrading a pack replaces only what it
//! installed; anything else with the same key is a conflict, never
//! overwritten. Workflows arrive as drafts: nothing is published.

use std::collections::{BTreeMap, BTreeSet};

use huxley_core::RequestContext;
use huxley_packs::{Pack, Trust};
use serde::{Deserialize, Serialize};
use serde_json::{Value as Json, json};
use sqlx::PgConnection;
use utoipa::ToSchema;
use uuid::Uuid;

/// The kinds of things a pack adds.
pub const KINDS: [&str; 6] = [
    "datasets",
    "integrations",
    "workflows",
    "forms",
    "scripts",
    "templates",
];

/// What installing would do to one item.
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ReviewItem {
    pub kind: String,
    pub key: String,
    /// `add`, `update` (installed by this pack before) or `conflict`
    /// (something else has this key: installing is refused).
    pub action: String,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ReviewScript {
    pub key: String,
    pub language: String,
}

/// What a pack would add, for an admin to approve.
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Review {
    pub upload_id: Uuid,
    pub key: String,
    pub name: String,
    pub version: String,
    pub description: String,
    pub publisher: String,
    pub trust: Trust,
    /// The version already installed in this organization, if any.
    pub installed_version: Option<String>,
    pub items: Vec<ReviewItem>,
    /// Hosts its integrations and HTTP steps call.
    pub egress: Vec<String>,
    pub scripts: Vec<ReviewScript>,
}

/// What an install created or updated, by kind then key.
pub type Items = BTreeMap<String, BTreeMap<String, Uuid>>;

#[derive(Debug, thiserror::Error)]
pub enum InstallError {
    #[error("{0}")]
    Content(String),
    #[error("{kind} {key:?} already exists here and wasn't installed by this pack")]
    Conflict { kind: String, key: String },
    #[error(transparent)]
    Db(#[from] sqlx::Error),
}

fn keys<'a>(pack: &'a Pack, kind: &str) -> &'a [String] {
    let c = &pack.manifest.contents;
    match kind {
        "integrations" => &c.integrations,
        "workflows" => &c.workflows,
        "forms" => &c.forms,
        "scripts" => &c.scripts,
        "datasets" => &c.datasets,
        _ => &c.templates,
    }
}

/// The id of the item of `kind` with `key` in `org_id`, if there is one.
pub(crate) async fn existing(
    conn: &mut PgConnection,
    org_id: Uuid,
    kind: &str,
    key: &str,
) -> sqlx::Result<Option<Uuid>> {
    let table = match kind {
        "integrations" => "integrations",
        "workflows" => "workflows",
        "forms" => "forms",
        "scripts" => "scripts",
        "datasets" => "datasets",
        // Templates have no key: a pack's own are tracked by id.
        _ => return Ok(None),
    };
    sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "SELECT id FROM {table} WHERE org_id = $1 AND key = $2"
    )))
    .bind(org_id)
    .bind(key)
    .fetch_optional(conn)
    .await
}

/// The pack's earlier install in `org_id`: its version and items.
async fn installed(
    conn: &mut PgConnection,
    org_id: Uuid,
    pack_key: &str,
) -> sqlx::Result<Option<(String, Items)>> {
    let row: Option<(String, Json)> = sqlx::query_as(
        "SELECT version, items FROM pack_installs WHERE org_id = $1 AND pack_key = $2",
    )
    .bind(org_id)
    .bind(pack_key)
    .fetch_optional(conn)
    .await?;
    Ok(row.map(|(version, items)| (version, serde_json::from_value(items).unwrap_or_default())))
}

fn host(url: &str) -> Option<String> {
    let rest = url.split_once("://")?.1;
    let host = rest.split(['/', '?', '#']).next()?.rsplit('@').next()?;
    let host = host.split(':').next()?;
    (!host.is_empty() && !host.contains("{{")).then(|| host.to_ascii_lowercase())
}

/// Collects the hosts of literal `http.request` URLs in a document.
fn walk(value: &Json, hosts: &mut BTreeSet<String>) {
    match value {
        Json::Object(map) => {
            if map.get("type") == Some(&json!("http.request"))
                && let Some(url) = map.get("with").and_then(|w| w["url"].as_str())
            {
                hosts.extend(host(url));
            }
            map.values().for_each(|v| walk(v, hosts));
        }
        Json::Array(items) => items.iter().for_each(|v| walk(v, hosts)),
        _ => {}
    }
}

/// Hosts the pack's integrations and literal `http.request` URLs reach.
fn egress(pack: &Pack) -> Vec<String> {
    let mut hosts = BTreeSet::new();
    for key in keys(pack, "integrations") {
        if let Some(text) = pack.text(&format!("integrations/{key}.yaml"))
            && let Ok(spec) = huxley_integrations::parse(text)
            && let Ok(imported) = huxley_integrations::import_value(spec)
        {
            for server in &imported.definition.servers {
                hosts.extend(host(&server.url));
            }
            for scheme in &imported.definition.auth_schemes {
                let text = serde_json::to_string(&scheme.kind).unwrap_or_default();
                for part in text.split('"').filter(|p| p.starts_with("http")) {
                    hosts.extend(host(part));
                }
            }
        }
    }
    for kind in ["workflows", "templates"] {
        for key in keys(pack, kind) {
            if let Some(text) = pack.text(&format!("{kind}/{key}.yaml"))
                && let Ok(doc) = serde_saphyr::from_str::<Json>(text)
            {
                walk(&doc, &mut hosts);
            }
        }
    }
    hosts.into_iter().collect()
}

#[derive(Deserialize)]
struct ScriptMeta {
    name: String,
    #[serde(default)]
    description: Option<String>,
    language: huxley_scripts::Language,
    #[serde(default)]
    parameters: Option<Json>,
}

fn script_meta(pack: &Pack, key: &str) -> Result<(ScriptMeta, String), InstallError> {
    let bad = |what: &str| InstallError::Content(format!("scripts/{key}: {what}"));
    let meta: ScriptMeta = serde_json::from_str(
        pack.text(&format!("scripts/{key}.json"))
            .unwrap_or_default(),
    )
    .map_err(|e| bad(&e.to_string()))?;
    let ext = meta.language.extension();
    let source = pack
        .text(&format!("scripts/{key}.{ext}"))
        .ok_or_else(|| bad(&format!("no scripts/{key}.{ext}")))?
        .to_owned();
    Ok((meta, source))
}

/// What installing `pack` in `org_id` would do (as the system: the caller
/// checks access to `org_id`).
pub async fn review(
    conn: &mut PgConnection,
    upload_id: Uuid,
    pack: &Pack,
    org_id: Uuid,
    trusted: &[String],
) -> Result<Review, InstallError> {
    let previous = installed(&mut *conn, org_id, &pack.manifest.key).await?;
    let mine = previous
        .as_ref()
        .map(|(_, items)| items.clone())
        .unwrap_or_default();
    let mut items = Vec::new();
    for kind in KINDS {
        for key in keys(pack, kind) {
            let ours = mine.get(kind).and_then(|m| m.get(key)).copied();
            let action = match existing(&mut *conn, org_id, kind, key).await? {
                Some(id) if Some(id) == ours => "update",
                Some(_) => "conflict",
                None if ours.is_some() && kind == "templates" => "update",
                None => "add",
            };
            items.push(ReviewItem {
                kind: kind.to_owned(),
                key: key.clone(),
                action: action.to_owned(),
            });
        }
    }
    let mut scripts = Vec::new();
    for key in keys(pack, "scripts") {
        let (meta, _) = script_meta(pack, key)?;
        scripts.push(ReviewScript {
            key: key.clone(),
            language: serde_json::to_value(meta.language)
                .ok()
                .and_then(|v| v.as_str().map(str::to_owned))
                .unwrap_or_default(),
        });
    }
    let m = &pack.manifest;
    Ok(Review {
        upload_id,
        key: m.key.clone(),
        name: m.name.clone(),
        version: m.version.clone(),
        description: m.description.clone(),
        publisher: m.publisher.clone(),
        trust: pack.trust(trusted),
        installed_version: previous.map(|(v, _)| v),
        items,
        egress: egress(pack),
        scripts,
    })
}

fn content_error(kind: &str, key: &str, e: impl std::fmt::Display) -> InstallError {
    InstallError::Content(format!("{kind}/{key}: {e}"))
}

/// Where an install writes: the caller's transaction and identity, and the
/// organization.
struct Target<'a> {
    conn: &'a mut PgConnection,
    ctx: &'a RequestContext,
    org_id: Uuid,
}

impl Target<'_> {
    async fn integration(
        &mut self,
        pack: &Pack,
        key: &str,
        current: Option<Uuid>,
    ) -> Result<Uuid, InstallError> {
        let bad = |e: &dyn std::fmt::Display| content_error("integrations", key, e);
        let spec = pack
            .text(&format!("integrations/{key}.yaml"))
            .unwrap_or_default();
        let overlay = match pack.text(&format!("integrations/{key}.overlay.json")) {
            Some(text) => Some(
                serde_json::from_str::<huxley_integrations::overlay::Overlay>(text)
                    .map_err(|e| bad(&e))?,
            ),
            None => None,
        };
        let Some(id) = current else {
            let detail = huxley_integrations::store::create(
                &mut *self.conn,
                self.ctx,
                self.org_id,
                Some(key),
                spec,
            )
            .await
            .map_err(|e| bad(&e))?;
            // The pack's overlay applies to a new install; on upgrades, your
            // own edits to it stay.
            if let Some(overlay) = overlay {
                huxley_integrations::store::set_overlay(
                    &mut *self.conn,
                    self.ctx,
                    detail.summary.id,
                    overlay,
                )
                .await
                .map_err(|e| bad(&e))?;
            }
            return Ok(detail.summary.id);
        };
        huxley_integrations::store::update(&mut *self.conn, self.ctx, id, spec, None)
            .await
            .map_err(|e| bad(&e))?;
        Ok(id)
    }

    async fn form(
        &mut self,
        pack: &Pack,
        key: &str,
        current: Option<Uuid>,
    ) -> Result<Uuid, InstallError> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct FormFile {
            name: String,
            #[serde(default)]
            description: Option<String>,
            schema: Json,
            #[serde(default)]
            ui: Json,
            /// Offered as an Action on a client, contact or ticket.
            #[serde(default, rename = "actionFor")]
            action_for: Option<String>,
        }
        let file: FormFile =
            serde_json::from_str(pack.text(&format!("forms/{key}.json")).unwrap_or_default())
                .map_err(|e| content_error("forms", key, e))?;
        let input = huxley_forms::FormInput {
            key: key.to_owned(),
            name: file.name,
            description: file.description,
            schema: file.schema,
            ui: file.ui,
            action_for: file.action_for,
        };
        let form = match current {
            Some(id) => huxley_forms::update(&mut *self.conn, self.ctx, id, &input).await,
            None => huxley_forms::create(&mut *self.conn, self.ctx, self.org_id, &input).await,
        }
        .map_err(|e| content_error("forms", key, e))?;
        Ok(form.id)
    }

    async fn dataset(
        &mut self,
        pack: &Pack,
        key: &str,
        current: Option<Uuid>,
    ) -> Result<Uuid, InstallError> {
        let bad = |e: &dyn std::fmt::Display| content_error("datasets", key, e);
        let mut file: Json = serde_json::from_str(
            pack.text(&format!("datasets/{key}.json"))
                .unwrap_or_default(),
        )
        .map_err(|e| bad(&e))?;
        let Some(fields) = file.as_object_mut() else {
            return Err(bad(&"not a dataset definition"));
        };
        fields.insert("key".to_owned(), json!(key));
        let input: huxley_datasets::DatasetInput =
            serde_json::from_value(file).map_err(|e| bad(&e))?;
        let dataset = match current {
            Some(id) => huxley_datasets::update(&mut *self.conn, self.ctx, id, &input).await,
            None => huxley_datasets::create(&mut *self.conn, self.ctx, self.org_id, &input).await,
        }
        .map_err(|e| bad(&e))?;
        Ok(dataset.id)
    }

    async fn script(
        &mut self,
        pack: &Pack,
        key: &str,
        current: Option<Uuid>,
    ) -> Result<Uuid, InstallError> {
        let (meta, source) = script_meta(pack, key)?;
        let script = if let Some(id) = current {
            let input = huxley_scripts::NewVersion {
                source,
                parameters: meta.parameters,
                notes: Some(format!(
                    "From pack {} {}",
                    pack.manifest.key, pack.manifest.version
                )),
                base_version: None,
            };
            huxley_scripts::save_version(&mut *self.conn, self.ctx, id, &input).await
        } else {
            let input = huxley_scripts::NewScript {
                key: key.to_owned(),
                name: meta.name,
                description: meta.description,
                language: meta.language,
                source,
                parameters: meta.parameters,
            };
            huxley_scripts::create(&mut *self.conn, self.ctx, self.org_id, &input).await
        }
        .map_err(|e| content_error("scripts", key, e))?;
        Ok(script.summary.id)
    }

    async fn workflow(
        &mut self,
        expr: &huxley_expr::Engine,
        pack: &Pack,
        key: &str,
        current: Option<Uuid>,
    ) -> Result<Uuid, InstallError> {
        let bad = |e: &dyn std::fmt::Display| content_error("workflows", key, e);
        let text = pack
            .text(&format!("workflows/{key}.yaml"))
            .unwrap_or_default();
        let mut document: Json = serde_saphyr::from_str(text).map_err(|e| bad(&e))?;
        if !document.is_object() {
            return Err(bad(&"not a workflow document"));
        }
        document["metadata"]["key"] = json!(key);
        let workflow = match current {
            Some(id) => {
                let summary = huxley_workflows::summary(&mut *self.conn, id)
                    .await
                    .map_err(|e| bad(&e))?;
                huxley_workflows::save_draft(
                    &mut *self.conn,
                    self.ctx,
                    expr,
                    id,
                    summary.revision,
                    document,
                )
                .await
            }
            None => {
                huxley_workflows::create(&mut *self.conn, self.ctx, expr, self.org_id, document)
                    .await
            }
        }
        .map_err(|e| bad(&e))?;
        Ok(workflow.summary.id)
    }

    /// Replaces the pack's previous templates with its current ones.
    async fn templates(
        &mut self,
        pack: &Pack,
        previous: Option<&BTreeMap<String, Uuid>>,
    ) -> Result<BTreeMap<String, Uuid>, InstallError> {
        for id in previous.into_iter().flat_map(BTreeMap::values) {
            match huxley_templates::store::delete(&mut *self.conn, self.ctx, *id).await {
                Ok(()) | Err(huxley_templates::store::StoreError::NotFound) => {}
                Err(e) => return Err(content_error("templates", &id.to_string(), e)),
            }
        }
        let mut out = BTreeMap::new();
        for key in keys(pack, "templates") {
            let bad = |e: &dyn std::fmt::Display| content_error("templates", key, e);
            let text = pack
                .text(&format!("templates/{key}.yaml"))
                .unwrap_or_default();
            let template: huxley_templates::Template =
                serde_saphyr::from_str(text).map_err(|e| bad(&e))?;
            let saved =
                huxley_templates::store::save(&mut *self.conn, self.ctx, self.org_id, &template)
                    .await
                    .map_err(|e| bad(&e))?;
            out.insert(key.clone(), saved.key.parse().map_err(|e| bad(&e))?);
        }
        Ok(out)
    }
}

/// For each keyed item, the id of what this pack installed before (if it's
/// still there). Anything else with the key is a conflict: nothing is
/// changed.
async fn current_ids(
    conn: &mut PgConnection,
    pack: &Pack,
    org_id: Uuid,
    previous: &Items,
) -> Result<Items, InstallError> {
    let mut out: Items = BTreeMap::new();
    for kind in KINDS.into_iter().filter(|k| *k != "templates") {
        for key in keys(pack, kind) {
            let ours = previous.get(kind).and_then(|m| m.get(key)).copied();
            match existing(&mut *conn, org_id, kind, key).await? {
                Some(id) if Some(id) == ours => {
                    out.entry(kind.to_owned())
                        .or_default()
                        .insert(key.clone(), id);
                }
                Some(_) => {
                    return Err(InstallError::Conflict {
                        kind: kind.trim_end_matches('s').to_owned(),
                        key: key.clone(),
                    });
                }
                None => {}
            }
        }
    }
    Ok(out)
}

/// Installs (or upgrades) `pack` in `org_id`, in the caller's transaction.
/// Items an earlier version added that this one doesn't are left in place
/// (and still counted as the pack's).
pub async fn install(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    expr: &huxley_expr::Engine,
    pack: &Pack,
    org_id: Uuid,
    trust: &Trust,
) -> Result<Items, InstallError> {
    let previous = installed(&mut *conn, org_id, &pack.manifest.key)
        .await?
        .map(|(_, items)| items)
        .unwrap_or_default();
    let current = current_ids(&mut *conn, pack, org_id, &previous).await?;
    let id_of = |kind: &str, key: &str| current.get(kind).and_then(|m| m.get(key)).copied();
    let mut items = previous.clone();
    items.remove("templates");
    let mut target = Target {
        conn: &mut *conn,
        ctx,
        org_id,
    };
    // Dependencies first: workflows may use the datasets, integrations,
    // forms and scripts.
    for kind in ["datasets", "integrations", "forms", "scripts", "workflows"] {
        for key in keys(pack, kind) {
            let at = id_of(kind, key);
            let id = match kind {
                "integrations" => target.integration(pack, key, at).await?,
                "forms" => target.form(pack, key, at).await?,
                "scripts" => target.script(pack, key, at).await?,
                "datasets" => target.dataset(pack, key, at).await?,
                _ => target.workflow(expr, pack, key, at).await?,
            };
            items
                .entry(kind.to_owned())
                .or_default()
                .insert(key.clone(), id);
        }
    }
    let templates = target.templates(pack, previous.get("templates")).await?;
    if !templates.is_empty() {
        items.insert("templates".to_owned(), templates);
    }

    let m = &pack.manifest;
    let trust_label = match trust {
        Trust::BuiltIn { .. } => "built_in",
        Trust::Trusted { .. } => "trusted",
        Trust::UnknownSigner { .. } => "unknown_signer",
        Trust::Unsigned => "unsigned",
    };
    sqlx::query(
        "INSERT INTO pack_installs (id, org_id, pack_key, name, version, publisher, trust, items, installed_by) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9) \
         ON CONFLICT (org_id, pack_key) DO UPDATE SET name = EXCLUDED.name, version = EXCLUDED.version, \
             publisher = EXCLUDED.publisher, trust = EXCLUDED.trust, items = EXCLUDED.items, \
             installed_by = EXCLUDED.installed_by, installed_at = now()",
    )
    .bind(huxley_core::new_id())
    .bind(org_id)
    .bind(&m.key)
    .bind(&m.name)
    .bind(&m.version)
    .bind(&m.publisher)
    .bind(trust_label)
    .bind(serde_json::to_value(&items).unwrap_or_default())
    .bind(ctx.actor.id())
    .execute(&mut *conn)
    .await?;
    huxley_db::audit::record(
        &mut *conn,
        ctx,
        huxley_db::audit::AuditRecord::new("pack.installed")
            .org(org_id)
            .details(json!({ "pack": m.key, "version": m.version, "trust": trust_label })),
    )
    .await
    .map_err(|e| InstallError::Db(sqlx::Error::Protocol(e.to_string())))?;
    Ok(items)
}

/// What to put in an exported pack: the manifest's identity, and the ids
/// of the organization's items to include.
#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExportRequest {
    pub org_id: Uuid,
    /// `<owner>/<name>`, lowercase.
    pub key: String,
    pub name: String,
    /// Semver, e.g. `1.0.0`.
    pub version: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub publisher: String,
    #[serde(default)]
    pub integrations: Vec<Uuid>,
    #[serde(default)]
    pub workflows: Vec<Uuid>,
    #[serde(default)]
    pub forms: Vec<Uuid>,
    #[serde(default)]
    pub scripts: Vec<Uuid>,
}

fn json_bytes(value: &impl Serialize) -> Vec<u8> {
    serde_json::to_vec_pretty(value).expect("JSON values serialize")
}

/// Builds an unsigned pack of `request`'s items, which must all belong to
/// its organization (as the system: the caller checks access). Workflows
/// are exported as their current drafts; connections and secrets never
/// are.
pub async fn export(
    conn: &mut PgConnection,
    expr: &huxley_expr::Engine,
    request: &ExportRequest,
) -> Result<Vec<u8>, InstallError> {
    let elsewhere =
        |what: &str| InstallError::Content(format!("that {what} isn't in this organization"));
    let mut files = BTreeMap::new();
    let mut contents = huxley_packs::Contents::default();
    for id in &request.integrations {
        let detail = huxley_integrations::store::get(&mut *conn, *id)
            .await
            .map_err(|_| elsewhere("integration"))?;
        if detail.summary.org_id != request.org_id {
            return Err(elsewhere("integration"));
        }
        let key = &detail.summary.key;
        let spec: Json = sqlx::query_scalar(
            "SELECT spec FROM integration_versions WHERE integration_id = $1 AND version = $2",
        )
        .bind(id)
        .bind(detail.summary.current_version)
        .fetch_one(&mut *conn)
        .await?;
        // JSON is YAML too.
        files.insert(format!("integrations/{key}.yaml"), json_bytes(&spec));
        if detail.overlay != huxley_integrations::overlay::Overlay::default() {
            files.insert(
                format!("integrations/{key}.overlay.json"),
                json_bytes(&detail.overlay),
            );
        }
        contents.integrations.push(key.clone());
    }
    for id in &request.forms {
        let form = huxley_forms::get(&mut *conn, *id)
            .await
            .map_err(|_| elsewhere("form"))?;
        if form.org_id != request.org_id {
            return Err(elsewhere("form"));
        }
        let file = json!({ "name": form.name, "description": form.description, "schema": form.schema, "ui": form.ui });
        files.insert(format!("forms/{}.json", form.key), json_bytes(&file));
        contents.forms.push(form.key);
    }
    for id in &request.scripts {
        let script = huxley_scripts::get(&mut *conn, *id)
            .await
            .map_err(|_| elsewhere("script"))?;
        let summary = &script.summary;
        if summary.org_id != request.org_id {
            return Err(elsewhere("script"));
        }
        let ext = summary.language.extension();
        let meta = json!({ "name": summary.name, "description": summary.description,
                           "language": summary.language, "parameters": script.parameters });
        files.insert(format!("scripts/{}.json", summary.key), json_bytes(&meta));
        files.insert(
            format!("scripts/{}.{ext}", summary.key),
            script.source.clone().into_bytes(),
        );
        contents.scripts.push(summary.key.clone());
    }
    for id in &request.workflows {
        let workflow = huxley_workflows::get(&mut *conn, expr, *id)
            .await
            .map_err(|_| elsewhere("workflow"))?;
        if workflow.summary.org_id != request.org_id {
            return Err(elsewhere("workflow"));
        }
        let key = workflow.summary.key.clone();
        files.insert(format!("workflows/{key}.yaml"), json_bytes(&workflow.draft));
        contents.workflows.push(key);
    }
    let manifest = huxley_packs::Manifest {
        api_version: "usehuxley.io/v1".into(),
        kind: "Pack".into(),
        key: request.key.clone(),
        name: request.name.clone(),
        version: request.version.clone(),
        description: request.description.clone(),
        publisher: request.publisher.clone(),
        contents,
        files: BTreeMap::new(),
    };
    huxley_packs::build(manifest, &files, None).map_err(|e| InstallError::Content(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::host;

    #[test]
    fn hosts_from_urls() {
        assert_eq!(
            host("https://graph.microsoft.com/v1.0").as_deref(),
            Some("graph.microsoft.com")
        );
        assert_eq!(
            host("http://u:p@API.example.com:8080/x").as_deref(),
            Some("api.example.com")
        );
        assert_eq!(host("https://{{ vars.host }}/x"), None);
        assert_eq!(host("not a url"), None);
    }
}
