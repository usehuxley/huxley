//! Reading, validating and writing HWF documents.

use std::fmt::Write as _;

use huxley_hwf::{HwfError, Step, Trigger, json_schema, parse_json, parse_yaml, to_yaml};

const EXAMPLE: &str = r#"
apiVersion: usehuxley.com/v1
kind: Workflow
metadata:
  key: onboard-user
  name: Onboard user
  tags: [m365, onboarding]
spec:
  inputs:
    type: object
    required: [firstName, lastName]
    properties:
      firstName: { type: string }
      lastName: { type: string }
  triggers:
    - type: manual
    - type: webhook
      auth: { type: hmac, secret: "{{ vars.hook_secret }}", prefix: "sha256=" }
      response: sync
      timeout: 30s
    - type: schedule
      cron: "0 7 * * 1-5"
      timezone: Europe/London
  steps:
    - id: build_upn
      type: transform
      output: "{{ (inputs.firstName ~ '.' ~ inputs.lastName) | lower }}@example.com"
    - id: create_user
      type: http.request
      with:
        method: POST
        url: https://graph.example.com/v1.0/users
        headers: { Authorization: "Bearer {{ vars.token }}" }
        body: { userPrincipalName: "{{ steps.build_upn.output }}" }
      retry: { max_attempts: 3, backoff: jitter, on: ["429", 5xx] }
      timeout: 30s
    - id: check
      type: if
      condition: "{{ steps.create_user.output.status == 201 }}"
      then:
        - id: groups
          type: foreach
          items: "{{ ['staff', 'vpn'] }}"
          as: group
          concurrency: 2
          steps:
            - id: add_group
              type: http.request
              with: { method: POST, url: "https://graph.example.com/groups/{{ group }}/members" }
      else:
        - id: give_up
          type: fail
          message: "Graph said {{ steps.create_user.output.status }}"
    - id: tell
      type: core.notify
      with: { to: "role:service-desk", title: "Created {{ steps.build_upn.output }}" }
  outputs:
    upn: "{{ steps.build_upn.output }}"
  on_error:
    - id: alert
      type: core.notify
      with: { to: [ "team:leads" ], title: "Onboarding failed", message: "{{ error.message }}" }
ui:
  nodes: { build_upn: { x: 120, y: 80 } }
"#;

fn diagnostics(yaml: &str) -> Vec<String> {
    match parse_yaml(yaml) {
        Err(HwfError::Invalid(d)) => d.into_iter().map(|d| d.to_string()).collect(),
        other => panic!("expected validation errors, got {other:?}"),
    }
}

/// A minimal valid document with `steps` replaced.
fn with_steps(steps: &str) -> String {
    format!(
        "apiVersion: usehuxley.com/v1\nkind: Workflow\nmetadata: {{ key: w, name: W }}\nspec:\n  steps:\n{steps}"
    )
}

#[test]
fn reads_the_example() {
    let workflow = parse_yaml(EXAMPLE).unwrap();
    assert_eq!(workflow.metadata.key, "onboard-user");
    assert_eq!(workflow.spec.triggers.len(), 3);
    assert!(
        matches!(&workflow.spec.triggers[1], Trigger::Webhook(w) if w.timeout.as_deref() == Some("30s"))
    );
    let ids: Vec<_> = workflow.spec.steps.iter().map(Step::id).collect();
    assert_eq!(ids, ["build_upn", "create_user", "check", "tell"]);
    let Step::If(check) = &workflow.spec.steps[2] else {
        panic!()
    };
    assert_eq!(check.then[0].type_name(), "foreach");
    assert!(workflow.ui.is_some());
}

#[test]
fn yaml_and_json_round_trip() {
    let workflow = parse_yaml(EXAMPLE).unwrap();
    let yaml = to_yaml(&workflow);
    assert_eq!(parse_yaml(&yaml).unwrap(), workflow);
    let json = serde_json::to_string(&workflow).unwrap();
    assert_eq!(parse_json(&json).unwrap(), workflow);
    // Defaults aren't written back out.
    assert!(!yaml.contains("continue_on_error"));
}

#[test]
fn unknown_fields_and_types_are_errors() {
    let typo =
        with_steps("    - { id: a, type: transform, output: 1, retyr: { max_attempts: 2 } }\n");
    let Err(HwfError::Parse(message)) = parse_yaml(&typo) else {
        panic!()
    };
    assert!(message.contains("retyr"), "{message}");

    let unknown = with_steps("    - { id: a, type: teleport }\n");
    let Err(HwfError::Parse(message)) = parse_yaml(&unknown) else {
        panic!()
    };
    assert!(message.contains("teleport"), "{message}");

    // YAML 1.2 booleans: `no` is not false.
    let yes_no = with_steps("    - { id: a, type: transform, output: 1, continue_on_error: no }\n");
    assert!(matches!(parse_yaml(&yes_no), Err(HwfError::Parse(_))));
}

