//! Huxley expressions: Jinja via minijinja (design doc §6.3, ADR-005).
//!
//! - A string that is exactly one `{{ expression }}` evaluates to a **typed**
//!   value (object, list, number, bool…). Any other string containing Jinja
//!   renders to a string. Objects and lists are evaluated recursively.
//! - Sandboxed: no template loading, no filesystem or network, and every
//!   evaluation is bounded by fuel, recursion depth and output size.
//! - Secrets are tracked: an evaluation that reads `secrets.*` (or calls
//!   `secrets.generate`) is marked tainted, so whatever records it can redact
//!   it. [`redact`] also masks known secret values inside larger documents.
//! - [`check_workflow`] finds syntax errors, unknown names, and `steps.<id>`
//!   references that can't have run yet, before a workflow is published.

mod check;
mod functions;
mod secrets;

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use minijinja::{Environment, UndefinedBehavior, Value};
use serde_json::Value as Json;

pub use check::check_workflow;
pub use secrets::{REDACTED, redact};

/// Fuel per evaluation: roughly the number of instructions it may execute.
const FUEL: u64 = 200_000;
/// Largest value an evaluation may produce, serialized.
pub const MAX_OUTPUT_BYTES: usize = 1024 * 1024;
const RECURSION_LIMIT: usize = 64;

#[derive(Debug, thiserror::Error)]
pub enum ExprError {
    /// A syntax or evaluation error, with minijinja's description.
    #[error("{0}")]
    Jinja(String),
    #[error("the result is larger than 1 MiB")]
    TooLarge,
}

impl From<minijinja::Error> for ExprError {
    fn from(error: minijinja::Error) -> Self {
        // `{:#}` would include the source; keep it to the message and line.
        let mut message = error.kind().to_string();
        if let Some(detail) = error.detail() {
            message = format!("{message}: {detail}");
        }
        if let Some(line) = error.line() {
            message = format!("{message} (line {line})");
        }
        Self::Jinja(message)
    }
}

/// The result of evaluating something.
#[derive(Debug, Clone, PartialEq)]
pub struct Evaluated {
    pub value: Json,
    /// Whether a secret was read while evaluating: record this value only
    /// redacted.
    pub tainted: bool,
}

/// What expressions can see: `inputs`, `steps`, `vars`… plus the secrets
/// reachable as `secrets.<name>`.
#[derive(Debug, Clone, Default)]
pub struct Scope {
    values: BTreeMap<String, Value>,
    secrets: Arc<BTreeMap<String, String>>,
    /// Top-level objects with secret keys: (name → its fields, secret keys).
    partly_secret: BTreeMap<String, (BTreeMap<String, Value>, std::collections::BTreeSet<String>)>,
}

impl Scope {
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a top-level name.
    #[must_use]
    pub fn with(mut self, name: &str, value: &Json) -> Self {
        self.insert(name, value);
        self
    }

    pub fn insert(&mut self, name: &str, value: &Json) {
        self.partly_secret.remove(name);
        self.values
            .insert(name.to_owned(), Value::from(minijinja::value::Serde(value)));
    }

    /// Adds a top-level object some of whose keys are secret: reading
    /// them taints the evaluation (e.g. `inputs` with `writeOnly` inputs).
    #[must_use]
    pub fn with_partly_secret(
        mut self,
        name: &str,
        value: &Json,
        secret: &std::collections::BTreeSet<String>,
    ) -> Self {
        let Some(object) = value.as_object().filter(|_| !secret.is_empty()) else {
            return self.with(name, value);
        };
        let fields = object
            .iter()
            .map(|(k, v)| (k.clone(), Value::from(minijinja::value::Serde(v))))
            .collect();
        self.values.remove(name);
        self.partly_secret
            .insert(name.to_owned(), (fields, secret.clone()));
        self
    }

    /// Makes `secrets.<name>` available.
    #[must_use]
    pub fn with_secrets(mut self, secrets: BTreeMap<String, String>) -> Self {
        self.secrets = Arc::new(secrets);
        self
    }

    /// The secret values, for [`redact`].
    pub fn secret_values(&self) -> impl Iterator<Item = &str> {
        self.secrets.values().map(String::as_str)
    }

    fn context(&self, touched: &Arc<AtomicBool>) -> Value {
        let mut map = self.values.clone();
        for (name, (fields, secret)) in &self.partly_secret {
            map.insert(
                name.clone(),
                Value::from_object(secrets::PartlySecret::new(
                    fields.clone(),
                    secret.clone(),
                    touched.clone(),
                )),
            );
        }
        map.insert(
            "secrets".into(),
            Value::from_object(secrets::SecretsObject::new(
                self.secrets.clone(),
                touched.clone(),
            )),
        );
        Value::from_object(map)
    }
}

/// The expression engine. Build one and share it.
#[derive(Debug)]
pub struct Engine {
    env: Environment<'static>,
}

impl Default for Engine {
    fn default() -> Self {
        Self::new()
    }
}

