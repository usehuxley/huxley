//! SCIM 2.0 (RFC 7643/7644) for identity providers (ADR-088), at
//! `/scim/v2`: Users (Huxley users) and Groups (teams), with `eq` filters,
//! paging, PUT and PATCH, and the discovery endpoints. Authenticated by the
//! sign-in provider's SCIM token, not a user's. Not in the OpenAPI document:
//! it's the standard SCIM API.

use axum::Router;
use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use huxley_core::{RequestContext, Via};
use huxley_db::{OrgScope, begin_scoped};
use huxley_sso::scim::{
    self, GroupFilter, GroupInput, ScimError, ScimGroup, ScimProvider, ScimUser, UserFilter,
    UserInput,
};
use serde::Deserialize;
use serde_json::{Map, Value, json};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use uuid::Uuid;

use crate::AppState;
use crate::context::ClientInfo;

const USER: &str = "urn:ietf:params:scim:schemas:core:2.0:User";
const GROUP: &str = "urn:ietf:params:scim:schemas:core:2.0:Group";
const LIST: &str = "urn:ietf:params:scim:api:messages:2.0:ListResponse";
const ERROR: &str = "urn:ietf:params:scim:api:messages:2.0:Error";
const PATCH: &str = "urn:ietf:params:scim:api:messages:2.0:PatchOp";
const MAX_COUNT: i64 = 200;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/ServiceProviderConfig", get(service_provider_config))
        .route("/ResourceTypes", get(resource_types))
        .route("/Schemas", get(schemas))
        .route("/Users", get(list_users).post(create_user))
        .route(
            "/Users/{id}",
            get(get_user)
                .put(put_user)
                .patch(patch_user)
                .delete(delete_user),
        )
        .route("/Groups", get(list_groups).post(create_group))
        .route(
            "/Groups/{id}",
            get(get_group)
                .put(put_group)
                .patch(patch_group)
                .delete(delete_group),
        )
}

// ---------------------------------------------------------------------------
// Responses and errors

struct Scim(StatusCode, Value);

impl IntoResponse for Scim {
    fn into_response(self) -> Response {
        let mut response = (self.0, self.1.to_string()).into_response();
        response.headers_mut().insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/scim+json"),
        );
        response
    }
}

fn ok(body: Value) -> Scim {
    Scim(StatusCode::OK, body)
}

#[derive(Debug)]
struct Problem {
    status: StatusCode,
    scim_type: Option<&'static str>,
    detail: String,
}

impl Problem {
    fn new(status: StatusCode, scim_type: Option<&'static str>, detail: impl Into<String>) -> Self {
        Self {
            status,
            scim_type,
            detail: detail.into(),
        }
    }
    fn invalid(detail: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, Some("invalidValue"), detail)
    }
}

impl IntoResponse for Problem {
    fn into_response(self) -> Response {
        let mut body = json!({
            "schemas": [ERROR],
            "status": self.status.as_u16().to_string(),
            "detail": self.detail,
        });
        if let Some(t) = self.scim_type {
            body["scimType"] = json!(t);
        }
        Scim(self.status, body).into_response()
    }
}

impl From<ScimError> for Problem {
    fn from(e: ScimError) -> Self {
        match e {
            ScimError::NotFound => Self::new(StatusCode::NOT_FOUND, None, "Resource not found"),
            ScimError::Conflict(m) => Self::new(StatusCode::CONFLICT, Some("uniqueness"), m),
            ScimError::Invalid(m) => Self::invalid(m),
            ScimError::Auth(huxley_auth::AuthError::InvalidEmail) => {
                Self::invalid("emails: not a valid email address")
            }
            ScimError::Auth(huxley_auth::AuthError::InvalidDisplayName) => {
                Self::invalid("displayName: up to 200 characters")
            }
            ScimError::Auth(huxley_auth::AuthError::EmailTaken) => Self::new(
                StatusCode::CONFLICT,
                Some("uniqueness"),
                "A user with that email exists",
            ),
            other => {
                tracing::error!(error = %other, "SCIM request failed");
                Self::new(StatusCode::INTERNAL_SERVER_ERROR, None, "Internal error")
            }
        }
    }
}

impl From<sqlx::Error> for Problem {
    fn from(e: sqlx::Error) -> Self {
        ScimError::from(e).into()
    }
}

impl From<huxley_db::DbError> for Problem {
    fn from(e: huxley_db::DbError) -> Self {
        ScimError::from(e).into()
    }
}

type ScimResult<T> = Result<T, Problem>;

fn time(t: OffsetDateTime) -> String {
    t.format(&Rfc3339).unwrap_or_default()
}

