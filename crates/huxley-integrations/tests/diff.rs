//! Comparing two versions of a spec.

use huxley_integrations::diff::{Rename, diff};
use huxley_integrations::{Definition, import};

fn definition(paths: &str, extra: &str) -> Definition {
    import(&format!(
        "openapi: 3.0.3\ninfo: {{ title: T, version: '1' }}\nservers: [{{ url: 'https://a.example.com' }}]\n{extra}paths:\n{paths}"
    ))
    .unwrap()
    .definition
}

const BASE: &str = r"
  /users:
    get:
      operationId: listUsers
      parameters:
        - { name: limit, in: query, schema: { type: integer } }
        - { name: filter, in: query, schema: { type: string } }
      responses: { '200': { description: ok, content: { application/json: { schema: { type: array } } } } }
    post:
      operationId: createUser
      requestBody: { required: true, content: { application/json: { schema: { type: object } } } }
      responses: { '201': { description: made } }
  /users/{id}:
    delete:
      operationId: deleteUser
      parameters: [ { name: id, in: path, schema: { type: string } } ]
      responses: { '204': { description: gone } }
  /health:
    get:
      operationId: health
      responses: { '200': { description: ok } }
";

const AUTH: &str =
    "components: { securitySchemes: { key: { type: apiKey, in: header, name: X-Key } } }\n";

#[test]
fn identical_specs_have_no_changes() {
    let d = definition(BASE, AUTH);
    let diff = diff(&d, &d);
    assert!(diff.is_empty());
    assert!(!diff.breaking);
}

#[test]
fn additions_are_safe() {
    let newer = BASE.replace(
        "        - { name: filter, in: query, schema: { type: string } }",
        "        - { name: filter, in: query, schema: { type: string } }\n        - { name: sort, in: query, schema: { type: string } }",
    ) + "  /teams:\n    get:\n      operationId: listTeams\n      responses: { '200': { description: ok } }\n";
    let diff = diff(&definition(BASE, AUTH), &definition(&newer, AUTH));
    assert!(!diff.breaking, "{diff:?}");
    assert_eq!(diff.added, vec!["listTeams"]);
    assert_eq!(diff.changed[0].key, "listUsers");
    assert_eq!(
        diff.changed[0].changes[0].message,
        "new optional input sort"
    );
}

#[test]
fn breaking_changes_are_flagged() {
    let newer = BASE
        // A parameter removed, another's type changed, a new required one.
        .replace(
            "        - { name: limit, in: query, schema: { type: integer } }\n        - { name: filter, in: query, schema: { type: string } }",
            "        - { name: limit, in: query, schema: { type: string } }\n        - { name: tenant, in: query, required: true, schema: { type: string } }",
        )
        // Renamed operation id, same method and path.
        .replace("operationId: deleteUser", "operationId: removeUser")
        // Moved.
        .replace("  /health:", "  /status:")
        // The response shape changed: a note, not breaking by itself.
        .replace("schema: { type: array }", "schema: { type: object }");
    let diff = diff(&definition(BASE, AUTH), &definition(&newer, ""));
    assert!(diff.breaking);
    assert_eq!(
        diff.renamed,
        vec![Rename {
            from: "deleteUser".into(),
            to: "removeUser".into()
        }]
    );
    let list = diff.changed.iter().find(|a| a.key == "listUsers").unwrap();
    let messages: Vec<(&str, bool)> = list
        .changes
        .iter()
        .map(|c| (c.message.as_str(), c.breaking))
        .collect();
    assert_eq!(
        messages,
        vec![
            ("input filter was removed", true),
            ("input limit is now string (was integer)", true),
            ("input tenant is now required", true),
            (
                "the response's shape changed: check expressions that read it",
                false
            ),
        ]
    );
    let health = diff.changed.iter().find(|a| a.key == "health").unwrap();
    assert_eq!(
        health.changes[0].message,
        "now GET /status (was GET /health)"
    );
    assert!(health.changes[0].breaking);
    assert_eq!(
        diff.connection[0].message,
        "auth scheme key was removed: connections using it stop working"
    );
}

#[test]
fn removed_actions_break() {
    let newer = BASE.replace(
        "    post:\n      operationId: createUser\n      requestBody: { required: true, content: { application/json: { schema: { type: object } } } }\n      responses: { '201': { description: made } }\n",
        "",
    );
    let diff = diff(&definition(BASE, AUTH), &definition(&newer, AUTH));
    assert!(diff.breaking);
    assert_eq!(diff.removed, vec!["createUser"]);
}
