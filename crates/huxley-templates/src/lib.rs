//! Built-in templates (design doc §6.6): a workflow, and optionally the
//! form that starts it, with **bindings** someone fills in when using the
//! template ("who to tell", "which script").
//!
//! A binding is a `%{key}` placeholder in a string of the template. It's
//! resolved after the YAML is parsed, so a value can never change the
//! document's structure or turn into an expression: a string that is just
//! `%{key}` takes the binding's value as typed (a list, for recipients),
//! and a placeholder inside a longer string takes it as text. `%{form}` is
//! the key of the form the template creates.

use std::collections::BTreeMap;
use std::sync::LazyLock;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value as Json};
use utoipa::ToSchema;

/// The built-in templates' sources, in display order.
const SOURCES: &[&str] = &[
    include_str!("../templates/request-and-notify.yaml"),
    include_str!("../templates/form-runs-script.yaml"),
    include_str!("../templates/webhook-approval.yaml"),
    include_str!("../templates/scheduled-health-check.yaml"),
];

#[derive(Debug, thiserror::Error)]
pub enum TemplateError {
    #[error("no such template")]
    NotFound,
    /// A binding's value is missing or wrong: `(binding, what)`.
    #[error("{1}")]
    Binding(String, String),
}

/// What a binding holds, which decides how it's checked and inserted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum BindingKind {
    /// Any text (at most 500 characters).
    Text,
    /// An `http` or `https` URL.
    Url,
    /// `user:<id>`, `team:<id>` or `role:<key>`, comma-separated: a list.
    Recipients,
    /// A library script's key.
    Script,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Binding {
    pub key: String,
    pub label: String,
    pub kind: BindingKind,
    #[serde(default)]
    pub default: Option<String>,
    #[serde(default)]
    pub help: Option<String>,
}

/// A template as the gallery shows it.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TemplateInfo {
    pub key: String,
    pub name: String,
    pub description: String,
    pub category: String,
    #[serde(default)]
    pub bindings: Vec<Binding>,
    /// Whether using it also creates a form.
    #[serde(default)]
    pub has_form: bool,
}

/// A form a template creates.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FormPart {
    pub name: String,
    pub schema: Json,
    #[serde(default)]
    pub ui: Json,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct Source {
    template: TemplateInfo,
    #[serde(default)]
    form: Option<FormPart>,
    workflow: Json,
}

static TEMPLATES: LazyLock<Vec<Source>> = LazyLock::new(|| {
    SOURCES
        .iter()
        .map(|text| {
            let mut source: Source =
                serde_saphyr::from_str(text).expect("built-in templates parse");
            source.template.has_form = source.form.is_some();
            source
        })
        .collect()
});

/// Every built-in template.
pub fn list() -> Vec<TemplateInfo> {
    TEMPLATES.iter().map(|s| s.template.clone()).collect()
}

pub fn get(key: &str) -> Option<TemplateInfo> {
    TEMPLATES
        .iter()
        .find(|s| s.template.key == key)
        .map(|s| s.template.clone())
}

/// A template, ready to create: its form (if any) and workflow document.
#[derive(Debug, Clone)]
pub struct Resolved {
    pub form: Option<FormPart>,
    pub workflow: Json,
}

/// Resolves template `key` with `values` (missing ones take their
/// defaults). The workflow gets `workflow_key` and `workflow_name`;
/// `form_key` is what `%{form}` becomes.
pub fn resolve(
    key: &str,
    values: &BTreeMap<String, String>,
    workflow_key: &str,
    workflow_name: &str,
    form_key: Option<&str>,
) -> Result<Resolved, TemplateError> {
    let source = TEMPLATES
        .iter()
        .find(|s| s.template.key == key)
        .ok_or(TemplateError::NotFound)?;
    let mut bound: BTreeMap<String, (String, Json)> = BTreeMap::new();
    for binding in &source.template.bindings {
        let raw = values
            .get(&binding.key)
            .map(|v| v.trim().to_owned())
            .filter(|v| !v.is_empty())
            .or_else(|| binding.default.clone())
            .ok_or_else(|| {
                TemplateError::Binding(binding.key.clone(), format!("{} is needed", binding.label))
            })?;
        let typed = check(binding, &raw)?;
        bound.insert(binding.key.clone(), (raw, typed));
    }
    if let Some(form_key) = form_key {
        bound.insert(
            "form".into(),
            (form_key.to_owned(), Json::String(form_key.to_owned())),
        );
    }
    let mut workflow = substitute(&source.workflow, &bound);
    workflow["metadata"]["key"] = Json::String(workflow_key.to_owned());
    workflow["metadata"]["name"] = Json::String(workflow_name.to_owned());
    let form = source.form.clone().map(|f| FormPart {
        schema: substitute(&f.schema, &bound),
        ui: substitute(&f.ui, &bound),
        ..f
    });
    Ok(Resolved { form, workflow })
}

fn is_key(value: &str) -> bool {
    (1..=64).contains(&value.len())
        && value.starts_with(|c: char| c.is_ascii_lowercase())
        && value
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
}