fn body(bytes: &Bytes) -> ScimResult<Value> {
    serde_json::from_slice(bytes).map_err(|e| {
        Problem::new(
            StatusCode::BAD_REQUEST,
            Some("invalidSyntax"),
            format!("Not valid JSON: {e}"),
        )
    })
}

fn id(raw: &str) -> ScimResult<Uuid> {
    Uuid::parse_str(raw)
        .map_err(|_| Problem::new(StatusCode::NOT_FOUND, None, "Resource not found"))
}

// ---------------------------------------------------------------------------
// Authentication

/// The provider whose SCIM token signed the request, and the context its
/// changes are audited in.
async fn provider(
    state: &AppState,
    headers: &HeaderMap,
    client: ClientInfo,
) -> ScimResult<(ScimProvider, RequestContext)> {
    let unauthorized = || {
        Problem::new(
            StatusCode::UNAUTHORIZED,
            None,
            "A valid SCIM bearer token is required",
        )
    };
    let token = crate::authz::bearer_token(headers).ok_or_else(unauthorized)?;
    if !huxley_auth::token::looks_valid(token, "hxscim_") {
        return Err(unauthorized());
    }
    let mut tx = begin_scoped(&state.db, &OrgScope::System).await?;
    let provider = scim::provider_by_token(&mut tx, token)
        .await?
        .ok_or_else(unauthorized)?;
    tx.commit().await?;
    let ctx = RequestContext {
        via: Via::Api,
        ..provider.context(&client.0)
    };
    Ok((provider, ctx))
}

// ---------------------------------------------------------------------------
// Resources

fn user_json(u: &ScimUser) -> Value {
    let mut out = json!({
        "schemas": [USER],
        "id": u.id,
        "userName": u.user_name,
        "displayName": u.display_name,
        "emails": [{ "value": u.email, "type": "work", "primary": true }],
        "active": u.active,
        "meta": {
            "resourceType": "User",
            "created": time(u.created_at),
            "lastModified": time(u.updated_at),
            "location": format!("/scim/v2/Users/{}", u.id),
        },
    });
    if let Some(e) = &u.external_id {
        out["externalId"] = json!(e);
    }
    if u.name.as_object().is_some_and(|m| !m.is_empty()) {
        out["name"] = u.name.clone();
    }
    out
}

fn group_json(g: &ScimGroup, members: bool) -> Value {
    let mut out = json!({
        "schemas": [GROUP],
        "id": g.id,
        "displayName": g.display_name,
        "meta": {
            "resourceType": "Group",
            "created": time(g.created_at),
            "lastModified": time(g.updated_at),
            "location": format!("/scim/v2/Groups/{}", g.id),
        },
    });
    if members {
        out["members"] = g
            .members
            .iter()
            .map(|(id, name)| json!({ "value": id, "display": name, "$ref": format!("/scim/v2/Users/{id}") }))
            .collect();
    }
    if let Some(e) = &g.external_id {
        out["externalId"] = json!(e);
    }
    out
}

fn list(total: i64, start: i64, resources: Vec<Value>) -> Value {
    json!({
        "schemas": [LIST],
        "totalResults": total,
        "startIndex": start,
        "itemsPerPage": resources.len(),
        "Resources": Value::Array(resources),
    })
}

/// A JSON boolean, or the strings "true"/"false" (Entra ID sends those).
fn boolean(v: &Value) -> Option<bool> {
    match v {
        Value::Bool(b) => Some(*b),
        Value::String(s) if s.eq_ignore_ascii_case("true") => Some(true),
        Value::String(s) if s.eq_ignore_ascii_case("false") => Some(false),
        _ => None,
    }
}

