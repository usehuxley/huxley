//! Prompt injection (docs/security.md): text an AI step reads, such as an
//! email or a ticket, can steer what it writes. So a step that *acts* on
//! free-form AI output (sends it, calls a service with it, changes a record
//! clients or other systems see) must run only once a person approves:
//! the step, or a step it's inside, has an `if` (or a condition) that reads
//! an approval step's decision, made after the AI step. `trust_ai: true` on
//! the step says to act without review.
//!
//! What counts as AI output: `ai.prompt` and `ai.extract` outputs, and
//! whatever is computed from them (transforms, variables, loops over them),
//! followed through the workflow. `ai.classify` labels come from a fixed
//! list, so they're free to route on. Steps that only keep AI output as a
//! suggestion for staff (triage suggestions, summaries, drafts, internal
//! notes and notifications) don't act.

use std::collections::{BTreeSet, HashSet};

use huxley_hwf::{Diagnostic, Step, Workflow};
use serde_json::Value as Json;

use crate::Engine;

/// Steps that don't act on what they're given: control flow, data, reads,
/// and keeping AI output as a suggestion for a person.
const SAFE: &[&str] = &[
    "transform",
    "set_var",
    "if",
    "switch",
    "foreach",
    "parallel",
    "try",
    "wait",
    "return",
    "fail",
    "approval",
    "ai.prompt",
    "ai.classify",
    "ai.extract",
    "core.notify",
    "dataset.query",
    "core.contact.find",
    "core.company.get",
    "core.ticket.get",
    "core.ticket.find",
    "core.ticket.embed",
    "core.kb.embed",
    "core.mail.fetch",
    "core.timesheet.check",
    "core.health.compute",
    "core.sentiment.record",
    "core.triage.options",
    "core.triage.apply",
    "core.dispatch.recommend",
    "core.workforce.insights",
    "core.ticket.insight",
    "core.kb.draft",
    "core.time.suggest",
    "core.expense.read_receipt",
    "core.scope.context",
    "core.scope.write",
    "core.scope.estimate_context",
    "core.scope.write_estimate",
    "core.invoice.unpaid",
    "core.invoice.status",
    "core.project.status",
];

/// Whether a step acts on what it's given (see the module docs).
fn acts(step: &Step, json: &Json) -> bool {
    match step.type_name() {
        // Internal notes are for staff; public and resolution notes aren't.
        "core.ticket.note" => {
            !matches!(json["with"].get("kind"), None | Some(Json::Null))
                && json["with"]["kind"] != "internal"
        }
        // Reports are staff-only unless shown in the portal.
        "core.project.report" => !matches!(
            json["with"].get("client_visible"),
            None | Some(Json::Null | Json::Bool(false))
        ),
        other => !SAFE.contains(&other),
    }
}

struct Node<'a> {
    step: &'a Step,
    path: String,
    index: usize,
    ancestors: Vec<usize>,
    /// Steps and variables its input reads (not its `if`).
    reads_steps: HashSet<String>,
    reads_vars: HashSet<String>,
    /// Steps its `if` (and an `if` or `switch` step's condition) reads.
    gate_steps: HashSet<String>,
    /// A `foreach`'s `items`.
    items_steps: HashSet<String>,
    items_vars: HashSet<String>,
    acts: bool,
}

