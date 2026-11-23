//! Static checks of a workflow's expressions: syntax, names in scope, and
//! `steps.<id>` references that point at steps which have run by then.
//!
//! With structured control flow (ADR-020), "has run by then" is decidable:
//! a step can read another that comes earlier in document order, isn't one
//! of its ancestors (those are still running), isn't in another branch of an
//! `if`, `switch` or `parallel` it shares, and isn't inside a loop it is
//! outside of.

use std::collections::HashMap;

use huxley_hwf::{Diagnostic, Step, Trigger, WebhookAuth, Workflow};
use serde_json::Value as Json;

use crate::Engine;

/// Names every expression can use.
const BASE_SCOPE: &[&str] = &[
    "inputs",
    "trigger",
    "steps",
    "vars",
    "org",
    "actor",
    "run",
    "env",
    "secrets",
    "connections",
];

/// One level of nesting: the control-flow step and which of its blocks.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Frame {
    owner: String,
    kind: &'static str,
    block: String,
}

#[derive(Debug, Clone)]
struct Place {
    index: usize,
    frames: Vec<Frame>,
}

/// Where an expression is evaluated.
struct Site<'a> {
    /// The step it belongs to (`None` for workflow outputs and triggers).
    step: Option<&'a str>,
    place: Place,
    names: Vec<String>,
}

struct Checker<'a> {
    engine: &'a Engine,
    places: HashMap<String, Place>,
    diagnostics: Vec<Diagnostic>,
}

/// Every expression problem in `workflow`. Run after [`huxley_hwf::validate`]:
/// it assumes step ids are unique.
pub fn check_workflow(engine: &Engine, workflow: &Workflow) -> Vec<Diagnostic> {
    let mut places = HashMap::new();
    let mut index = 0;
    index_block(
        &workflow.spec.steps,
        &mut Vec::new(),
        &mut index,
        &mut places,
    );
    index_block(
        &workflow.spec.on_error,
        &mut vec![Frame {
            owner: String::new(),
            kind: "on_error",
            block: "on_error".into(),
        }],
        &mut index,
        &mut places,
    );
    let mut checker = Checker {
        engine,
        places,
        diagnostics: Vec::new(),
    };

    for (i, trigger) in workflow.spec.triggers.iter().enumerate() {
        checker.trigger(&format!("spec.triggers[{i}]"), trigger);
    }
    let base: Vec<String> = BASE_SCOPE.iter().map(|s| (*s).to_owned()).collect();
    checker.block("spec.steps", &workflow.spec.steps, &base);
    let mut with_error = base.clone();
    with_error.push("error".into());
    checker.block("spec.on_error", &workflow.spec.on_error, &with_error);
    if let Some(outputs) = &workflow.spec.outputs {
        // Outputs are evaluated after the last top-level step.
        let site = Site {
            step: None,
            place: Place {
                index: usize::MAX,
                frames: Vec::new(),
            },
            names: base,
        };
        for (key, value) in outputs {
            checker.value(&site, &format!("spec.outputs.{key}"), value);
        }
    }
    checker.diagnostics
}

fn index_block(
    steps: &[Step],
    frames: &mut Vec<Frame>,
    index: &mut usize,
    places: &mut HashMap<String, Place>,
) {
    for step in steps {
        places.insert(
            step.id().to_owned(),
            Place {
                index: *index,
                frames: frames.clone(),
            },
        );
        *index += 1;
        for (label, block) in step.blocks() {
            frames.push(Frame {
                owner: step.id().to_owned(),
                kind: step.type_name(),
                block: block_name(&label),
            });
            index_block(block, frames, index, places);
            frames.pop();
        }
    }
}

/// `cases[2].steps` → `cases[2]`; `branches[1].steps` → `branches[1]`.
fn block_name(label: &str) -> String {
    label.trim_end_matches(".steps").to_owned()
}

/// Keys of a step's JSON that hold nested steps or aren't expressions.
const NOT_EXPRESSIONS: &[&str] = &[
    "id",
    "type",
    "name",
    "description",
    "then",
    "else",
    "steps",
    "default",
    "catch",
    "finally",
    "branches",
    "cases",
    "retry",
    "timeout",
    "continue_on_error",
    "concurrency",
    "as",
    "join",
    "workflow",
    "wait",
];

