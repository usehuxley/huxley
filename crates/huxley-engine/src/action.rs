//! The `integration.action` step (design doc §6.2, §7): calls one action
//! of an imported integration through a connection.
//!
//! - **Which integration:** the key before the `/`, found in the run's
//!   organization or the nearest one above it. A run uses one version of
//!   each integration throughout: the one current when the run started
//!   (pinned in `runs.integration_pins`), or when it first called the
//!   integration if it couldn't be pinned then.
//! - **Which action:** the key after the `/`, or what it was renamed to
//!   (the version's `aliases`).
//! - **Which connection:** the one named, else the nearest connection to
//!   the integration: the run's organization's own, then shared ones above.
//! - **The request:** built from the action's method, path and parameter
//!   locations, authenticated from the connection, and sent through the
//!   `http.request` machinery (egress policy, size limits, error kinds).
//!   The step's output is the response body.

use std::collections::BTreeSet;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use huxley_db::{OrgScope, begin_scoped};
use huxley_hwf::{Step, Workflow};
use huxley_integrations::connections::{self, ConnectionStatus, ResolvedAuth};
use huxley_integrations::{Action, Definition, ParamLocation};
use serde_json::{Map, Value as Json, json};
use sqlx::PgConnection;
use uuid::Uuid;

use crate::walk::error;
use crate::{Engine, EngineResult};

/// The integration keys a workflow's action steps use.
pub(crate) fn integration_keys(workflow: &Workflow) -> BTreeSet<String> {
    fn visit(steps: &[Step], out: &mut BTreeSet<String>) {
        for step in steps {
            if let Step::IntegrationAction(s) = step
                && let Some((integration, _)) = s.action.split_once('/')
            {
                out.insert(integration.to_owned());
            }
            for (_, block) in step.blocks() {
                visit(block, out);
            }
        }
    }
    let mut out = BTreeSet::new();
    visit(&workflow.spec.steps, &mut out);
    visit(&workflow.spec.on_error, &mut out);
    out
}

/// The integration `key` refers to from `org_id`: its id and current
/// version. Nearest organization first.
async fn find_integration(
    conn: &mut PgConnection,
    org_id: Uuid,
    key: &str,
) -> sqlx::Result<Option<(Uuid, i32)>> {
    sqlx::query_as(
        "SELECT i.id, i.current_version FROM integrations i \
           JOIN organizations o ON o.id = i.org_id \
          WHERE i.key = $1 AND o.path @> (SELECT path FROM organizations WHERE id = $2) \
          ORDER BY nlevel(o.path) DESC LIMIT 1",
    )
    .bind(key)
    .bind(org_id)
    .fetch_optional(conn)
    .await
}

/// Pins the current version of each integration the workflow uses, as the
/// run starts. Integrations the starting scope can't see are pinned when
/// first called instead.
pub(crate) async fn pin(
    conn: &mut PgConnection,
    run_id: Uuid,
    org_id: Uuid,
    document: &Json,
) -> EngineResult<()> {
    let Ok(workflow) = serde_json::from_value::<Workflow>(document.clone()) else {
        return Ok(());
    };
    let mut pins = Map::new();
    for key in integration_keys(&workflow) {
        if let Some((id, version)) = find_integration(&mut *conn, org_id, &key).await? {
            pins.insert(key, json!({ "id": id, "version": version }));
        }
    }
    if !pins.is_empty() {
        sqlx::query("UPDATE runs SET integration_pins = $2 WHERE id = $1")
            .bind(run_id)
            .bind(Json::Object(pins))
            .execute(conn)
            .await?;
    }
    Ok(())
}

/// The run's pinned version of `key`, pinning it now if it wasn't.
async fn pinned(
    conn: &mut PgConnection,
    run_id: Uuid,
    org_id: Uuid,
    key: &str,
) -> EngineResult<Option<(Uuid, i32)>> {
    let (pins,): (Json,) = sqlx::query_as("SELECT integration_pins FROM runs WHERE id = $1")
        .bind(run_id)
        .fetch_one(&mut *conn)
        .await?;
    if let (Some(id), Some(version)) = (
        pins[key]["id"]
            .as_str()
            .and_then(|s| s.parse::<Uuid>().ok()),
        pins[key]["version"]
            .as_i64()
            .and_then(|v| i32::try_from(v).ok()),
    ) {
        return Ok(Some((id, version)));
    }
    let Some((id, version)) = find_integration(&mut *conn, org_id, key).await? else {
        return Ok(None);
    };
    sqlx::query(
        "UPDATE runs SET integration_pins = integration_pins || jsonb_build_object($2::text, $3::jsonb) \
          WHERE id = $1",
    )
    .bind(run_id)
    .bind(key)
    .bind(json!({ "id": id, "version": version }))
    .execute(conn)
    .await?;
    Ok(Some((id, version)))
}

