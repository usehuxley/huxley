//! Importing real-shaped specs in each supported format.

use huxley_integrations::{Action, AuthKind, ImportError, Imported, ParamLocation, import};
use serde_json::json;

fn fixture(name: &str) -> Imported {
    let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
    import(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn action<'a>(imported: &'a Imported, key: &str) -> &'a Action {
    imported
        .definition
        .actions
        .iter()
        .find(|a| a.key == key)
        .unwrap_or_else(|| panic!("no action {key}"))
}

#[test]
fn openapi_3_0_yaml() {
    let imported = fixture("tickets-3.0.yaml");
    let d = &imported.definition;
    assert_eq!(d.title, "Tickets API");
    assert_eq!(d.version, "2.1");
    assert_eq!(d.source_format, "3.0.3");
    assert_eq!(d.servers[0].url, "https://{tenant}.example.com/api/v2");
    assert_eq!(d.servers[0].variables[0].name, "tenant");
    assert_eq!(d.servers[0].variables[0].default, "demo");
    assert_eq!(d.servers[0].variables[1].options, vec!["x", "y"]);

    // Auth: the API key, OAuth2's two usable flows; digest is skipped.
    let keys: Vec<&str> = d.auth_schemes.iter().map(|s| s.key.as_str()).collect();
    assert_eq!(keys, vec!["apiKey", "oauth", "oauth_client_credentials"]);
    assert_eq!(
        d.auth_schemes[0].kind,
        AuthKind::ApiKey {
            location: ParamLocation::Header,
            name: "X-Api-Key".into()
        }
    );
    assert!(matches!(
        &d.auth_schemes[1].kind,
        AuthKind::OAuth2AuthorizationCode { scopes, .. } if scopes == &["tickets.read", "tickets.write"]
    ));

    // List: path-level and operation parameters merged; Authorization is
    // the connection's business; the output inlines the item schema.
    let list = action(&imported, "listTickets");
    assert_eq!(list.name, "List tickets");
    assert_eq!(list.group.as_deref(), Some("Tickets"));
    assert_eq!(list.method, "GET");
    let names: Vec<&str> = list.parameters.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, vec!["page", "status", "X-Request-Id"]);
    assert_eq!(
        list.input_schema["properties"]["page"],
        json!({ "type": "integer", "minimum": 1, "description": "Page number" })
    );
    assert_eq!(list.input_schema["required"], json!([]));
    let output = list.output_schema.as_ref().unwrap();
    assert_eq!(output["items"]["properties"]["subject"]["type"], "string");
    assert_eq!(
        output["items"]["properties"]["children"]["items"]["description"],
        "Ticket (recursive)"
    );
    assert_eq!(list.auth, vec!["apiKey"]);

    // Create: the body is an input; its own security overrides the default.
    let create = action(&imported, "createTicket");
    assert_eq!(
        create.body.as_ref().unwrap().content_type,
        "application/json"
    );
    assert_eq!(create.input_schema["required"], json!(["body"]));
    assert_eq!(
        create.input_schema["properties"]["body"]["required"],
        json!(["subject"])
    );
    assert_eq!(create.auth, vec!["oauth", "oauth_client_credentials"]);

    // A repeated operation id gets a suffix; path parameters are required;
    // the response came through a $ref.
    let one = action(&imported, "listTickets_2");
    assert_eq!(one.input_schema["required"], json!(["id"]));
    assert_eq!(
        one.output_schema.as_ref().unwrap()["properties"]["id"]["type"],
        "integer"
    );

    // No operation id: one is made up. `security: []` needs no auth.
    let delete = action(&imported, "delete_tickets_id");
    assert!(delete.deprecated);
    assert_eq!(delete.auth, Vec::<String>::new());
    assert_eq!(delete.output_schema, None);

    let warnings: Vec<&str> = imported
        .warnings
        .iter()
        .map(|w| w.message.as_str())
        .collect();
    assert!(
        warnings
            .iter()
            .any(|w| w.contains("\"listTickets\" is used twice")),
        "{warnings:?}"
    );
    assert!(
        warnings.iter().any(|w| w.contains("\"digest\"")),
        "{warnings:?}"
    );
}

#[test]
fn swagger_2_0_is_converted() {
    let imported = fixture("pets-2.0.json");
    let d = &imported.definition;
    assert_eq!(d.source_format, "2.0");
    let urls: Vec<&str> = d.servers.iter().map(|s| s.url.as_str()).collect();
    assert_eq!(
        urls,
        vec!["https://pets.example.com/v1", "http://pets.example.com/v1"]
    );
    assert_eq!(d.auth_schemes[0].kind, AuthKind::Basic);
    assert_eq!(
        d.auth_schemes[1].kind,
        AuthKind::OAuth2ClientCredentials {
            token_url: "https://auth.example.com/token".into(),
            scopes: vec!["pets".into()]
        }
    );

    let list = action(&imported, "list_pets");
    assert_eq!(
        list.input_schema["properties"]["limit"],
        json!({ "type": "integer", "default": 20 })
    );
    assert_eq!(
        list.output_schema.as_ref().unwrap()["items"]["required"],
        json!(["name"])
    );
    assert_eq!(list.auth, vec!["basic", "partner"]);

    let add = action(&imported, "addPet");
    assert_eq!(add.body.as_ref().unwrap().content_type, "application/json");
    assert_eq!(
        add.input_schema["properties"]["body"]["properties"]["tag"]["type"],
        "string"
    );

    let upload = action(&imported, "uploadPhoto");
    assert_eq!(
        upload.body.as_ref().unwrap().content_type,
        "multipart/form-data"
    );
    assert_eq!(
        upload.input_schema["properties"]["body"]["properties"]["file"],
        json!({ "type": "string", "format": "binary" })
    );
    assert_eq!(upload.input_schema["required"], json!(["petId", "body"]));
    assert_eq!(imported.warnings, vec![]);
}

#[test]
fn openapi_3_1_with_gaps() {
    let imported = fixture("minimal-3.1.json");
    let d = &imported.definition;
    assert_eq!(d.version, "");
    assert_eq!(d.servers, vec![]);
    assert!(matches!(
        d.auth_schemes[0].kind,
        AuthKind::OpenIdConnect { .. }
    ));
    let ping = action(&imported, "get_ping");
    assert_eq!(
        ping.output_schema,
        Some(json!({ "type": ["object", "null"] }))
    );
    let put = action(&imported, "put_things_id");
    assert_eq!(
        put.body.as_ref().unwrap().content_type,
        "application/x-www-form-urlencoded"
    );
    // A path parameter the spec forgot to declare is added, with a warning.
    assert_eq!(put.parameters[0].name, "id");
    assert_eq!(put.input_schema["required"], json!(["id"]));
    let pointers: Vec<&str> = imported
        .warnings
        .iter()
        .map(|w| w.pointer.as_str())
        .collect();
    assert_eq!(
        pointers,
        vec!["/info/version", "/servers", "/paths/~1things~1{id}/put"]
    );
}

#[test]
fn not_openapi() {
    assert!(matches!(
        import("{\"hello\": 1}"),
        Err(ImportError::NotOpenApi(_))
    ));
    assert!(matches!(
        import("openapi: 4.0.0\n"),
        Err(ImportError::NotOpenApi(_))
    ));
    assert!(matches!(import("{ nope"), Err(ImportError::Parse(_))));
    let huge = format!(
        "{{\"x\": \"{}\"}}",
        "a".repeat(huxley_integrations::MAX_SPEC_BYTES)
    );
    assert!(matches!(import(&huge), Err(ImportError::TooLarge)));
}
