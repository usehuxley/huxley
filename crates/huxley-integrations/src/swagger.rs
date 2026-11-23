//! Swagger 2.0 to OpenAPI 3.0, as far as the importer needs: servers from
//! `schemes`/`host`/`basePath`, `definitions` and `securityDefinitions`
//! under `components`, `body` and `formData` parameters as request bodies,
//! and response schemas under a media type.

use serde_json::{Map, Value as Json, json};

use crate::refs::lookup;

/// Converts a Swagger 2.0 document. `$ref`s are rewritten to their
/// OpenAPI 3 locations.
pub(crate) fn convert(doc: &Json) -> Json {
    let consumes = strings(&doc["consumes"]);
    let produces = strings(&doc["produces"]);
    let mut components = Map::new();
    if let Some(defs) = doc.get("definitions") {
        components.insert("schemas".into(), rewrite_refs(defs));
    }
    if let Some(responses) = doc.get("responses") {
        components.insert("responses".into(), rewrite_refs(responses));
    }
    if let Some(defs) = doc.get("securityDefinitions").and_then(Json::as_object) {
        let schemes: Map<String, Json> = defs
            .iter()
            .map(|(k, v)| (k.clone(), security_scheme(v)))
            .collect();
        components.insert("securitySchemes".into(), Json::Object(schemes));
    }

    let mut paths = Map::new();
    if let Some(items) = doc["paths"].as_object() {
        for (path, item) in items {
            let Some(item) = item.as_object() else {
                continue;
            };
            let shared = item.get("parameters").cloned().unwrap_or(json!([]));
            let mut out = Map::new();
            for (method, op) in item {
                if method == "parameters" || !op.is_object() {
                    continue;
                }
                out.insert(
                    method.clone(),
                    operation(doc, op, &shared, &consumes, &produces),
                );
            }
            paths.insert(path.clone(), Json::Object(out));
        }
    }

    let mut converted = json!({
        "openapi": "3.0.3",
        "info": doc.get("info").cloned().unwrap_or(json!({})),
        "servers": servers(doc),
        "paths": paths,
        "components": components,
    });
    if let Some(security) = doc.get("security") {
        converted["security"] = security.clone();
    }
    if let Some(tags) = doc.get("tags") {
        converted["tags"] = tags.clone();
    }
    converted
}