/// Every step that acts on AI output without an approval.
pub fn check(engine: &Engine, workflow: &Workflow) -> Vec<Diagnostic> {
    let mut nodes = Vec::new();
    collect(engine, &workflow.spec.steps, "spec.steps", &[], &mut nodes);
    collect(
        engine,
        &workflow.spec.on_error,
        "spec.on_error",
        &[],
        &mut nodes,
    );
    if !nodes
        .iter()
        .any(|n| matches!(n.step.type_name(), "ai.prompt" | "ai.extract"))
    {
        return Vec::new();
    }
    let ids: Vec<&str> = nodes.iter().map(|n| n.step.id()).collect();
    let approvals: HashSet<&str> = nodes
        .iter()
        .filter(|n| n.step.type_name() == "approval")
        .map(|n| n.step.id())
        .collect();

    let input_sources = propagate(&nodes, &ids);

    let mut diagnostics = Vec::new();
    for (i, node) in nodes.iter().enumerate() {
        let from = &input_sources[i];
        if !node.acts || from.is_empty() || node.step.common().trust_ai {
            continue;
        }
        let latest_source = from.iter().max().copied().unwrap_or(0);
        let gated = std::iter::once(i)
            .chain(node.ancestors.iter().copied())
            .any(|g| {
                nodes[g].gate_steps.iter().any(|read| {
                    approvals.contains(read.as_str())
                        && ids
                            .iter()
                            .position(|id| id == read)
                            .is_some_and(|j| nodes[j].index > latest_source)
                })
            });
        if gated {
            continue;
        }
        let ai: Vec<&str> = from.iter().map(|&s| ids[s]).collect();
        diagnostics.push(Diagnostic {
            path: node.path.clone(),
            message: format!(
                "acts on AI output ({}) without a person's approval, and text the AI read can steer that output. \
                 Add an approval step after it and run this step only if it's approved \
                 (if: \"{{{{ steps.<approval>.output.decision == 'approved' }}}}\"), \
                 or set trust_ai: true to act without review",
                ai.iter().map(|id| format!("steps.{id}")).collect::<Vec<_>>().join(", ")
            ),
        });
    }
    diagnostics
}

/// Which AI steps (by index) each step's input carries, followed to a
/// fixpoint through inputs, variables, loops and containers.
fn propagate(nodes: &[Node<'_>], ids: &[&str]) -> Vec<BTreeSet<usize>> {
    let mut sources: Vec<BTreeSet<usize>> = nodes
        .iter()
        .map(|n| {
            if matches!(n.step.type_name(), "ai.prompt" | "ai.extract") {
                BTreeSet::from([n.index])
            } else {
                BTreeSet::new()
            }
        })
        .collect();
    let mut input_sources: Vec<BTreeSet<usize>> = vec![BTreeSet::new(); nodes.len()];
    loop {
        let mut changed = false;
        let tainted_vars: Vec<(String, BTreeSet<usize>)> = nodes
            .iter()
            .zip(&sources)
            .filter(|(n, s)| n.step.type_name() == "set_var" && !s.is_empty())
            .flat_map(|(n, s)| var_names(n.step).into_iter().map(move |v| (v, s.clone())))
            .collect();
        for (i, node) in nodes.iter().enumerate() {
            let mut from = BTreeSet::new();
            for read in &node.reads_steps {
                if let Some(j) = ids.iter().position(|id| id == read) {
                    from.extend(sources[j].iter().copied());
                }
            }
            for (var, s) in &tainted_vars {
                if node.reads_vars.contains(var) {
                    from.extend(s.iter().copied());
                }
            }
            // Looping over AI output: everything inside reads it.
            for &a in &node.ancestors {
                let looped = &nodes[a];
                for read in &looped.items_steps {
                    if let Some(j) = ids.iter().position(|id| id == read) {
                        from.extend(sources[j].iter().copied());
                    }
                }
                for (var, s) in &tainted_vars {
                    if looped.items_vars.contains(var) {
                        from.extend(s.iter().copied());
                    }
                }
            }
            if !from.is_subset(&input_sources[i]) {
                input_sources[i].extend(from.iter().copied());
                changed = true;
            }
            if !from.is_subset(&sources[i]) {
                sources[i].extend(from);
                changed = true;
            }
        }
        // A container's output carries what's inside it.
        for i in 0..nodes.len() {
            for &a in &nodes[i].ancestors.clone() {
                let inner = sources[i].clone();
                if !inner.is_subset(&sources[a]) {
                    sources[a].extend(inner);
                    changed = true;
                }
            }
        }
        if !changed {
            break;
        }
    }
    input_sources
}

fn collect<'a>(
    engine: &Engine,
    steps: &'a [Step],
    path: &str,
    ancestors: &[usize],
    nodes: &mut Vec<Node<'a>>,
) {
    for (i, step) in steps.iter().enumerate() {
        let here = format!("{path}[{i}]");
        let index = nodes.len();
        let json = serde_json::to_value(step).unwrap_or(Json::Null);
        let mut input = json.clone();
        let mut gate = Vec::new();
        if let Some(object) = input.as_object_mut() {
            for key in ["if", "id", "name", "description", "type"] {
                if let Some(v) = object.remove(key)
                    && key == "if"
                {
                    gate.push(v);
                }
            }
            // An if's condition and a switch's value decide what runs.
            match step.type_name() {
                "if" => gate.extend(object.remove("condition")),
                "switch" => gate.extend(object.remove("value")),
                _ => {}
            }
        }
        let items = if step.type_name() == "foreach" {
            input.as_object_mut().and_then(|o| o.remove("items"))
        } else {
            None
        };
        let (reads_steps, reads_vars) = references(engine, &input);
        let (gate_steps, _) = references(engine, &Json::Array(gate));
        let (items_steps, items_vars) =
            items.map_or_else(Default::default, |v| references(engine, &v));
        nodes.push(Node {
            step,
            path: here.clone(),
            index,
            ancestors: ancestors.to_vec(),
            reads_steps,
            reads_vars,
            gate_steps,
            items_steps,
            items_vars,
            acts: acts(step, &json),
        });
        let mut inner = ancestors.to_vec();
        inner.push(index);
        for (label, block) in step.blocks() {
            collect(engine, block, &format!("{here}.{label}"), &inner, nodes);
        }
    }
}