#[test]
fn structural_problems_are_reported_with_paths() {
    let found = diagnostics(&with_steps(
        r"    - { id: a, type: transform, output: 1, timeout: 5 minutes }
    - { id: a, type: transform, output: 2 }
    - { id: Bad-Id, type: transform, output: 3, retry: { max_attempts: 1, on: [teapot] } }
    - id: loop
      type: foreach
      items: '{{ [1] }}'
      concurrency: 500
      as: steps
      steps:
        - { id: done, type: return }
        - { id: after, type: transform, output: 4 }
    - { id: p, type: parallel, branches: [ { id: only, steps: [] } ] }
    - { id: t, type: try, steps: [] }
    - { id: w, type: wait }
",
    ));
    let expected = [
        "spec.steps[0].timeout: \"5 minutes\" is not a duration",
        "spec.steps[1].id: another step is already \"a\"",
        "spec.steps[2].id: use lowercase letters",
        "spec.steps[2].retry.max_attempts: between 2 and 20",
        "spec.steps[2].retry.on[0]: \"teapot\"",
        "spec.steps[3].concurrency: between 1 and 50",
        "spec.steps[3].as: a lowercase name",
        "spec.steps[3].steps[1]: unreachable",
        "spec.steps[4].branches: a parallel step needs at least two branches",
        "spec.steps[5]: a try step needs catch or finally",
        "spec.steps[6]: set exactly one of duration or until",
    ];
    for want in expected {
        assert!(
            found.iter().any(|f| f.starts_with(want)),
            "missing {want:?} in {found:#?}"
        );
    }
    assert_eq!(found.len(), expected.len(), "{found:#?}");
}

#[test]
fn document_level_checks() {
    let found = diagnostics(
        r"apiVersion: usehuxley.com/v2
kind: Workflow
metadata: { key: Not A Key, name: ' ' }
spec:
  inputs: { type: array }
  triggers:
    - { type: schedule }
    - { type: schedule, interval: 15m, timezone: UTC }
    - { type: webhook, timeout: 10m }
  steps: []
",
    );
    let expected = [
        "apiVersion: must be \"usehuxley.com/v1\"",
        "metadata.key:",
        "metadata.name: is required",
        "spec.inputs: must be a JSON Schema",
        "spec.triggers[0]: set exactly one of cron or interval",
        "spec.triggers[1].id: another trigger is already \"schedule\"",
        "spec.triggers[1].timezone: only applies to cron",
        "spec.triggers[2].timeout: at most 5m",
        "spec.steps: a workflow needs at least one step",
    ];
    for want in expected {
        assert!(
            found.iter().any(|f| f.starts_with(want)),
            "missing {want:?} in {found:#?}"
        );
    }
    assert_eq!(found.len(), expected.len(), "{found:#?}");
}

#[test]
fn oversized_and_hostile_documents_are_refused() {
    assert!(matches!(
        parse_yaml(&"#".repeat(huxley_hwf::MAX_DOCUMENT_BYTES + 1)),
        Err(HwfError::TooLarge)
    ));
    // Alias expansion ("billion laughs") inside the free-form `ui` block is
    // bounded by the parser's budget.
    let mut bomb = with_steps("    - { id: a, type: transform, output: 1 }\n");
    bomb.push_str("ui:\n  a: &a [x,x,x,x,x,x,x,x,x,x]\n");
    for i in 0..12 {
        let prev = if i == 0 {
            "a".to_owned()
        } else {
            format!("l{}", i - 1)
        };
        let refs = vec![format!("*{prev}"); 10].join(",");
        writeln!(bomb, "  l{i}: &l{i} [{refs}]").unwrap();
    }
    let Err(HwfError::Parse(message)) = parse_yaml(&bomb) else {
        panic!("the bomb parsed")
    };
    assert!(
        message.contains("budget") || message.contains("alias"),
        "{message}"
    );
}

/// The published schema is generated from the types. Regenerate with
/// `UPDATE_SCHEMA=1 cargo test -p huxley-hwf`.
#[test]
fn published_schema_is_up_to_date() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../schemas/hwf/v1.json");
    let generated = serde_json::to_string_pretty(&json_schema()).unwrap() + "\n";
    if std::env::var_os("UPDATE_SCHEMA").is_some() {
        std::fs::create_dir_all(std::path::Path::new(path).parent().unwrap()).unwrap();
        std::fs::write(path, &generated).unwrap();
    }
    let published = std::fs::read_to_string(path).unwrap_or_default();
    assert!(
        published == generated,
        "schemas/hwf/v1.json is stale: run UPDATE_SCHEMA=1 cargo test -p huxley-hwf"
    );
    let schema = json_schema();
    assert_eq!(schema["$id"], "https://usehuxley.com/schemas/hwf/v1.json");
    // Step types are listed by their HWF names.
    let text = generated;
    for name in ["\"http.request\"", "\"core.notify\"", "\"foreach\""] {
        assert!(text.contains(name), "{name}");
    }
}

#[test]
fn script_environment_names_are_checked() {
    let ok = with_steps(
        "    - { id: a, type: script, script: greet, env: { REGION: eu }, connections: { GRAPH: msgraph/contoso } }\n",
    );
    assert!(parse_yaml(&ok).is_ok());
    let bad = with_steps(
        "    - { id: a, type: script, script: greet, env: { lower: x, HUXLEY_X: y, PATH: z, GRAPH_TOKEN: t }, \
           connections: { GRAPH: \"{{ x }}\" } }\n",
    );
    let found = diagnostics(&bad).join("\n");
    for expected in [
        "env.lower",
        "env.HUXLEY_X",
        "env.PATH",
        "env.GRAPH_TOKEN",
        "connections.GRAPH",
    ] {
        assert!(found.contains(expected), "{expected} not in {found}");
    }
}