fn strings(value: &Json) -> Vec<String> {
    value
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(Json::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

fn servers(doc: &Json) -> Json {
    let base = doc["basePath"].as_str().unwrap_or("");
    let Some(host) = doc["host"].as_str() else {
        return json!([{ "url": if base.is_empty() { "/" } else { base } }]);
    };
    let mut schemes = strings(&doc["schemes"]);
    if schemes.is_empty() {
        schemes.push("https".into());
    }
    Json::Array(
        schemes
            .iter()
            .map(|scheme| json!({ "url": format!("{scheme}://{host}{base}") }))
            .collect(),
    )
}

fn security_scheme(def: &Json) -> Json {
    let description = def.get("description").cloned();
    let mut scheme = match def["type"].as_str() {
        Some("basic") => json!({ "type": "http", "scheme": "basic" }),
        Some("apiKey") => json!({ "type": "apiKey", "in": def["in"], "name": def["name"] }),
        Some("oauth2") => {
            let scopes = def.get("scopes").cloned().unwrap_or(json!({}));
            let flow = match def["flow"].as_str() {
                Some("application") => json!({ "clientCredentials": {
                    "tokenUrl": def["tokenUrl"], "scopes": scopes } }),
                Some("accessCode") => json!({ "authorizationCode": {
                    "authorizationUrl": def["authorizationUrl"],
                    "tokenUrl": def["tokenUrl"], "scopes": scopes } }),
                Some("implicit") => json!({ "implicit": {
                    "authorizationUrl": def["authorizationUrl"], "scopes": scopes } }),
                Some("password") => json!({ "password": {
                    "tokenUrl": def["tokenUrl"], "scopes": scopes } }),
                _ => json!({}),
            };
            json!({ "type": "oauth2", "flows": flow })
        }
        _ => def.clone(),
    };
    if let (Some(d), Json::Object(map)) = (description, &mut scheme) {
        map.insert("description".into(), d);
    }
    scheme
}

fn operation(
    doc: &Json,
    op: &Json,
    shared: &Json,
    consumes: &[String],
    produces: &[String],
) -> Json {
    let mut out = op.as_object().cloned().unwrap_or_default();
    let consumes = Some(strings(&op["consumes"]))
        .filter(|c| !c.is_empty())
        .unwrap_or_else(|| consumes.to_vec());
    let produces = Some(strings(&op["produces"]))
        .filter(|p| !p.is_empty())
        .unwrap_or_else(|| produces.to_vec());
    out.remove("consumes");
    out.remove("produces");

    // Operation parameters override shared ones with the same name and place.
    let mut params: Vec<Json> = Vec::new();
    for p in shared
        .as_array()
        .into_iter()
        .flatten()
        .chain(op["parameters"].as_array().into_iter().flatten())
    {
        let p = p
            .get("$ref")
            .and_then(Json::as_str)
            .and_then(|r| lookup(doc, r))
            .unwrap_or(p);
        params.retain(|q| !(q["name"] == p["name"] && q["in"] == p["in"]));
        params.push(p.clone());
    }

    let mut parameters = Vec::new();
    let mut form = Map::new();
    let mut form_required = Vec::new();
    let mut has_file = false;
    for p in &params {
        match p["in"].as_str() {
            Some("body") => {
                let content_type = consumes
                    .iter()
                    .find(|c| c.contains("json"))
                    .or_else(|| consumes.first())
                    .map_or("application/json", String::as_str);
                out.insert(
                    "requestBody".into(),
                    json!({
                        "description": p.get("description"),
                        "required": p["required"].as_bool().unwrap_or(false),
                        "content": { content_type: { "schema": rewrite_refs(&p["schema"]) } },
                    }),
                );
            }
            Some("formData") => {
                has_file |= p["type"] == "file";
                if p["required"].as_bool().unwrap_or(false) {
                    form_required.push(p["name"].clone());
                }
                let mut schema = simple_schema(p);
                if p["type"] == "file" {
                    schema = json!({ "type": "string", "format": "binary" });
                }
                form.insert(p["name"].as_str().unwrap_or_default().to_owned(), schema);
            }
            Some(location) => parameters.push(json!({
                "name": p["name"],
                "in": location,
                "required": p["required"].as_bool().unwrap_or(location == "path"),
                "description": p.get("description"),
                "schema": simple_schema(p),
            })),
            None => {}
        }
    }
    if !form.is_empty() {
        let content_type = if has_file || consumes.iter().any(|c| c == "multipart/form-data") {
            "multipart/form-data"
        } else {
            "application/x-www-form-urlencoded"
        };
        out.insert(
            "requestBody".into(),
            json!({
                "required": !form_required.is_empty(),
                "content": { content_type: { "schema": {
                    "type": "object", "properties": form, "required": form_required,
                } } },
            }),
        );
    }
    out.insert("parameters".into(), Json::Array(parameters));

    if let Some(responses) = op["responses"].as_object() {
        out.insert("responses".into(), responses_v3(responses, &produces));
    }
    Json::Object(out)
}

/// 2.0 responses (a `schema` each) as 3.0 ones (a schema per media type).
fn responses_v3(responses: &Map<String, Json>, produces: &[String]) -> Json {
    let media = produces
        .iter()
        .find(|p| p.contains("json"))
        .or_else(|| produces.first())
        .map_or("application/json", String::as_str);
    let converted: Map<String, Json> = responses
        .iter()
        .map(|(code, r)| {
            if let Some(reference) = r.get("$ref").and_then(Json::as_str) {
                return (code.clone(), json!({ "$ref": rewrite_ref(reference) }));
            }
            let description = r.get("description").cloned().unwrap_or(json!(""));
            let mut response = json!({ "description": description });
            if let Some(schema) = r.get("schema") {
                response["content"] = json!({ media: { "schema": rewrite_refs(schema) } });
            }
            (code.clone(), response)
        })
        .collect();
    Json::Object(converted)
}

/// A 2.0 non-body parameter's type fields as a schema.
fn simple_schema(p: &Json) -> Json {
    let mut schema = Map::new();
    for key in [
        "type", "format", "items", "enum", "default", "minimum", "maximum", "pattern",
    ] {
        if let Some(v) = p.get(key) {
            schema.insert(key.into(), rewrite_refs(v));
        }
    }
    Json::Object(schema)
}

fn rewrite_ref(reference: &str) -> String {
    for (from, to) in [
        ("#/definitions/", "#/components/schemas/"),
        ("#/parameters/", "#/components/parameters/"),
        ("#/responses/", "#/components/responses/"),
    ] {
        if let Some(rest) = reference.strip_prefix(from) {
            return format!("{to}{rest}");
        }
    }
    reference.to_owned()
}

fn rewrite_refs(value: &Json) -> Json {
    match value {
        Json::Object(map) => Json::Object(
            map.iter()
                .map(|(k, v)| {
                    let v = match (k.as_str(), v) {
                        ("$ref", Json::String(r)) => Json::String(rewrite_ref(r)),
                        _ => rewrite_refs(v),
                    };
                    (k.clone(), v)
                })
                .collect(),
        ),
        Json::Array(items) => Json::Array(items.iter().map(rewrite_refs).collect()),
        other => other.clone(),
    }
}
