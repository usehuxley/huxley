//! Checks that serde can't express: identifiers, uniqueness, durations,
//! ranges, unreachable steps. Expressions are checked by the expression
//! engine, which adds its own diagnostics.

use std::collections::HashSet;

use serde::Serialize;
use serde_json::Value;

use crate::duration;
use crate::model::{Join, RetryPolicy, Step, Trigger, Workflow};

/// The `apiVersion` this crate reads.
pub const API_VERSION: &str = "usehuxley.com/v1";

const MAX_FOREACH_CONCURRENCY: u32 = 50;
const MAX_ATTEMPTS: u32 = 20;
const MAX_STEPS: usize = 500;
const MAX_SYNC_WEBHOOK_SECS: u64 = 300;

/// A problem with a workflow, and where it is (`spec.steps[2].then[0].id`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Diagnostic {
    pub path: String,
    pub message: String,
}

impl std::fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.path, self.message)
    }
}

#[derive(Default)]
struct Checker {
    diagnostics: Vec<Diagnostic>,
    step_ids: HashSet<String>,
    steps: usize,
}

impl Checker {
    fn error(&mut self, path: impl Into<String>, message: impl Into<String>) {
        self.diagnostics.push(Diagnostic {
            path: path.into(),
            message: message.into(),
        });
    }

    fn duration(&mut self, path: &str, value: Option<&str>) {
        if let Some(text) = value
            && duration::parse(text).is_none()
        {
            self.error(
                path,
                format!("{text:?} is not a duration (e.g. 30s, 5m, 1h30m; at most a year)"),
            );
        }
    }
}