/// The connection a step uses: the one named, or the nearest one to the
/// integration the run's organization may use (its own, then shared ones
/// above it).
async fn find_connection(
    conn: &mut PgConnection,
    org_id: Uuid,
    integration_id: Uuid,
    key: Option<&str>,
) -> sqlx::Result<Option<Uuid>> {
    sqlx::query_scalar(
        "SELECT c.id FROM connections c JOIN organizations o ON o.id = c.org_id \
          WHERE c.integration_id = $1 AND ($3::text IS NULL OR c.key = $3) \
            AND (c.org_id = $2 OR (c.shared AND \
                 o.path @> (SELECT path FROM organizations WHERE id = $2))) \
          ORDER BY nlevel(o.path) DESC LIMIT 1",
    )
    .bind(integration_id)
    .bind(org_id)
    .bind(key)
    .fetch_optional(conn)
    .await
}

/// The connection `target` (`<integration>` or `<integration>/<connection>`)
/// means from `org_id`, as for an action.
pub(crate) async fn connection_for(
    conn: &mut PgConnection,
    org_id: Uuid,
    target: &str,
) -> EngineResult<Result<Uuid, Json>> {
    let (integration, key) = match target.split_once('/') {
        Some((integration, key)) => (integration, Some(key)),
        None => (target, None),
    };
    let Some((integration_id, _)) = find_integration(&mut *conn, org_id, integration).await? else {
        return Ok(Err(error(
            "definition",
            format!("no integration {integration:?} here or above"),
        )));
    };
    Ok(find_connection(&mut *conn, org_id, integration_id, key)
        .await?
        .ok_or_else(|| {
            error(
                "connection",
                format!("no connection {target:?} this organization can use"),
            )
        }))
}

/// What a step needs: the action, its connection, and the definition.
struct Target {
    action: Action,
    connection_id: Uuid,
    rate_limit: Option<huxley_integrations::overlay::RateLimit>,
}

async fn target(
    conn: &mut PgConnection,
    run_id: Option<Uuid>,
    org_id: Uuid,
    input: &Json,
) -> EngineResult<Result<Target, Json>> {
    let full = input["action"].as_str().unwrap_or_default();
    let Some((integration_key, action_key)) = full.split_once('/') else {
        return Ok(Err(error(
            "definition",
            format!("action {full:?} should be <integration>/<action>"),
        )));
    };
    // Runs use their pinned version; polls (no run) use the current one.
    let found = match run_id {
        Some(run_id) => pinned(&mut *conn, run_id, org_id, integration_key).await?,
        None => find_integration(&mut *conn, org_id, integration_key).await?,
    };
    let Some((integration_id, version)) = found else {
        return Ok(Err(error(
            "definition",
            format!("no integration {integration_key:?} here or above"),
        )));
    };
    let (definition, aliases, overlay): (Json, Json, Json) = sqlx::query_as(
        "SELECT v.definition, v.aliases, i.overlay FROM integration_versions v \
           JOIN integrations i ON i.id = v.integration_id \
          WHERE v.integration_id = $1 AND v.version = $2",
    )
    .bind(integration_id)
    .bind(version)
    .fetch_one(&mut *conn)
    .await?;
    let mut definition: Definition = serde_json::from_value(definition)
        .map_err(|e| crate::EngineError::Document(e.to_string()))?;
    // The overlay's pagination applies to calls (names and hiding don't matter).
    let overlay = huxley_integrations::overlay::Overlay::from_json(&overlay);
    overlay.apply(&mut definition, &aliases);
    let rate_limit = overlay.rate_limit;
    let key = aliases[action_key].as_str().unwrap_or(action_key);
    let Some(action) = definition.actions.into_iter().find(|a| a.key == key) else {
        return Ok(Err(error(
            "definition",
            format!(
                "integration {integration_key:?} (version {version}) has no action {action_key:?}"
            ),
        )));
    };
    let connection = input["connection"].as_str();
    let Some(connection_id) =
        find_connection(&mut *conn, org_id, integration_id, connection).await?
    else {
        return Ok(Err(error(
            "connection",
            match connection {
                Some(k) => {
                    format!("no connection {k:?} to {integration_key:?} this organization can use")
                }
                None => format!("no connection to {integration_key:?} this organization can use"),
            },
        )));
    };
    Ok(Ok(Target {
        action,
        connection_id,
        rate_limit,
    }))
}