/// The step ids and variable names a value's expressions read, leaving out
/// nested steps (they're checked on their own).
fn references(engine: &Engine, value: &Json) -> (HashSet<String>, HashSet<String>) {
    let mut strings = Vec::new();
    strings_outside_steps(value, &mut strings);
    let (mut steps, mut vars) = (HashSet::new(), HashSet::new());
    for s in strings {
        for name in engine.references(s).unwrap_or_default() {
            if let Some(rest) = name.strip_prefix("steps.") {
                steps.insert(rest.split('.').next().unwrap_or_default().to_owned());
            } else if let Some(rest) = name.strip_prefix("vars.") {
                vars.insert(rest.split('.').next().unwrap_or_default().to_owned());
            }
        }
    }
    (steps, vars)
}

fn is_steps(value: &Json) -> bool {
    value.as_array().is_some_and(|items| {
        !items.is_empty()
            && items.iter().all(|item| {
                item.get("type").is_some() && item.get("id").is_some()
                    || item.get("steps").is_some()
            })
    })
}

fn strings_outside_steps<'a>(value: &'a Json, out: &mut Vec<&'a str>) {
    match value {
        Json::String(s) => out.push(s),
        Json::Array(items) if !is_steps(value) => {
            for item in items {
                strings_outside_steps(item, out);
            }
        }
        Json::Object(map) => {
            for item in map.values() {
                if !is_steps(item) {
                    strings_outside_steps(item, out);
                }
            }
        }
        _ => {}
    }
}

