//! Static checks of workflow expressions.

use huxley_expr::{Engine, check_workflow};

fn problems(steps: &str) -> Vec<String> {
    let yaml = format!(
        "apiVersion: usehuxley.io/v1\nkind: Workflow\nmetadata: {{ key: w, name: W }}\nspec:\n{steps}"
    );
    let workflow = huxley_hwf::parse_yaml(&yaml).unwrap();
    check_workflow(&Engine::new(), &workflow)
        .into_iter()
        .map(|d| d.to_string())
        .collect()
}

fn assert_problems(steps: &str, expected: &[&str]) {
    let found = problems(steps);
    for want in expected {
        assert!(
            found.iter().any(|f| f.starts_with(want)),
            "missing {want:?} in {found:#?}"
        );
    }
    assert_eq!(found.len(), expected.len(), "{found:#?}");
}

#[test]
fn a_well_formed_workflow_has_no_problems() {
    assert_problems(
        r#"  triggers:
    - type: webhook
      auth: { type: bearer, token: "{{ secrets.hook_token }}" }
    - { type: event, event: ticket.created, filter: "{{ event.priority == 'high' }}" }
  steps:
    - { id: first, type: transform, output: "{{ inputs.name ~ uuid() ~ (range(3) | list | length) ~ now() }}" }
    - id: loop
      type: foreach
      items: "{{ steps.first.output | list }}"
      as: letter
      steps:
        - { id: inner, type: transform, output: "{{ letter ~ loop.index ~ steps.first.output }}" }
        - { id: inner2, type: transform, output: "{{ steps.inner.output }}" }
    - id: guarded
      type: try
      steps:
        - { id: risky, type: http.request, with: { url: "https://x/{{ steps.loop.output | length }}" } }
      catch:
        - { id: handle, type: transform, output: "{{ error.message ~ steps.risky.output }}" }
      finally:
        - { id: tidy, type: transform, output: "{{ steps.handle.output }}" }
    - id: pick
      type: switch
      value: "{{ steps.first.output }}"
      cases:
        - { when: "{{ inputs.name }}", steps: [ { id: matched, type: transform, output: 1 } ] }
  outputs:
    result: "{{ steps.tidy.output }}"
    maybe: "{{ steps.matched.output }}"
  on_error:
    - { id: alert, type: core.notify, with: { to: "role:x", title: "{{ error.message }} at {{ steps.first.output }}" } }
"#,
        &[],
    );
}

#[test]
fn bad_references_are_explained() {
    assert_problems(
        r#"  steps:
    - { id: early, type: transform, output: "{{ steps.late.output }}" }
    - { id: self_ref, type: transform, output: "{{ steps.self_ref.output }}" }
    - { id: ghost, type: transform, output: "{{ steps.nowhere.output }}" }
    - id: branch
      type: if
      condition: "{{ steps.branch.output }}"
      then:
        - { id: yes_side, type: transform, output: 1 }
        - { id: parent, type: transform, output: "{{ steps.branch.output }}" }
      else:
        - { id: no_side, type: transform, output: "{{ steps.yes_side.output }}" }
    - id: fan
      type: parallel
      branches:
        - { id: a, steps: [ { id: left, type: transform, output: 1 } ] }
        - { id: b, steps: [ { id: right, type: transform, output: "{{ steps.left.output }}" } ] }
    - id: each
      type: foreach
      items: "{{ [1, 2] }}"
      steps: [ { id: body, type: transform, output: "{{ item }}" } ]
    - { id: after, type: transform, output: "{{ steps.body.output }}" }
    - { id: late, type: transform, output: 1 }
"#,
        &[
            "spec.steps[0].output: step \"late\" runs after this",
            "spec.steps[1].output: step \"self_ref\" can't read its own output",
            "spec.steps[2].output: there is no step \"nowhere\"",
            "spec.steps[3].condition: step \"branch\" can't read its own output",
            "spec.steps[3].then[1].output: step \"branch\" contains this one",
            "spec.steps[3].else[0].output: step \"yes_side\" is in another branch of \"branch\"",
            "spec.steps[4].branches[1].steps[0].output: step \"left\" is in another branch of \"fan\", which runs at the same time",
            "spec.steps[6].output: step \"body\" is inside the loop \"each\"",
        ],
    );
}

#[test]
fn names_and_syntax_are_checked() {
    assert_problems(
        r#"  triggers:
    - { type: event, event: x, filter: "{{ steps.a.output }}" }
  steps:
    - { id: a, type: transform, output: "{{ item }}" }
    - { id: b, type: transform, output: "{{ error.message }}" }
    - { id: c, type: transform, output: "{{ inputs.x | }}" }
    - { id: d, type: transform, output: "{% for x in inputs %}{{ x }}{% endfor %} {{ undefined_thing }}" }
"#,
        &[
            "spec.triggers[0].filter: \"steps\" isn't available here",
            "spec.steps[0].output: \"item\" isn't available here",
            "spec.steps[1].output: \"error\" isn't available here",
            "spec.steps[2].output: syntax error",
            "spec.steps[3].output: \"undefined_thing\" isn't available here",
        ],
    );
}