/// How a string is evaluated.
enum Form<'a> {
    Literal,
    /// Exactly one `{{ … }}`: the inner expression.
    Expression(&'a str),
    Template,
}

fn form(text: &str) -> Form<'_> {
    let trimmed = text.trim();
    if let Some(inner) = trimmed
        .strip_prefix("{{")
        .and_then(|t| t.strip_suffix("}}"))
        && !inner.contains("{{")
        && !inner.contains("}}")
        && !inner.contains("{%")
    {
        return Form::Expression(inner.trim_matches('-'));
    }
    if text.contains("{{") || text.contains("{%") || text.contains("{#") {
        Form::Template
    } else {
        Form::Literal
    }
}

impl Engine {
    pub fn new() -> Self {
        let mut env = Environment::new();
        env.set_undefined_behavior(UndefinedBehavior::SemiStrict);
        env.set_fuel(Some(FUEL));
        env.set_recursion_limit(RECURSION_LIMIT);
        // Values are data, not HTML.
        env.set_auto_escape_callback(|_| minijinja::AutoEscape::None);
        env.set_unknown_method_callback(minijinja_contrib::pycompat::unknown_method_callback);
        minijinja_contrib::add_to_environment(&mut env);
        functions::register(&mut env);
        Self { env }
    }

    /// Evaluates a JSON value whose strings may hold Jinja.
    pub fn eval(&self, value: &Json, scope: &Scope) -> Result<Evaluated, ExprError> {
        let touched = Arc::new(AtomicBool::new(false));
        let context = scope.context(&touched);
        let value = self.eval_value(value, &context)?;
        if serde_json::to_vec(&value).map_or(true, |v| v.len() > MAX_OUTPUT_BYTES) {
            return Err(ExprError::TooLarge);
        }
        Ok(Evaluated {
            value,
            tainted: touched.load(Ordering::Relaxed),
        })
    }

    /// Evaluates a string (see [`Engine::eval`]).
    pub fn eval_str(&self, text: &str, scope: &Scope) -> Result<Evaluated, ExprError> {
        self.eval(&Json::String(text.to_owned()), scope)
    }

    /// Evaluates a condition with Jinja truthiness (an undefined value is
    /// false).
    pub fn eval_condition(&self, text: &str, scope: &Scope) -> Result<bool, ExprError> {
        let touched = Arc::new(AtomicBool::new(false));
        let context = scope.context(&touched);
        match form(text) {
            Form::Expression(expr) => {
                let compiled = self.env.compile_expression(expr)?;
                Ok(compiled.eval(context)?.is_true())
            }
            _ => Ok(truthy(
                &self.eval_value(&Json::String(text.to_owned()), &context)?,
            )),
        }
    }

    fn eval_value(&self, value: &Json, context: &Value) -> Result<Json, ExprError> {
        Ok(match value {
            Json::String(text) => match form(text) {
                Form::Literal => value.clone(),
                Form::Expression(expr) => {
                    let result = self.env.compile_expression(expr)?.eval(context.clone())?;
                    to_json(&result)?
                }
                Form::Template => Json::String(self.env.render_str(text, context.clone())?),
            },
            Json::Array(items) => Json::Array(
                items
                    .iter()
                    .map(|v| self.eval_value(v, context))
                    .collect::<Result<_, _>>()?,
            ),
            Json::Object(map) => Json::Object(
                map.iter()
                    .map(|(k, v)| Ok((k.clone(), self.eval_value(v, context)?)))
                    .collect::<Result<_, ExprError>>()?,
            ),
            other => other.clone(),
        })
    }

    /// Whether `name` is a built-in function or global (`range`, `now`,
    /// `uuid`, `dict`…) rather than something the scope must provide.
    pub fn is_global(&self, name: &str) -> bool {
        self.env.globals().any(|(global, _)| global == name)
    }

    /// Compiles a string and returns the names it reads, as dotted paths
    /// (`steps.build_upn.output`). Literals return nothing.
    pub fn references(&self, text: &str) -> Result<Vec<String>, ExprError> {
        let mut names: Vec<String> = match form(text) {
            Form::Literal => return Ok(Vec::new()),
            Form::Expression(expr) => self
                .env
                .compile_expression(expr)?
                .undeclared_variables(true)
                .into_iter()
                .collect(),
            Form::Template => self
                .env
                .template_from_str(text)?
                .undeclared_variables(true)
                .into_iter()
                .collect(),
        };
        names.sort();
        Ok(names)
    }
}

fn truthy(value: &Json) -> bool {
    match value {
        Json::Null => false,
        Json::Bool(b) => *b,
        Json::Number(n) => n.as_f64().is_some_and(|f| f != 0.0),
        Json::String(s) => !s.is_empty(),
        Json::Array(a) => !a.is_empty(),
        Json::Object(o) => !o.is_empty(),
    }
}

fn to_json(value: &Value) -> Result<Json, ExprError> {
    if value.is_undefined() {
        return Ok(Json::Null);
    }
    serde_json::to_value(value).map_err(|e| ExprError::Jinja(format!("not a JSON value: {e}")))
}