/// Whether `key` is a valid workflow key: lowercase letters, digits and
/// single hyphens, starting with a letter, at most 64 characters.
pub fn is_valid_key(key: &str) -> bool {
    (1..=64).contains(&key.len())
        && key.starts_with(|c: char| c.is_ascii_lowercase())
        && !key.ends_with('-')
        && !key.contains("--")
        && key
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// Whether `id` is a valid step id: lowercase letters, digits and
/// underscores, starting with a letter, at most 64 characters. Usable
/// unquoted in expressions (`steps.<id>.output`).
pub fn is_valid_step_id(id: &str) -> bool {
    (1..=64).contains(&id.len())
        && id.starts_with(|c: char| c.is_ascii_lowercase())
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

/// Every structural problem with `workflow`; empty when it's valid.
pub fn validate(workflow: &Workflow) -> Vec<Diagnostic> {
    let mut c = Checker::default();
    if workflow.api_version != API_VERSION {
        c.error(
            "apiVersion",
            format!("must be {API_VERSION:?}, not {:?}", workflow.api_version),
        );
    }
    let meta = &workflow.metadata;
    if !is_valid_key(&meta.key) {
        c.error(
            "metadata.key",
            "use lowercase letters, digits and hyphens, starting with a letter (at most 64)",
        );
    }
    if meta.name.trim().is_empty() {
        c.error("metadata.name", "is required");
    }

    let spec = &workflow.spec;
    if let Some(inputs) = &spec.inputs
        && inputs.get("type").and_then(Value::as_str) != Some("object")
    {
        c.error(
            "spec.inputs",
            "must be a JSON Schema with \"type\": \"object\"",
        );
    }
    triggers(&mut c, &spec.triggers);
    if spec.steps.is_empty() {
        c.error("spec.steps", "a workflow needs at least one step");
    }
    block(&mut c, "spec.steps", &spec.steps);
    block(&mut c, "spec.on_error", &spec.on_error);
    if c.steps > MAX_STEPS {
        c.error(
            "spec.steps",
            format!("at most {MAX_STEPS} steps in a workflow ({} here)", c.steps),
        );
    }
    if let Some(settings) = &spec.settings {
        c.duration("spec.settings.timeout", settings.timeout.as_deref());
        if settings.concurrency == Some(0) {
            c.error("spec.settings.concurrency", "must be at least 1");
        }
    }
    c.diagnostics
}

fn triggers(c: &mut Checker, triggers: &[Trigger]) {
    let mut ids = HashSet::new();
    for (i, trigger) in triggers.iter().enumerate() {
        let path = format!("spec.triggers[{i}]");
        // Default id: the type, so one trigger of each type needs no id.
        let id = trigger.id().unwrap_or(trigger.type_name());
        if !is_valid_step_id(id) {
            c.error(
                format!("{path}.id"),
                "use lowercase letters, digits and underscores",
            );
        } else if !ids.insert(id.to_owned()) {
            c.error(
                format!("{path}.id"),
                format!("another trigger is already {id:?}: give each its own id"),
            );
        }
        match trigger {
            Trigger::Schedule(s) => {
                match (&s.cron, &s.interval) {
                    (Some(cron), None) => {
                        if let Err(e) = crate::cron::Cron::parse(cron) {
                            c.error(format!("{path}.cron"), e);
                        }
                    }
                    (None, Some(_)) => {
                        c.duration(&format!("{path}.interval"), s.interval.as_deref());
                    }
                    _ => c.error(&path, "set exactly one of cron or interval"),
                }
                if s.interval.is_some() && s.timezone.is_some() {
                    c.error(format!("{path}.timezone"), "only applies to cron schedules");
                } else if let Some(tz) = &s.timezone
                    && crate::cron::time_zone(tz).is_none()
                {
                    c.error(
                        format!("{path}.timezone"),
                        format!(
                            "{tz:?} isn't a time zone (use an IANA name such as Europe/London)"
                        ),
                    );
                }
            }
            Trigger::Webhook(w) => {
                let timeout = format!("{path}.timeout");
                c.duration(&timeout, w.timeout.as_deref());
                if w.timeout
                    .as_deref()
                    .and_then(duration::parse)
                    .is_some_and(|d| d.as_secs() > MAX_SYNC_WEBHOOK_SECS)
                {
                    c.error(timeout, "at most 5m");
                }
            }
            Trigger::Poll(p) => poll_trigger(c, &path, p),
            Trigger::Event(e) if e.event.trim().is_empty() => {
                c.error(format!("{path}.event"), "is required");
            }
            Trigger::Form(f) if f.form.trim().is_empty() => {
                c.error(format!("{path}.form"), "is required");
            }
            _ => {}
        }
    }
}

/// The shortest poll interval.
const MIN_POLL_SECS: u64 = 60;

fn poll_trigger(c: &mut Checker, path: &str, p: &crate::PollTrigger) {
    let fixed = p
        .action
        .split_once('/')
        .is_some_and(|(integration, action)| {
            is_valid_key(integration)
                && !action.is_empty()
                && action
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_')
        });
    if !fixed {
        c.error(format!("{path}.action"), "<integration key>/<action key>");
    }
    if p.connection.as_deref().is_some_and(|k| !is_valid_key(k)) {
        c.error(format!("{path}.connection"), "a connection key");
    }
    match duration::parse(&p.interval) {
        None => c.error(
            format!("{path}.interval"),
            format!("{:?} is not a duration", p.interval),
        ),
        Some(d) if d.as_secs() < MIN_POLL_SECS => {
            c.error(format!("{path}.interval"), "at least 1m");
        }
        Some(_) => {}
    }
}

fn block(c: &mut Checker, path: &str, steps: &[Step]) {
    for (i, step) in steps.iter().enumerate() {
        let here = format!("{path}[{i}]");
        if i > 0 && steps[i - 1].ends_block() {
            c.error(
                &here,
                format!(
                    "unreachable: the previous step ({}) ends the run",
                    steps[i - 1].type_name()
                ),
            );
        }
        one(c, &here, step);
    }
}

/// `action` and `connection` are fixed keys, so publishing can check them.
fn integration_action(c: &mut Checker, path: &str, s: &crate::IntegrationActionStep) {
    let fixed = s
        .action
        .split_once('/')
        .is_some_and(|(integration, action)| {
            is_valid_key(integration)
                && !action.is_empty()
                && action
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_')
        });
    if !fixed {
        c.error(
            format!("{path}.action"),
            "<integration key>/<action key>, e.g. tickets-api/createTicket (not an expression)",
        );
    }
    if s.max_pages.is_some_and(|n| !(1..=100).contains(&n)) {
        c.error(format!("{path}.max_pages"), "from 1 to 100");
    }
    if s.max_pages.is_some() && !s.all_pages {
        c.error(format!("{path}.max_pages"), "only with all_pages: true");
    }
    if s.connection.as_deref().is_some_and(|k| !is_valid_key(k)) {
        c.error(
            format!("{path}.connection"),
            "a connection key (not an expression)",
        );
    }
}

/// An environment variable name a script may be given.
fn is_env_name(name: &str) -> bool {
    name.len() <= 64
        && name.starts_with(|c: char| c.is_ascii_uppercase() || c == '_')
        && name
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
        && !name.starts_with("HUXLEY_")
        && !matches!(name, "PATH" | "HOME" | "USER" | "HOSTNAME" | "TERM")
}

fn script(c: &mut Checker, path: &str, s: &crate::ScriptStep) {
    if !is_valid_key(&s.script) {
        c.error(format!("{path}.script"), "a script key (not an expression)");
    }
    for name in s.env.keys() {
        if !is_env_name(name) {
            c.error(
                format!("{path}.env.{name}"),
                "upper-case letters, digits and underscores (not HUXLEY_*, PATH or HOME)",
            );
        }
    }
    for (name, target) in &s.connections {
        if !is_env_name(name) {
            c.error(
                format!("{path}.connections.{name}"),
                "upper-case letters, digits and underscores (not HUXLEY_*, PATH or HOME)",
            );
        }
        let (integration, connection) = target.split_once('/').unwrap_or((target, ""));
        if !is_valid_key(integration) || !(connection.is_empty() || is_valid_key(connection)) {
            c.error(
                format!("{path}.connections.{name}"),
                "<integration> or <integration>/<connection> (keys, not expressions)",
            );
        }
        for suffix in ["_URL", "_TOKEN", "_KEY", "_USERNAME", "_PASSWORD"] {
            if s.env.contains_key(&format!("{name}{suffix}")) {
                c.error(
                    format!("{path}.env.{name}{suffix}"),
                    format!("set by connection {name}"),
                );
            }
        }
    }
}

fn wait_for_callback(c: &mut Checker, path: &str, s: &crate::WaitForCallbackStep) {
    c.duration(&format!("{path}.expires"), s.expires.as_deref());
    let Some(send) = &s.send else { return };
    match (&send.action, &send.request) {
        (Some(action), None) => {
            let fixed = action.split_once('/').is_some_and(|(i, a)| {
                is_valid_key(i)
                    && !a.is_empty()
                    && a.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
            });
            if !fixed {
                c.error(
                    format!("{path}.send.action"),
                    "<integration key>/<action key>",
                );
            }
        }
        (None, Some(_)) => {}
        _ => c.error(
            format!("{path}.send"),
            "set exactly one of action or request",
        ),
    }
}

fn one(c: &mut Checker, path: &str, step: &Step) {
    c.steps += 1;
    let common = step.common();
    if !is_valid_step_id(common.id) {
        c.error(
            format!("{path}.id"),
            "use lowercase letters, digits and underscores, starting with a letter (at most 64)",
        );
    } else if !c.step_ids.insert(common.id.to_owned()) {
        c.error(
            format!("{path}.id"),
            format!(
                "another step is already {:?}: step ids are unique in a workflow",
                common.id
            ),
        );
    }
    c.duration(&format!("{path}.timeout"), common.timeout);
    if let Some(retry) = common.retry {
        retry_policy(c, &format!("{path}.retry"), retry);
    }

    match step {
        Step::Foreach(s) => {
            if s.concurrency
                .is_some_and(|n| n == 0 || n > MAX_FOREACH_CONCURRENCY)
            {
                c.error(
                    format!("{path}.concurrency"),
                    format!("between 1 and {MAX_FOREACH_CONCURRENCY}"),
                );
            }
            if let Some(name) = &s.item_name
                && (!is_valid_step_id(name) || RESERVED.contains(&name.as_str()))
            {
                c.error(
                    format!("{path}.as"),
                    "a lowercase name that isn't already in scope (steps, vars, loop…)",
                );
            }
        }
        Step::Parallel(s) => {
            if s.branches.len() < 2 {
                c.error(
                    format!("{path}.branches"),
                    "a parallel step needs at least two branches",
                );
            }
            let mut ids = HashSet::new();
            for (i, branch) in s.branches.iter().enumerate() {
                if !is_valid_step_id(&branch.id) || !ids.insert(branch.id.as_str()) {
                    c.error(
                        format!("{path}.branches[{i}].id"),
                        "branch ids are unique lowercase names",
                    );
                }
            }
            if s.join == Join::Any
                && s.branches
                    .iter()
                    .any(|b| b.steps.iter().any(Step::ends_block))
            {
                c.error(
                    format!("{path}.join"),
                    "with join: any, branches can't end the run (return/fail)",
                );
            }
        }
        Step::Switch(s) if s.cases.is_empty() => {
            c.error(format!("{path}.cases"), "a switch needs at least one case");
        }
        Step::Try(s) if s.catch.is_empty() && s.finally.is_empty() => {
            c.error(path, "a try step needs catch or finally");
        }
        Step::Wait(s) => match (&s.duration, &s.until) {
            (Some(_), None) => c.duration(&format!("{path}.duration"), s.duration.as_deref()),
            (None, Some(_)) => {}
            _ => c.error(path, "set exactly one of duration or until"),
        },
        Step::IntegrationAction(s) => integration_action(c, path, s),
        Step::WaitForCallback(s) => wait_for_callback(c, path, s),
        Step::Script(s) => script(c, path, s),
        Step::Approval(s) => {
            c.duration(&format!("{path}.expires"), s.expires.as_deref());
            if s.title.trim().is_empty() {
                c.error(format!("{path}.title"), "is required");
            }
        }
        Step::Subworkflow(s) if !is_valid_key(&s.workflow) => {
            c.error(format!("{path}.workflow"), "a workflow key");
        }
        Step::Fail(s) if s.message.trim().is_empty() => {
            c.error(format!("{path}.message"), "is required");
        }
        _ => {}
    }

    for (label, steps) in step.blocks() {
        block(c, &format!("{path}.{label}"), steps);
    }
}

/// Names that are already in expression scope.
const RESERVED: &[&str] = &[
    "inputs",
    "trigger",
    "steps",
    "vars",
    "org",
    "actor",
    "run",
    "env",
    "loop",
    "error",
    "secrets",
    "connections",
];

fn retry_policy(c: &mut Checker, path: &str, retry: &RetryPolicy) {
    if !(2..=MAX_ATTEMPTS).contains(&retry.max_attempts) {
        c.error(
            format!("{path}.max_attempts"),
            format!("between 2 and {MAX_ATTEMPTS} (attempts in total, including the first)"),
        );
    }
    c.duration(&format!("{path}.delay"), retry.delay.as_deref());
    c.duration(&format!("{path}.max_delay"), retry.max_delay.as_deref());
    for (i, on) in retry.on.iter().enumerate() {
        let ok = matches!(on.as_str(), "timeout" | "network" | "4xx" | "5xx")
            || (on.len() == 3 && on.parse::<u16>().is_ok_and(|s| (400..600).contains(&s)));
        if !ok {
            c.error(
                format!("{path}.on[{i}]"),
                format!("{on:?}: use an HTTP status (429), 4xx, 5xx, timeout or network"),
            );
        }
    }
}