/// Checks a value and gives its typed form.
fn check(binding: &Binding, raw: &str) -> Result<Json, TemplateError> {
    let bad = |what: &str| {
        TemplateError::Binding(binding.key.clone(), format!("{}: {what}", binding.label))
    };
    match binding.kind {
        BindingKind::Text => {
            if raw.chars().count() > 500 {
                return Err(bad("at most 500 characters"));
            }
            Ok(Json::String(raw.to_owned()))
        }
        BindingKind::Url => {
            let ok = (raw.starts_with("https://") || raw.starts_with("http://"))
                && raw.len() <= 2000
                && !raw.contains(char::is_whitespace)
                && !raw.contains("{{")
                && !raw.contains("{%");
            if ok {
                Ok(Json::String(raw.to_owned()))
            } else {
                Err(bad("an http(s) URL"))
            }
        }
        BindingKind::Script => {
            if is_key(raw) {
                Ok(Json::String(raw.to_owned()))
            } else {
                Err(bad("a script key"))
            }
        }
        BindingKind::Recipients => {
            let list: Vec<&str> = raw
                .split(',')
                .map(str::trim)
                .filter(|r| !r.is_empty())
                .collect();
            let ok = !list.is_empty()
                && list.iter().all(|r| {
                    r.split_once(':').is_some_and(|(kind, id)| {
                        matches!(kind, "user" | "team" | "role")
                            && !id.is_empty()
                            && id
                                .chars()
                                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
                    })
                });
            if ok {
                Ok(Json::Array(
                    list.into_iter()
                        .map(|r| Json::String(r.to_owned()))
                        .collect(),
                ))
            } else {
                Err(bad("user:<id>, team:<id> or role:<key>, comma-separated"))
            }
        }
    }
}

/// Replaces `%{key}` placeholders: a whole string becomes the typed value,
/// a placeholder inside text becomes the text (lists joined with ", ").
fn substitute(value: &Json, bound: &BTreeMap<String, (String, Json)>) -> Json {
    match value {
        Json::String(s) => {
            if let Some(key) = s.strip_prefix("%{").and_then(|r| r.strip_suffix('}'))
                && let Some((_, typed)) = bound.get(key)
            {
                return typed.clone();
            }
            let mut out = s.clone();
            for (key, (raw, _)) in bound {
                out = out.replace(&format!("%{{{key}}}"), raw);
            }
            Json::String(out)
        }
        Json::Array(items) => Json::Array(items.iter().map(|v| substitute(v, bound)).collect()),
        Json::Object(map) => Json::Object(
            map.iter()
                .map(|(k, v)| (k.clone(), substitute(v, bound)))
                .collect::<Map<_, _>>(),
        ),
        other => other.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn every_template_resolves_with_its_defaults_to_a_valid_workflow() {
        for info in list() {
            let mut values = BTreeMap::new();
            for b in &info.bindings {
                if b.default.is_none() {
                    values.insert(b.key.clone(), "some-script".to_owned());
                }
            }
            let form = info.has_form.then_some("the-form");
            let resolved = resolve(&info.key, &values, "my-flow", "My flow", form).unwrap();
            let text = resolved.workflow.to_string();
            assert!(!text.contains("%{"), "{}: unresolved {text}", info.key);
            assert_eq!(resolved.workflow["metadata"]["key"], "my-flow");
            let parsed = huxley_hwf::parse_json(&text);
            assert!(parsed.is_ok(), "{}: {parsed:?}", info.key);
            // Publishable as it comes: its expressions check out too.
            let problems =
                huxley_expr::check_workflow(&huxley_expr::Engine::new(), &parsed.unwrap());
            assert!(problems.is_empty(), "{}: {problems:?}", info.key);
            assert_eq!(resolved.form.is_some(), info.has_form);
        }
    }

    #[test]
    fn values_are_checked_and_never_change_the_structure() {
        let values: BTreeMap<String, String> =
            [("recipients".to_owned(), "role:admin, team:abc-1".to_owned())].into();
        let r = resolve("request-and-notify", &values, "k", "N", Some("f")).unwrap();
        assert_eq!(r.workflow["spec"]["triggers"][0]["form"], "f");
        assert_eq!(
            r.workflow["spec"]["steps"][0]["with"]["to"],
            json!(["role:admin", "team:abc-1"])
        );
        for (key, bad) in [("recipients", "everyone"), ("recipients", "role:admin} ]")] {
            let values: BTreeMap<String, String> = [(key.to_owned(), bad.to_owned())].into();
            assert!(
                resolve("request-and-notify", &values, "k", "N", Some("f")).is_err(),
                "{bad}"
            );
        }
        for url in ["ftp://x", "https://x/{{ secrets.k }}", "https://a b"] {
            let values: BTreeMap<String, String> = [("url".to_owned(), url.to_owned())].into();
            assert!(
                resolve("scheduled-health-check", &values, "k", "N", None).is_err(),
                "{url}"
            );
        }
        // Inside text, a value is text: here the URL in the alert's title.
        let values: BTreeMap<String, String> =
            [("url".to_owned(), "https://status.example.com".to_owned())].into();
        let r = resolve("scheduled-health-check", &values, "k", "N", None).unwrap();
        let alert = &r.workflow["spec"]["steps"][1]["then"][0]["with"]["title"];
        assert_eq!(alert, "https://status.example.com isn't healthy");
        // A required binding without a default must be given.
        assert!(matches!(
            resolve("form-runs-script", &BTreeMap::new(), "k", "N", Some("f")),
            Err(TemplateError::Binding(key, _)) if key == "script"
        ));
        assert!(matches!(
            resolve("nope", &BTreeMap::new(), "k", "N", None),
            Err(TemplateError::NotFound)
        ));
    }
}
