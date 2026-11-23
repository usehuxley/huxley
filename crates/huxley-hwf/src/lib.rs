//! The Huxley Workflow Format (HWF): the one document format that the canvas,
//! workflows-as-code and MCP all read and write (design doc §6.2, ADR-004).
//!
//! - [`model`]: the v1 types. Unknown fields are errors.
//! - [`parse_yaml`] / [`parse_json`]: read a document; [`to_yaml`] writes one.
//! - [`validate`]: structural checks beyond the types.
//! - [`json_schema`]: the published JSON Schema (`schemas/hwf/v1.json`).

pub mod duration;
pub mod model;
mod validate;

pub use model::*;
pub use validate::{API_VERSION, Diagnostic, is_valid_key, is_valid_step_id, validate};

/// Largest document accepted, in bytes.
pub const MAX_DOCUMENT_BYTES: usize = 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum HwfError {
    #[error("workflow documents are limited to 1 MiB")]
    TooLarge,
    /// The document isn't well-formed YAML/JSON or doesn't match the types.
    /// The message includes the location.
    #[error("{0}")]
    Parse(String),
    #[error("{}", .0.iter().map(ToString::to_string).collect::<Vec<_>>().join("\n"))]
    Invalid(Vec<Diagnostic>),
}

/// Reads a YAML document (JSON is valid YAML too) and validates it.
pub fn parse_yaml(text: &str) -> Result<Workflow, HwfError> {
    if text.len() > MAX_DOCUMENT_BYTES {
        return Err(HwfError::TooLarge);
    }
    let options = serde_saphyr::options! {
        // YAML 1.2: `yes`/`no`/`on`/`off` stay strings.
        strict_booleans: true,
    };
    let workflow: Workflow = serde_saphyr::from_str_with_options(text, options)
        .map_err(|e| HwfError::Parse(e.to_string()))?;
    checked(workflow)
}

/// Reads a JSON document and validates it.
pub fn parse_json(text: &str) -> Result<Workflow, HwfError> {
    if text.len() > MAX_DOCUMENT_BYTES {
        return Err(HwfError::TooLarge);
    }
    let workflow: Workflow =
        serde_json::from_str(text).map_err(|e| HwfError::Parse(e.to_string()))?;
    checked(workflow)
}

/// Validates an already-deserialized document (e.g. from a JSON request body).
pub fn from_value(value: serde_json::Value) -> Result<Workflow, HwfError> {
    let workflow: Workflow =
        serde_json::from_value(value).map_err(|e| HwfError::Parse(e.to_string()))?;
    checked(workflow)
}

fn checked(workflow: Workflow) -> Result<Workflow, HwfError> {
    let diagnostics = validate(&workflow);
    if diagnostics.is_empty() {
        Ok(workflow)
    } else {
        Err(HwfError::Invalid(diagnostics))
    }
}

/// Writes a document as YAML.
pub fn to_yaml(workflow: &Workflow) -> String {
    serde_saphyr::to_string(workflow).expect("HWF types always serialize")
}

/// The JSON Schema for HWF v1 documents, with keys sorted so the output is
/// the same however `serde_json` is built (`preserve_order` or not).
pub fn json_schema() -> serde_json::Value {
    let mut schema = schemars::schema_for!(Workflow).to_value();
    if let Some(object) = schema.as_object_mut() {
        object.insert(
            "$id".into(),
            "https://usehuxley.com/schemas/hwf/v1.json".into(),
        );
    }
    sorted(schema)
}

fn sorted(value: serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Object(map) => {
            let mut entries: Vec<_> = map.into_iter().collect();
            entries.sort_by(|a, b| a.0.cmp(&b.0));
            serde_json::Value::Object(entries.into_iter().map(|(k, v)| (k, sorted(v))).collect())
        }
        serde_json::Value::Array(items) => {
            serde_json::Value::Array(items.into_iter().map(sorted).collect())
        }
        other => other,
    }
}