fn text(v: &Value) -> Option<String> {
    v.as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

fn user_input(doc: &Value) -> ScimResult<UserInput> {
    let user_name =
        text(&doc["userName"]).ok_or_else(|| Problem::invalid("userName is required"))?;
    let emails = doc["emails"].as_array().cloned().unwrap_or_default();
    let email = emails
        .iter()
        .find(|e| boolean(&e["primary"]) == Some(true))
        .or_else(|| emails.first())
        .and_then(|e| text(&e["value"]));
    Ok(UserInput {
        user_name,
        external_id: text(&doc["externalId"]),
        name: doc["name"]
            .as_object()
            .map_or_else(|| json!({}), |m| Value::Object(m.clone())),
        display_name: text(&doc["displayName"]),
        email,
        active: doc.get("active").is_none_or(|a| boolean(a).unwrap_or(true)),
    })
}

fn group_input(doc: &Value) -> ScimResult<GroupInput> {
    let display_name =
        text(&doc["displayName"]).ok_or_else(|| Problem::invalid("displayName is required"))?;
    let mut members = Vec::new();
    for m in doc["members"].as_array().into_iter().flatten() {
        let raw = text(&m["value"]).ok_or_else(|| Problem::invalid("members need a value"))?;
        let id = Uuid::parse_str(&raw)
            .map_err(|_| Problem::invalid(format!("members: no user {raw}")))?;
        if !members.contains(&id) {
            members.push(id);
        }
    }
    Ok(GroupInput {
        display_name,
        external_id: text(&doc["externalId"]),
        members,
    })
}

// ---------------------------------------------------------------------------
// Filters and paging

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListParams {
    filter: Option<String>,
    start_index: Option<i64>,
    count: Option<i64>,
    attributes: Option<String>,
    excluded_attributes: Option<String>,
}

impl ListParams {
    /// (1-based start, offset, count)
    fn page(&self) -> (i64, i64, i64) {
        let start = self.start_index.unwrap_or(1).max(1);
        (
            start,
            start - 1,
            self.count.unwrap_or(100).clamp(0, MAX_COUNT),
        )
    }
}

/// `attr eq "value"`: the only filter SCIM clients send for lookups.
fn parse_eq(filter: &str) -> ScimResult<(String, String)> {
    let bad = || {
        Problem::new(
            StatusCode::BAD_REQUEST,
            Some("invalidFilter"),
            format!("Unsupported filter: {filter}"),
        )
    };
    let filter = filter.trim();
    // The attribute ends at the first space outside brackets:
    // `emails[type eq "work"].value eq "…"`.
    let mut depth = 0i32;
    let end = filter
        .char_indices()
        .find(|&(_, c)| {
            match c {
                '[' => depth += 1,
                ']' => depth -= 1,
                _ => {}
            }
            depth == 0 && c.is_whitespace()
        })
        .map(|(i, _)| i)
        .ok_or_else(bad)?;
    let (attr, rest) = (&filter[..end], &filter[end..]);
    let rest = rest.trim_start();
    let (op, value) = rest.split_once(char::is_whitespace).ok_or_else(bad)?;
    if !op.eq_ignore_ascii_case("eq") {
        return Err(bad());
    }
    let value = value.trim();
    let value = serde_json::from_str::<String>(value).map_err(|_| bad())?;
    Ok((attr.to_owned(), value))
}

fn user_filter(filter: Option<&str>) -> ScimResult<Option<UserFilter>> {
    let Some(f) = filter.filter(|f| !f.trim().is_empty()) else {
        return Ok(None);
    };
    let (attr, value) = parse_eq(f)?;
    let attr = attr.to_ascii_lowercase();
    let attr = attr
        .strip_prefix(&format!("{}:", USER.to_ascii_lowercase()))
        .unwrap_or(&attr);
    Ok(Some(match attr {
        "username" => UserFilter::UserName(value),
        "externalid" => UserFilter::ExternalId(value),
        "emails.value" | "emails" => UserFilter::Email(value),
        a if a.starts_with("emails[") && a.ends_with("].value") => UserFilter::Email(value),
        _ => {
            return Err(Problem::new(
                StatusCode::BAD_REQUEST,
                Some("invalidFilter"),
                format!(
                    "Users can be filtered by userName, externalId or emails.value, not {attr}"
                ),
            ));
        }
    }))
}

fn group_filter(filter: Option<&str>) -> ScimResult<Option<GroupFilter>> {
    let Some(f) = filter.filter(|f| !f.trim().is_empty()) else {
        return Ok(None);
    };
    let (attr, value) = parse_eq(f)?;
    Ok(Some(match attr.to_ascii_lowercase().as_str() {
        "displayname" => GroupFilter::DisplayName(value),
        "externalid" => GroupFilter::ExternalId(value),
        other => {
            return Err(Problem::new(
                StatusCode::BAD_REQUEST,
                Some("invalidFilter"),
                format!("Groups can be filtered by displayName or externalId, not {other}"),
            ));
        }
    }))
}

/// Whether to include group members: not when the client excludes them or
/// asks only for other attributes.
fn want_members(params: &ListParams) -> bool {
    let has = |list: &Option<String>| {
        list.as_deref().is_some_and(|l| {
            l.split(',')
                .any(|a| a.trim().eq_ignore_ascii_case("members"))
        })
    };
    if has(&params.excluded_attributes) {
        return false;
    }
    params.attributes.is_none() || has(&params.attributes)
}

// ---------------------------------------------------------------------------
// PATCH

/// A PATCH path: `attr`, `attr.sub`, `attr[sub eq "v"]` or
/// `attr[sub eq "v"].sub`, without the core schema's URN.
struct PatchPath {
    attr: String,
    filter: Option<(String, String)>,
    sub: Option<String>,
}

fn parse_path(raw: &str, schema: &str) -> ScimResult<Option<PatchPath>> {
    let mut path = raw.trim();
    if let Some(rest) = path.strip_prefix(schema).and_then(|r| r.strip_prefix(':')) {
        path = rest;
    } else if path.starts_with("urn:") {
        // An extension schema (enterprise user and the like): not stored.
        return Ok(None);
    }
    let (head, sub_after_filter, filter) = match path.split_once('[') {
        Some((attr, rest)) => {
            let (inner, after) = rest.split_once(']').ok_or_else(|| {
                Problem::new(
                    StatusCode::BAD_REQUEST,
                    Some("invalidPath"),
                    format!("Bad path: {raw}"),
                )
            })?;
            (
                attr,
                after.strip_prefix('.').map(str::to_owned),
                Some(parse_eq(inner)?),
            )
        }
        None => (path, None, None),
    };
    let (attr, sub) = match (filter.is_some(), head.split_once('.')) {
        (false, Some((a, s))) => (a.to_owned(), Some(s.to_owned())),
        _ => (head.to_owned(), sub_after_filter),
    };
    Ok(Some(PatchPath { attr, filter, sub }))
}

fn key_of(doc: &Map<String, Value>, attr: &str) -> String {
    doc.keys()
        .find(|k| k.eq_ignore_ascii_case(attr))
        .cloned()
        .unwrap_or_else(|| attr.to_owned())
}

fn matches(element: &Value, (sub, value): &(String, String)) -> bool {
    element
        .as_object()
        .and_then(|m| m.iter().find(|(k, _)| k.eq_ignore_ascii_case(sub)))
        .is_some_and(|(_, v)| {
            v.as_str().is_some_and(|s| s.eq_ignore_ascii_case(value))
                || serde_json::from_str::<Value>(value).ok().as_ref() == Some(v)
        })
}

fn same_value(a: &Value, b: &Value) -> bool {
    a.get("value").is_some() && a.get("value") == b.get("value")
}

/// Applies SCIM PATCH operations to a resource's JSON.
fn apply_patch(doc: &mut Value, operations: &[Value], schema: &str) -> ScimResult<()> {
    for op in operations {
        let kind = op["op"].as_str().unwrap_or_default().to_ascii_lowercase();
        if !matches!(kind.as_str(), "add" | "replace" | "remove") {
            return Err(Problem::invalid(format!(
                "Unsupported operation: {}",
                op["op"]
            )));
        }
        let value = op.get("value").cloned().unwrap_or(Value::Null);
        let Some(raw_path) = op["path"].as_str() else {
            // No path: the value is an object of attributes (Okta, Entra).
            let Some(attrs) = value.as_object() else {
                return Err(Problem::new(
                    StatusCode::BAD_REQUEST,
                    Some("noTarget"),
                    "An operation without a path needs an object value",
                ));
            };
            for (k, v) in attrs {
                apply_one(doc, &kind, k, v.clone(), schema)?;
            }
            continue;
        };
        apply_one(doc, &kind, raw_path, value, schema)?;
    }
    Ok(())
}

/// An operation on the elements of a multi-valued attribute that match a
/// filter: `attr[filter]` or `attr[filter].sub`.
fn apply_filtered(
    items: &mut Vec<Value>,
    kind: &str,
    filter: &(String, String),
    sub: Option<&str>,
    value: &Value,
) {
    match (kind, sub) {
        ("remove", None) => items.retain(|e| !matches(e, filter)),
        ("remove", Some(sub)) => {
            for e in items.iter_mut().filter(|e| matches(e, filter)) {
                if let Some(m) = e.as_object_mut() {
                    let k = key_of(m, sub);
                    m.remove(&k);
                }
            }
        }
        (_, Some(sub)) => {
            if !items.iter().any(|e| matches(e, filter)) {
                items.push(json!({ filter.0.clone(): filter.1.clone() }));
            }
            for e in items.iter_mut().filter(|e| matches(e, filter)) {
                if let Some(m) = e.as_object_mut() {
                    let k = key_of(m, sub);
                    m.insert(k, value.clone());
                }
            }
        }
        (_, None) => {
            items.retain(|e| !matches(e, filter));
            items.extend(
                value
                    .as_array()
                    .cloned()
                    .unwrap_or_else(|| vec![value.clone()]),
            );
        }
    }
}

fn apply_one(
    doc: &mut Value,
    kind: &str,
    raw_path: &str,
    value: Value,
    schema: &str,
) -> ScimResult<()> {
    let Some(path) = parse_path(raw_path, schema)? else {
        return Ok(());
    };
    let obj = doc.as_object_mut().expect("resources are objects");
    let key = key_of(obj, &path.attr);
    match (&path.filter, &path.sub) {
        // attr[filter](.sub)
        (Some(filter), sub) => {
            let list = obj.entry(key).or_insert_with(|| json!([]));
            if !list.is_array() {
                *list = json!([]);
            }
            let items = list.as_array_mut().expect("just made an array");
            apply_filtered(items, kind, filter, sub.as_deref(), &value);
        }
        // attr.sub
        (None, Some(sub)) => {
            let parent = obj.entry(key).or_insert_with(|| json!({}));
            if !parent.is_object() {
                *parent = json!({});
            }
            let m = parent.as_object_mut().expect("just made an object");
            let k = key_of(m, sub);
            if kind == "remove" {
                m.remove(&k);
            } else {
                m.insert(k, value);
            }
        }
        // attr
        (None, None) => {
            let multi = obj.get(&key).is_some_and(Value::is_array) || value.is_array();
            match kind {
                "remove" if multi && !value.is_null() => {
                    let drop = value
                        .as_array()
                        .cloned()
                        .unwrap_or_else(|| vec![value.clone()]);
                    if let Some(items) = obj.get_mut(&key).and_then(Value::as_array_mut) {
                        items.retain(|e| !drop.iter().any(|d| same_value(e, d)));
                    }
                }
                "remove" => {
                    obj.remove(&key);
                }
                "add" if multi => {
                    let add = value
                        .as_array()
                        .cloned()
                        .unwrap_or_else(|| vec![value.clone()]);
                    let items = obj.entry(key).or_insert_with(|| json!([]));
                    if !items.is_array() {
                        *items = json!([]);
                    }
                    let items = items.as_array_mut().expect("just made an array");
                    for a in add {
                        if !items.iter().any(|e| same_value(e, &a)) {
                            items.push(a);
                        }
                    }
                }
                "add" if value.is_object() && obj.get(&key).is_some_and(Value::is_object) => {
                    let target = obj
                        .get_mut(&key)
                        .and_then(Value::as_object_mut)
                        .expect("an object");
                    for (k, v) in value.as_object().expect("an object") {
                        target.insert(k.clone(), v.clone());
                    }
                }
                _ => {
                    obj.insert(key, value);
                }
            }
        }
    }
    Ok(())
}

fn patch_operations(doc: &Value) -> ScimResult<Vec<Value>> {
    let ok_schema = doc["schemas"]
        .as_array()
        .is_some_and(|s| s.iter().any(|v| v == PATCH));
    if !ok_schema {
        return Err(Problem::new(
            StatusCode::BAD_REQUEST,
            Some("invalidSyntax"),
            format!("A PATCH body has the schema {PATCH}"),
        ));
    }
    let ops = doc
        .get("Operations")
        .or_else(|| doc.get("operations"))
        .and_then(Value::as_array)
        .cloned()
        .ok_or_else(|| {
            Problem::new(
                StatusCode::BAD_REQUEST,
                Some("invalidSyntax"),
                "Operations are required",
            )
        })?;
    Ok(ops)
}

// ---------------------------------------------------------------------------
// Handlers

async fn list_users(
    State(state): State<AppState>,
    headers: HeaderMap,
    client: ClientInfo,
    Query(params): Query<ListParams>,
) -> ScimResult<Scim> {
    let (provider, _) = provider(&state, &headers, client).await?;
    let filter = user_filter(params.filter.as_deref())?;
    let (start, offset, count) = params.page();
    let mut tx = begin_scoped(&state.db, &OrgScope::System).await?;
    let (total, users) =
        scim::list_users(&mut tx, &provider, filter.as_ref(), offset, count).await?;
    tx.commit().await?;
    Ok(ok(list(
        total,
        start,
        users.iter().map(user_json).collect(),
    )))
}

async fn get_user(
    State(state): State<AppState>,
    headers: HeaderMap,
    client: ClientInfo,
    Path(raw): Path<String>,
) -> ScimResult<Scim> {
    let (provider, _) = provider(&state, &headers, client).await?;
    let mut tx = begin_scoped(&state.db, &OrgScope::System).await?;
    let user = scim::get_user(&mut tx, &provider, id(&raw)?).await?;
    tx.commit().await?;
    Ok(ok(user_json(&user)))
}

async fn create_user(
    State(state): State<AppState>,
    headers: HeaderMap,
    client: ClientInfo,
    bytes: Bytes,
) -> ScimResult<Scim> {
    let (provider, ctx) = provider(&state, &headers, client).await?;
    let input = user_input(&body(&bytes)?)?;
    let mut tx = begin_scoped(&state.db, &OrgScope::System).await?;
    let user = scim::create_user(&mut tx, &ctx, &provider, &input).await?;
    tx.commit().await?;
    Ok(Scim(StatusCode::CREATED, user_json(&user)))
}

async fn put_user(
    State(state): State<AppState>,
    headers: HeaderMap,
    client: ClientInfo,
    Path(raw): Path<String>,
    bytes: Bytes,
) -> ScimResult<Scim> {
    let (provider, ctx) = provider(&state, &headers, client).await?;
    let input = user_input(&body(&bytes)?)?;
    let mut tx = begin_scoped(&state.db, &OrgScope::System).await?;
    let user = scim::replace_user(&mut tx, &ctx, &provider, id(&raw)?, &input).await?;
    tx.commit().await?;
    Ok(ok(user_json(&user)))
}

async fn patch_user(
    State(state): State<AppState>,
    headers: HeaderMap,
    client: ClientInfo,
    Path(raw): Path<String>,
    bytes: Bytes,
) -> ScimResult<Scim> {
    let (provider, ctx) = provider(&state, &headers, client).await?;
    let ops = patch_operations(&body(&bytes)?)?;
    let id = id(&raw)?;
    let mut tx = begin_scoped(&state.db, &OrgScope::System).await?;
    let mut doc = user_json(&scim::get_user(&mut tx, &provider, id).await?);
    apply_patch(&mut doc, &ops, USER)?;
    let user = scim::replace_user(&mut tx, &ctx, &provider, id, &user_input(&doc)?).await?;
    tx.commit().await?;
    Ok(ok(user_json(&user)))
}

async fn delete_user(
    State(state): State<AppState>,
    headers: HeaderMap,
    client: ClientInfo,
    Path(raw): Path<String>,
) -> ScimResult<StatusCode> {
    let (provider, ctx) = provider(&state, &headers, client).await?;
    let mut tx = begin_scoped(&state.db, &OrgScope::System).await?;
    scim::delete_user(&mut tx, &ctx, &provider, id(&raw)?).await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn list_groups(
    State(state): State<AppState>,
    headers: HeaderMap,
    client: ClientInfo,
    Query(params): Query<ListParams>,
) -> ScimResult<Scim> {
    let (provider, _) = provider(&state, &headers, client).await?;
    let filter = group_filter(params.filter.as_deref())?;
    let (start, offset, count) = params.page();
    let members = want_members(&params);
    let mut tx = begin_scoped(&state.db, &OrgScope::System).await?;
    let (total, groups) =
        scim::list_groups(&mut tx, &provider, filter.as_ref(), offset, count, members).await?;
    tx.commit().await?;
    Ok(ok(list(
        total,
        start,
        groups.iter().map(|g| group_json(g, members)).collect(),
    )))
}

async fn get_group(
    State(state): State<AppState>,
    headers: HeaderMap,
    client: ClientInfo,
    Path(raw): Path<String>,
    Query(params): Query<ListParams>,
) -> ScimResult<Scim> {
    let (provider, _) = provider(&state, &headers, client).await?;
    let mut tx = begin_scoped(&state.db, &OrgScope::System).await?;
    let group = scim::get_group(&mut tx, &provider, id(&raw)?).await?;
    tx.commit().await?;
    Ok(ok(group_json(&group, want_members(&params))))
}

async fn create_group(
    State(state): State<AppState>,
    headers: HeaderMap,
    client: ClientInfo,
    bytes: Bytes,
) -> ScimResult<Scim> {
    let (provider, ctx) = provider(&state, &headers, client).await?;
    let input = group_input(&body(&bytes)?)?;
    let mut tx = begin_scoped(&state.db, &OrgScope::System).await?;
    let group = scim::create_group(&mut tx, &ctx, &provider, &input).await?;
    tx.commit().await?;
    Ok(Scim(StatusCode::CREATED, group_json(&group, true)))
}

async fn put_group(
    State(state): State<AppState>,
    headers: HeaderMap,
    client: ClientInfo,
    Path(raw): Path<String>,
    bytes: Bytes,
) -> ScimResult<Scim> {
    let (provider, ctx) = provider(&state, &headers, client).await?;
    let input = group_input(&body(&bytes)?)?;
    let mut tx = begin_scoped(&state.db, &OrgScope::System).await?;
    let group = scim::replace_group(&mut tx, &ctx, &provider, id(&raw)?, &input).await?;
    tx.commit().await?;
    Ok(ok(group_json(&group, true)))
}

async fn patch_group(
    State(state): State<AppState>,
    headers: HeaderMap,
    client: ClientInfo,
    Path(raw): Path<String>,
    bytes: Bytes,
) -> ScimResult<Response> {
    let (provider, ctx) = provider(&state, &headers, client).await?;
    let ops = patch_operations(&body(&bytes)?)?;
    let id = id(&raw)?;
    let mut tx = begin_scoped(&state.db, &OrgScope::System).await?;
    let mut doc = group_json(&scim::get_group(&mut tx, &provider, id).await?, true);
    apply_patch(&mut doc, &ops, GROUP)?;
    scim::replace_group(&mut tx, &ctx, &provider, id, &group_input(&doc)?).await?;
    tx.commit().await?;
    // Large groups: membership changes answer without the member list.
    Ok(StatusCode::NO_CONTENT.into_response())
}

async fn delete_group(
    State(state): State<AppState>,
    headers: HeaderMap,
    client: ClientInfo,
    Path(raw): Path<String>,
) -> ScimResult<StatusCode> {
    let (provider, ctx) = provider(&state, &headers, client).await?;
    let mut tx = begin_scoped(&state.db, &OrgScope::System).await?;
    scim::delete_group(&mut tx, &ctx, &provider, id(&raw)?).await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

// ---------------------------------------------------------------------------
// Discovery

async fn service_provider_config(
    State(state): State<AppState>,
    headers: HeaderMap,
    client: ClientInfo,
) -> ScimResult<Scim> {
    provider(&state, &headers, client).await?;
    Ok(ok(json!({
        "schemas": ["urn:ietf:params:scim:schemas:core:2.0:ServiceProviderConfig"],
        "documentationUri": "https://usehuxley.io/docs/scim",
        "patch": { "supported": true },
        "bulk": { "supported": false, "maxOperations": 0, "maxPayloadSize": 0 },
        "filter": { "supported": true, "maxResults": MAX_COUNT },
        "changePassword": { "supported": false },
        "sort": { "supported": false },
        "etag": { "supported": false },
        "authenticationSchemes": [{
            "type": "oauthbearertoken",
            "name": "Bearer token",
            "description": "The SCIM token from Settings → Single sign-on → the provider → Provisioning",
            "primary": true,
        }],
    })))
}

async fn resource_types(
    State(state): State<AppState>,
    headers: HeaderMap,
    client: ClientInfo,
) -> ScimResult<Scim> {
    provider(&state, &headers, client).await?;
    let types = vec![
        json!({ "schemas": ["urn:ietf:params:scim:schemas:core:2.0:ResourceType"], "id": "User", "name": "User",
                "endpoint": "/Users", "schema": USER }),
        json!({ "schemas": ["urn:ietf:params:scim:schemas:core:2.0:ResourceType"], "id": "Group", "name": "Group",
                "endpoint": "/Groups", "schema": GROUP }),
    ];
    Ok(ok(list(2, 1, types)))
}

fn attribute(name: &str, kind: &str, required: bool, multi: bool) -> Value {
    json!({ "name": name, "type": kind, "multiValued": multi, "required": required,
            "caseExact": false, "mutability": "readWrite", "returned": "default",
            "uniqueness": if name == "userName" || name == "displayName" { "server" } else { "none" } })
}

async fn schemas(
    State(state): State<AppState>,
    headers: HeaderMap,
    client: ClientInfo,
) -> ScimResult<Scim> {
    provider(&state, &headers, client).await?;
    let user = json!({
        "schemas": ["urn:ietf:params:scim:schemas:core:2.0:Schema"], "id": USER, "name": "User",
        "attributes": [
            attribute("userName", "string", true, false), attribute("externalId", "string", false, false),
            attribute("name", "complex", false, false), attribute("displayName", "string", false, false),
            attribute("emails", "complex", false, true), attribute("active", "boolean", false, false),
        ],
    });
    let group = json!({
        "schemas": ["urn:ietf:params:scim:schemas:core:2.0:Schema"], "id": GROUP, "name": "Group",
        "attributes": [
            attribute("displayName", "string", true, false), attribute("externalId", "string", false, false),
            attribute("members", "complex", false, true),
        ],
    });
    Ok(ok(list(2, 1, vec![user, group])))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user() -> Value {
        json!({ "schemas": [USER], "userName": "ada@example.com", "active": true,
                "emails": [{ "value": "ada@example.com", "type": "work", "primary": true }],
                "name": { "givenName": "Ada", "familyName": "Lovelace" } })
    }

    #[test]
    fn entra_style_patches_apply() {
        let mut doc = user();
        apply_patch(
            &mut doc,
            &[
                json!({ "op": "Replace", "path": "active", "value": "False" }),
                json!({ "op": "Replace", "path": "emails[type eq \"work\"].value", "value": "ada@new.example" }),
                json!({ "op": "Add", "path": "name.familyName", "value": "Byron" }),
                json!({ "op": "Add", "path": "urn:ietf:params:scim:schemas:extension:enterprise:2.0:User:department", "value": "Eng" }),
            ],
            USER,
        )
        .unwrap();
        let input = user_input(&doc).unwrap();
        assert!(!input.active);
        assert_eq!(input.email.as_deref(), Some("ada@new.example"));
        assert_eq!(input.name["familyName"], "Byron");
    }

    #[test]
    fn okta_style_patches_apply() {
        let mut doc = user();
        apply_patch(
            &mut doc,
            &[json!({ "op": "replace", "value": { "active": false, "displayName": "Ada L" } })],
            USER,
        )
        .unwrap();
        let input = user_input(&doc).unwrap();
        assert!(!input.active);
        assert_eq!(input.display_name.as_deref(), Some("Ada L"));
    }

    #[test]
    fn member_patches_apply() {
        let (a, b, c) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
        let mut doc = json!({ "displayName": "Eng", "members": [{ "value": a }] });
        apply_patch(
            &mut doc,
            &[
                json!({ "op": "Add", "path": "members", "value": [{ "value": b }, { "value": a }] }),
                json!({ "op": "Add", "path": "members", "value": [{ "value": c }] }),
                json!({ "op": "Remove", "path": format!("members[value eq \"{a}\"]") }),
                json!({ "op": "Remove", "path": "members", "value": [{ "value": c }] }),
            ],
            GROUP,
        )
        .unwrap();
        assert_eq!(group_input(&doc).unwrap().members, vec![b]);
        apply_patch(
            &mut doc,
            &[json!({ "op": "replace", "path": "displayName", "value": "Engineering" })],
            GROUP,
        )
        .unwrap();
        assert_eq!(group_input(&doc).unwrap().display_name, "Engineering");
    }

    mod random {
        use super::*;
        use proptest::prelude::*;

        fn json_value() -> impl Strategy<Value = Value> {
            let leaf = prop_oneof![
                Just(Value::Null),
                any::<bool>().prop_map(Value::Bool),
                any::<i64>().prop_map(|n| json!(n)),
                "\\PC{0,20}".prop_map(Value::String),
            ];
            leaf.prop_recursive(4, 32, 6, |inner| {
                prop_oneof![
                    proptest::collection::vec(inner.clone(), 0..6).prop_map(Value::Array),
                    proptest::collection::btree_map("[a-zA-Z.\\[\\]\" ]{0,12}", inner, 0..6)
                        .prop_map(|m| Value::Object(m.into_iter().collect())),
                ]
            })
        }

        proptest! {
            #![proptest_config(ProptestConfig::with_cases(512))]

            /// Identity providers send these; anything must parse or fail cleanly.
            #[test]
            fn any_filter_parses_or_fails_cleanly(text in "\\PC{0,80}") {
                let _ = user_filter(Some(&text));
                let _ = group_filter(Some(&text));
            }

            #[test]
            fn any_patch_applies_or_fails_cleanly(
                ops in proptest::collection::vec(
                    (prop::sample::select(vec!["add", "Replace", "remove", "copy"]),
                     proptest::option::of("[a-zA-Z.:\\[\\]\" =0-9]{0,40}"),
                     json_value()),
                    0..8,
                ),
            ) {
                let ops: Vec<Value> = ops
                    .into_iter()
                    .map(|(op, path, value)| {
                        let mut o = json!({ "op": op, "value": value });
                        if let Some(path) = path {
                            o["path"] = json!(path);
                        }
                        o
                    })
                    .collect();
                let mut doc = user();
                if apply_patch(&mut doc, &ops, USER).is_ok() {
                    let _ = user_input(&doc);
                }
                let mut doc = json!({ "displayName": "G", "members": [] });
                if apply_patch(&mut doc, &ops, GROUP).is_ok() {
                    let _ = group_input(&doc);
                }
            }
        }
    }

    #[test]
    fn filters_parse() {
        assert!(matches!(
            user_filter(Some("userName eq \"ada@example.com\"")).unwrap(),
            Some(UserFilter::UserName(v)) if v == "ada@example.com"
        ));
        assert!(matches!(
            user_filter(Some("emails[type eq \"work\"].value eq \"a@b.c\"")).unwrap(),
            Some(UserFilter::Email(_))
        ));
        assert!(matches!(
            group_filter(Some("displayName eq \"Eng\"")).unwrap(),
            Some(GroupFilter::DisplayName(_))
        ));
        assert!(user_filter(Some("userName sw \"a\"")).is_err());
        assert!(user_filter(Some("title eq \"x\"")).is_err());
    }
}