/// Checks inputs against the action's schema.
fn check_inputs(action: &Action, inputs: &Json) -> Result<(), Json> {
    let Ok(validator) = jsonschema::validator_for(&action.input_schema) else {
        // An odd schema in a vendor spec shouldn't block the call.
        return Ok(());
    };
    let problems: Vec<String> = validator
        .iter_errors(inputs)
        .map(|e| {
            let at = e.instance_path().to_string();
            if at.is_empty() {
                e.to_string()
            } else {
                format!("{at}: {e}")
            }
        })
        .collect();
    if problems.is_empty() {
        Ok(())
    } else {
        let mut failure = error(
            "inputs",
            format!(
                "the inputs don't fit {}: {}",
                action.key,
                problems.join("; ")
            ),
        );
        failure["problems"] = json!(problems);
        Err(failure)
    }
}

/// A path parameter's value as text.
fn text(value: &Json) -> String {
    match value {
        Json::String(s) => s.clone(),
        Json::Null => String::new(),
        other => other.to_string(),
    }
}

/// The `http.request` input for an action call.
fn request(
    action: &Action,
    base_url: &str,
    auth: &ResolvedAuth,
    token: Option<&str>,
    inputs: &Json,
) -> Result<Json, Json> {
    let mut path = action.path.clone();
    let mut query = Map::new();
    let mut headers = Map::new();
    let mut cookies = Vec::new();
    for param in &action.parameters {
        let value = &inputs[&param.name];
        if value.is_null() {
            continue;
        }
        match param.location {
            ParamLocation::Path => {
                let encoded: String =
                    url::form_urlencoded::byte_serialize(text(value).as_bytes()).collect();
                path = path.replace(&format!("{{{}}}", param.name), &encoded.replace('+', "%20"));
            }
            ParamLocation::Query => {
                query.insert(param.name.clone(), value.clone());
            }
            ParamLocation::Header => {
                headers.insert(param.name.clone(), Json::String(text(value)));
            }
            ParamLocation::Cookie => cookies.push(format!("{}={}", param.name, text(value))),
        }
    }
    match auth {
        ResolvedAuth::None => {}
        ResolvedAuth::ApiKey {
            location,
            name,
            value,
        } => match location {
            ParamLocation::Query => {
                query.insert(name.clone(), Json::String(value.clone()));
            }
            ParamLocation::Cookie => cookies.push(format!("{name}={value}")),
            _ => {
                headers.insert(name.clone(), Json::String(value.clone()));
            }
        },
        ResolvedAuth::Basic { username, password } => {
            let encoded = STANDARD.encode(format!("{username}:{password}"));
            headers.insert("Authorization".into(), json!(format!("Basic {encoded}")));
        }
        ResolvedAuth::Bearer { token } => {
            headers.insert("Authorization".into(), json!(format!("Bearer {token}")));
        }
        ResolvedAuth::OAuth2ClientCredentials { .. }
        | ResolvedAuth::OAuth2AuthorizationCode { .. } => {
            let token = token.unwrap_or_default();
            headers.insert("Authorization".into(), json!(format!("Bearer {token}")));
        }
    }
    if !cookies.is_empty() {
        headers.insert("Cookie".into(), Json::String(cookies.join("; ")));
    }
    let mut spec = json!({
        "method": action.method,
        "url": format!("{base_url}{path}"),
        "headers": headers,
        "query": query,
    });
    if let Some(body) = &action.body
        && !inputs["body"].is_null()
    {
        let body_type = if body.content_type.contains("json") {
            "json"
        } else if body
            .content_type
            .starts_with("application/x-www-form-urlencoded")
        {
            "form"
        } else if body.content_type.starts_with("text/") {
            "text"
        } else {
            return Err(error(
                "unsupported",
                format!("{} request bodies aren't supported yet", body.content_type),
            ));
        };
        spec["body"] = inputs["body"].clone();
        spec["body_type"] = json!(body_type);
    }
    Ok(spec)
}

impl Engine {
    /// Runs an `integration.action` step: `Ok` holds the response body.
    pub(crate) async fn call_action(
        &self,
        run_id: Uuid,
        org_id: Uuid,
        input: &Json,
    ) -> EngineResult<Result<Json, Json>> {
        self.call_action_in(Some(run_id), org_id, input, false)
            .await
    }

    /// Calls an action for a poll trigger: no run, so the integration's
    /// current version. `input` is `{action, connection, inputs}`; `Ok`
    /// holds the response body.
    pub async fn poll_action(
        &self,
        org_id: Uuid,
        input: &Json,
    ) -> EngineResult<Result<Json, Json>> {
        self.call_action_in(None, org_id, input, false).await
    }

