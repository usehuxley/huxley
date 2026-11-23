//! Integration definitions (design doc §7.1, ADR-023).
//!
//! [`import`] reads an OpenAPI 3.0 or 3.1 document, or a Swagger 2.0 one
//! (converted on the way), as JSON or YAML, and produces a [`Definition`]:
//! the API's servers, its auth schemes, and one [`Action`] per operation,
//! each with a single JSON Schema for its inputs (path, query, header and
//! cookie parameters plus the body) and one for its output.
//!
//! The importer works on the document as plain JSON rather than typed
//! OpenAPI structures, so the small mistakes common in published specs
//! (a missing `info.version`, a stray field, a dangling `$ref`) become
//! [`Warning`]s instead of failing the whole import.

pub mod connections;
pub mod diff;
mod import;
pub mod oauth;
mod refs;
pub mod store;
mod swagger;
pub mod usage;

use serde::{Deserialize, Serialize};
use serde_json::Value as Json;
use utoipa::ToSchema;

pub use import::{import, import_value, parse};

/// Largest spec accepted, in bytes.
pub const MAX_SPEC_BYTES: usize = 20 * 1024 * 1024;
/// Most operations imported from one spec.
pub const MAX_ACTIONS: usize = 5000;

#[derive(Debug, thiserror::Error)]
pub enum ImportError {
    #[error("the spec is larger than {} MB", MAX_SPEC_BYTES / 1024 / 1024)]
    TooLarge,
    #[error("the spec isn't valid JSON or YAML: {0}")]
    Parse(String),
    #[error("not an OpenAPI 3.x or Swagger 2.0 document: {0}")]
    NotOpenApi(String),
    #[error("the spec has more than {MAX_ACTIONS} operations")]
    TooManyActions,
}

/// An API, as Huxley uses it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Definition {
    pub title: String,
    /// The API's own version (`info.version`).
    pub version: String,
    pub description: Option<String>,
    /// Which format it came from: `3.1.0`, `3.0.3`, `2.0`, …
    pub source_format: String,
    pub servers: Vec<Server>,
    pub auth_schemes: Vec<AuthScheme>,
    pub actions: Vec<Action>,
}

/// A base URL, possibly with `{variables}` that connections fill in.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Server {
    pub url: String,
    pub description: Option<String>,
    pub variables: Vec<ServerVariable>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ServerVariable {
    pub name: String,
    pub default: String,
    /// Allowed values, if the spec limits them.
    pub options: Vec<String>,
    pub description: Option<String>,
}

/// How a connection authenticates.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AuthScheme {
    /// The spec's name for it (the `securitySchemes` key).
    pub key: String,
    pub description: Option<String>,
    #[serde(flatten)]
    pub kind: AuthKind,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum AuthKind {
    /// A key sent in a header, query parameter or cookie.
    ApiKey {
        location: ParamLocation,
        name: String,
    },
    Basic,
    Bearer {
        format: Option<String>,
    },
    #[serde(rename_all = "camelCase")]
    OAuth2ClientCredentials {
        token_url: String,
        scopes: Vec<String>,
    },
    #[serde(rename_all = "camelCase")]
    OAuth2AuthorizationCode {
        authorization_url: String,
        token_url: String,
        refresh_url: Option<String>,
        scopes: Vec<String>,
    },
    #[serde(rename_all = "camelCase")]
    OpenIdConnect {
        discovery_url: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum ParamLocation {
    Path,
    Query,
    Header,
    Cookie,
}

/// One operation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Action {
    /// Unique in the definition: the `operationId` made safe, or
    /// `<method>_<path>` when there is none.
    pub key: String,
    pub name: String,
    pub description: Option<String>,
    /// The first tag, for grouping.
    pub group: Option<String>,
    pub method: String,
    /// The path template, e.g. `/users/{id}`.
    pub path: String,
    pub parameters: Vec<Parameter>,
    pub body: Option<Body>,
    /// One object schema: each parameter by name, plus `body`.
    #[schema(value_type = Object)]
    pub input_schema: Json,
    /// The JSON schema of the first successful response, if it has one.
    #[schema(value_type = Option<Object>)]
    pub output_schema: Option<Json>,
    /// Auth scheme keys any one of which works; empty means none is needed
    /// (or the spec doesn't say).
    pub auth: Vec<String>,
    pub deprecated: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Parameter {
    pub name: String,
    pub location: ParamLocation,
    pub required: bool,
    pub description: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Body {
    /// e.g. `application/json`, `application/x-www-form-urlencoded`.
    pub content_type: String,
    pub required: bool,
}

/// Something the importer skipped or guessed, and where.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct Warning {
    /// A JSON pointer into the spec, e.g. `/paths/~1users/get`.
    pub pointer: String,
    pub message: String,
}

/// A definition and what came up while importing it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct Imported {
    pub definition: Definition,
    pub warnings: Vec<Warning>,
}