fn var_names(step: &Step) -> Vec<String> {
    match step {
        Step::SetVar(s) => s.vars.keys().cloned().collect(),
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NONE: [&str; 0] = [];

    fn problems(steps: &str) -> Vec<String> {
        let yaml = format!(
            "apiVersion: usehuxley.io/v1\nkind: Workflow\nmetadata: {{ key: t, name: T }}\nspec:\n  steps:\n{steps}"
        );
        let workflow = huxley_hwf::parse_yaml(&yaml).unwrap();
        check(&Engine::new(), &workflow)
            .into_iter()
            .map(|d| d.path)
            .collect()
    }

    const DRAFT: &str = "    - id: draft\n      type: ai.prompt\n      with: { prompt: \"Reply to {{ trigger.body }}\" }\n";
    const APPROVE: &str = "    - id: ok\n      type: approval\n      title: Send it?\n      message: \"{{ steps.draft.output.text }}\"\n      approvers: \"role:admin\"\n";

    fn email(extra: &str) -> String {
        format!(
            "    - id: send\n      type: core.email\n{extra}      with: {{ to: a@example.com, subject: Re, text: \"{{{{ steps.draft.output.text }}}}\" }}\n"
        )
    }

    #[test]
    fn acting_on_ai_output_needs_an_approval_after_it() {
        assert_eq!(
            problems(&format!("{DRAFT}{}", email(""))),
            ["spec.steps[1]"]
        );
        let gated = "      if: \"{{ steps.ok.output.decision == 'approved' }}\"\n";
        assert_eq!(problems(&format!("{DRAFT}{APPROVE}{}", email(gated))), NONE);
        // An approval before the AI wrote anything doesn't count.
        let early = "    - id: ok\n      type: approval\n      title: Go?\n      approvers: \"role:admin\"\n";
        assert_eq!(
            problems(&format!("{early}{DRAFT}{}", email(gated))),
            ["spec.steps[2]"]
        );
        // Nor does an if that isn't about an approval.
        let other = "      if: \"{{ steps.draft.output.text != '' }}\"\n";
        assert_eq!(
            problems(&format!("{DRAFT}{}", email(other))),
            ["spec.steps[1]"]
        );
        // Saying so explicitly.
        assert_eq!(
            problems(&format!("{DRAFT}{}", email("      trust_ai: true\n"))),
            NONE
        );
    }

    #[test]
    fn ai_output_is_followed_through_transforms_variables_and_loops() {
        let via_transform = format!(
            "{DRAFT}    - id: shaped\n      type: transform\n      output: \"{{{{ steps.draft.output.text | upper }}}}\"\n    - id: call\n      type: http.request\n      with: {{ method: POST, url: https://example.com, body: \"{{{{ steps.shaped.output }}}}\" }}\n"
        );
        assert_eq!(problems(&via_transform), ["spec.steps[2]"]);
        let via_var = format!(
            "{DRAFT}    - id: keep\n      type: set_var\n      vars: {{ answer: \"{{{{ steps.draft.output.text }}}}\" }}\n    - id: call\n      type: http.request\n      with: {{ url: \"https://example.com/?q={{{{ vars.answer }}}}\" }}\n"
        );
        assert_eq!(problems(&via_var), ["spec.steps[2]"]);
        let looped = "    - id: pick\n      type: ai.extract\n      with: { text: \"{{ trigger.body }}\", schema: { type: object, properties: { hosts: { type: array } } } }\n    - id: each\n      type: foreach\n      items: \"{{ steps.pick.output.hosts }}\"\n      steps:\n        - id: ping\n          type: http.request\n          with: { url: https://example.com/ping }\n";
        assert_eq!(problems(looped), ["spec.steps[1].steps[0]"]);
    }

    #[test]
    fn labels_conditions_and_suggestions_are_free() {
        // ai.classify picks from a fixed list.
        let classify = "    - id: kind\n      type: ai.classify\n      with: { text: \"{{ trigger.body }}\", labels: [spam, ham] }\n    - id: send\n      type: core.email\n      with: { to: a@example.com, subject: \"{{ steps.kind.output.label }}\", text: hi }\n";
        assert_eq!(problems(classify), NONE);
        // Deciding whether to act, not what to do.
        let routed = format!(
            "{DRAFT}    - id: send\n      type: core.email\n      if: \"{{{{ 'urgent' in steps.draft.output.text }}}}\"\n      with: {{ to: a@example.com, subject: Urgent, text: Look }}\n"
        );
        assert_eq!(problems(&routed), NONE);
        // Internal notes are suggestions for staff; public ones aren't.
        let note = |kind: &str| {
            format!(
                "{DRAFT}    - id: note\n      type: core.ticket.note\n      with: {{ ticket_id: \"{{{{ trigger.id }}}}\", body: \"{{{{ steps.draft.output.text }}}}\"{kind} }}\n"
            )
        };
        assert_eq!(problems(&note("")), NONE);
        assert_eq!(problems(&note(", kind: internal")), NONE);
        assert_eq!(problems(&note(", kind: public")), ["spec.steps[1]"]);
    }

    #[test]
    fn an_approval_on_an_enclosing_step_gates_what_is_inside() {
        let steps = format!(
            "{DRAFT}{APPROVE}    - id: approved\n      type: if\n      condition: \"{{{{ steps.ok.output.decision == 'approved' }}}}\"\n      then:\n{}",
            email("").lines().fold(String::new(), |mut out, l| {
                use std::fmt::Write as _;
                let _ = writeln!(out, "    {l}");
                out
            })
        );
        assert_eq!(problems(&steps), NONE, "{:?}", problems(&steps));
    }
}