impl Checker<'_> {
    fn error(&mut self, path: &str, message: String) {
        self.diagnostics.push(Diagnostic {
            path: path.to_owned(),
            message,
        });
    }

    fn trigger(&mut self, path: &str, trigger: &Trigger) {
        let names = |extra: &[&str]| -> Vec<String> {
            ["vars", "secrets", "org", "env"]
                .iter()
                .chain(extra)
                .map(|s| (*s).to_owned())
                .collect()
        };
        let site = |names: Vec<String>| Site {
            step: None,
            place: Place {
                index: 0,
                frames: Vec::new(),
            },
            names,
        };
        match trigger {
            Trigger::Webhook(w) => {
                let site = site(names(&[]));
                match &w.auth {
                    Some(WebhookAuth::Hmac { secret, .. }) => {
                        self.text(&site, &format!("{path}.auth.secret"), secret);
                    }
                    Some(WebhookAuth::Bearer { token }) => {
                        self.text(&site, &format!("{path}.auth.token"), token);
                    }
                    Some(WebhookAuth::Basic { username, password }) => {
                        self.text(&site, &format!("{path}.auth.username"), username);
                        self.text(&site, &format!("{path}.auth.password"), password);
                    }
                    None => {}
                }
            }
            Trigger::Event(e) => {
                if let Some(filter) = &e.filter {
                    let site = site(names(&["event"]));
                    self.text(&site, &format!("{path}.filter"), filter);
                }
            }
            _ => {}
        }
    }

    fn block(&mut self, path: &str, steps: &[Step], names: &[String]) {
        for (i, step) in steps.iter().enumerate() {
            self.step(&format!("{path}[{i}]"), step, names);
        }
    }

    fn step(&mut self, path: &str, step: &Step, names: &[String]) {
        let place = self.places.get(step.id()).cloned().unwrap_or(Place {
            index: 0,
            frames: Vec::new(),
        });
        let site = Site {
            step: Some(step.id()),
            place,
            names: names.to_vec(),
        };
        let json = serde_json::to_value(step).unwrap_or(Json::Null);
        if let Json::Object(map) = &json {
            for (key, value) in map {
                if !NOT_EXPRESSIONS.contains(&key.as_str()) {
                    self.value(&site, &format!("{path}.{key}"), value);
                }
            }
        }
        if let Step::Switch(s) = step {
            for (i, case) in s.cases.iter().enumerate() {
                self.value(&site, &format!("{path}.cases[{i}].when"), &case.when);
            }
        }

        // Nested blocks, with the names each adds.
        for (label, block) in step.blocks() {
            let mut inner = names.to_vec();
            match step {
                Step::Foreach(f) => {
                    inner.push("loop".into());
                    inner.push(f.item_name.clone().unwrap_or_else(|| "item".into()));
                }
                Step::Try(_) if label == "catch" => inner.push("error".into()),
                _ => {}
            }
            self.block(&format!("{path}.{label}"), block, &inner);
        }
    }

    fn value(&mut self, site: &Site<'_>, path: &str, value: &Json) {
        match value {
            Json::String(text) => self.text(site, path, text),
            Json::Array(items) => {
                for (i, item) in items.iter().enumerate() {
                    self.value(site, &format!("{path}[{i}]"), item);
                }
            }
            Json::Object(map) => {
                for (key, item) in map {
                    self.value(site, &format!("{path}.{key}"), item);
                }
            }
            _ => {}
        }
    }

    fn text(&mut self, site: &Site<'_>, path: &str, text: &str) {
        let references = match self.engine.references(text) {
            Ok(references) => references,
            Err(e) => {
                self.error(path, e.to_string());
                return;
            }
        };
        for reference in references {
            let mut parts = reference.split('.');
            let root = parts.next().unwrap_or_default();
            if !site.names.iter().any(|n| n == root) && !self.engine.is_global(root) {
                self.error(
                    path,
                    format!(
                        "{root:?} isn't available here (available: {})",
                        site.names.join(", ")
                    ),
                );
                continue;
            }
            if root == "steps" {
                // `steps` on its own (e.g. `steps | list`) names no step.
                if let Some(problem) = parts.next().and_then(|id| self.visibility(site, id)) {
                    self.error(path, problem);
                }
            }
        }
    }

    /// Why `site` can't read `steps.<id>`, if it can't.
    fn visibility(&self, site: &Site<'_>, id: &str) -> Option<String> {
        let Some(target) = self.places.get(id) else {
            return Some(format!("there is no step {id:?}"));
        };
        if site.step == Some(id) {
            return Some(format!("step {id:?} can't read its own output"));
        }
        if site.place.frames.iter().any(|f| f.owner == id) {
            return Some(format!(
                "step {id:?} contains this one, so it hasn't finished yet"
            ));
        }
        if target.index > site.place.index {
            return Some(format!("step {id:?} runs after this"));
        }
        // The first level where they diverge.
        for (theirs, ours) in target.frames.iter().zip(&site.place.frames) {
            if theirs.owner != ours.owner {
                break;
            }
            if theirs.block != ours.block {
                return match theirs.kind {
                    "if" | "switch" => Some(format!(
                        "step {id:?} is in another branch of {:?}, so it won't have run",
                        theirs.owner
                    )),
                    "parallel" => Some(format!(
                        "step {id:?} is in another branch of {:?}, which runs at the same time",
                        theirs.owner
                    )),
                    _ => None,
                };
            }
        }
        // Inside a loop this site is outside of.
        if let Some(frame) = target
            .frames
            .iter()
            .find(|f| f.kind == "foreach" && !site.place.frames.contains(f))
        {
            return Some(format!(
                "step {id:?} is inside the loop {:?}; read steps.{}.output instead",
                frame.owner, frame.owner
            ));
        }
        None
    }
}