    /// Calls a read-only (GET) action to look something up, e.g. a form
    /// field's choices: anything else is refused. Same input as
    /// [`Self::poll_action`].
    pub async fn lookup_action(
        &self,
        org_id: Uuid,
        input: &Json,
    ) -> EngineResult<Result<Json, Json>> {
        self.call_action_in(None, org_id, input, true).await
    }

    async fn call_action_in(
        &self,
        run_id: Option<Uuid>,
        org_id: Uuid,
        input: &Json,
        read_only: bool,
    ) -> EngineResult<Result<Json, Json>> {
        let inputs = match &input["inputs"] {
            Json::Null => json!({}),
            other => other.clone(),
        };
        let mut tx = begin_scoped(self.db(), &OrgScope::System).await?;
        let target = match target(&mut tx, run_id, org_id, input).await? {
            Ok(target) => target,
            Err(failure) => {
                tx.commit().await?;
                return Ok(Err(failure));
            }
        };
        tx.commit().await?;
        if read_only && !target.action.method.eq_ignore_ascii_case("get") {
            return Ok(Err(error(
                "definition",
                format!(
                    "{} isn't a read-only (GET) action, so it can't be used to look things up",
                    target.action.key
                ),
            )));
        }
        if let Err(failure) = check_inputs(&target.action, &inputs) {
            return Ok(Err(failure));
        }

        let mut tx = begin_scoped(self.db(), &OrgScope::System).await?;
        let view = connections::get(&mut tx, target.connection_id)
            .await
            .map_err(|e| crate::EngineError::Document(e.to_string()))?;
        let resolved = match connections::resolve_for(
            &mut tx,
            &self.crypto,
            target.connection_id,
            Some(org_id),
        )
        .await
        {
            Ok(resolved) => resolved,
            Err(e) => {
                tx.commit().await?;
                return Ok(Err(error("connection", e.to_string())));
            }
        };
        tx.commit().await?;
        if view.status == ConnectionStatus::NeedsAuth {
            return Ok(Err(error(
                "auth",
                format!(
                    "connection {:?} needs someone to sign it in{}",
                    view.key,
                    view.status_message
                        .map(|m| format!(" ({m})"))
                        .unwrap_or_default()
                ),
            )));
        }
        let token = match &resolved.auth {
            ResolvedAuth::OAuth2ClientCredentials { .. }
            | ResolvedAuth::OAuth2AuthorizationCode { .. } => {
                match self
                    .connection_token_for(target.connection_id, Some(org_id))
                    .await
                {
                    Ok(token) => token,
                    Err(e) => return Ok(Err(crate::oauth::token_error(&e))),
                }
            }
            _ => None,
        };
        let spec = match request(
            &target.action,
            &resolved.base_url,
            &resolved.auth,
            token.as_deref(),
            &inputs,
        ) {
            Ok(spec) => spec,
            Err(failure) => return Ok(Err(failure)),
        };
        if let Some(limit) = target.rate_limit
            && let Err(at) = crate::rate::acquire(self.db(), target.connection_id, limit).await?
        {
            return Ok(Err(rate_limited(at)));
        }
        Ok(self.send(&target, spec, &resolved.base_url, input).await)
    }

    /// Sends the request: one page, or every page with `all_pages`.
    async fn send(
        &self,
        target: &Target,
        spec: Json,
        base_url: &str,
        input: &Json,
    ) -> Result<Json, Json> {
        let action = &target.action;
        if input["all_pages"] == json!(true) {
            let Some(pagination) = action.pagination.clone() else {
                return Err(error(
                    "definition",
                    format!(
                        "{} has no pagination set: set it in the integration's overlay to use all_pages",
                        action.key
                    ),
                ));
            };
            let max_pages = input["max_pages"]
                .as_u64()
                .map_or(DEFAULT_MAX_PAGES, |n| n.clamp(1, MAX_PAGES));
            let limit = target
                .rate_limit
                .map(|l| (self.db().clone(), target.connection_id, l));
            return crate::pages::all_pages(
                &self.egress,
                spec,
                base_url,
                &pagination,
                max_pages,
                limit,
            )
            .await;
        }
        crate::http::request(&self.egress, &spec)
            .await
            .map(|response| response["body"].clone())
    }
}

/// The failure of a call over its rate limit: the step waits until `at`.
pub(crate) fn rate_limited(at: time::OffsetDateTime) -> Json {
    let mut failure = error(
        "rate_limited",
        "the integration's rate limit is used up for now",
    );
    failure["retry_at"] = json!(
        at.format(&time::format_description::well_known::Rfc3339)
            .unwrap_or_default()
    );
    failure
}

/// Pages followed with `all_pages` when the step doesn't say.
const DEFAULT_MAX_PAGES: u64 = 20;
const MAX_PAGES: u64 = 100;
