//! OpenAPI 3.x (and converted Swagger 2.0) to a [`Definition`].

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Map, Value as Json, json};

use crate::refs::Resolver;
use crate::{
    Action, AuthKind, AuthScheme, Body, Definition, ImportError, Imported, MAX_ACTIONS,
    MAX_SPEC_BYTES, ParamLocation, Parameter, Server, ServerVariable,
};

const METHODS: [&str; 8] = [
    "get", "put", "post", "delete", "options", "head", "patch", "trace",
];

/// Imports a spec given as JSON or YAML text.
pub fn import(text: &str) -> Result<Imported, ImportError> {
    if text.len() > MAX_SPEC_BYTES {
        return Err(ImportError::TooLarge);
    }
    let doc: Json = if text.trim_start().starts_with('{') {
        serde_json::from_str(text).map_err(|e| ImportError::Parse(e.to_string()))?
    } else {
        let options = serde_saphyr::options! {
            // YAML 1.2: `yes`/`no`/`on`/`off` stay strings.
            strict_booleans: true,
        };
        serde_saphyr::from_str_with_options(text, options)
            .map_err(|e| ImportError::Parse(e.to_string()))?
    };
    import_value(doc)
}

/// Imports a spec that's already been parsed.
pub fn import_value(doc: Json) -> Result<Imported, ImportError> {
    let (doc, source_format) = if let Some(v) = doc["openapi"].as_str() {
        if !v.starts_with("3.") {
            return Err(ImportError::NotOpenApi(format!("openapi version {v}")));
        }
        let v = v.to_owned();
        (doc, v)
    } else if doc["swagger"].as_str() == Some("2.0") {
        (crate::swagger::convert(&doc), "2.0".to_owned())
    } else {
        return Err(ImportError::NotOpenApi(
            "it has neither an `openapi` nor a `swagger: \"2.0\"` field".into(),
        ));
    };

    let mut r = Resolver::new(&doc);
    let info = &doc["info"];
    let title = text(&info["title"]).unwrap_or_else(|| {
        r.warn("/info/title", "no title; using \"Untitled API\"");
        "Untitled API".into()
    });
    let version = text(&info["version"]).unwrap_or_else(|| {
        r.warn("/info/version", "no version");
        String::new()
    });
    let servers = servers(&doc, &mut r);
    let (auth_schemes, auth_keys) = auth_schemes(&doc, &mut r);
    let default_auth = security(&doc["security"], &auth_keys);
    let actions = actions(&doc, &mut r, &default_auth, &auth_keys)?;

    Ok(Imported {
        definition: Definition {
            title,
            version,
            description: text(&info["description"]),
            source_format,
            servers,
            auth_schemes,
            actions,
        },
        warnings: r.warnings,
    })
}

fn text(value: &Json) -> Option<String> {
    value
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

/// A JSON pointer segment (`/` and `~` escaped).
fn escape(segment: &str) -> String {
    segment.replace('~', "~0").replace('/', "~1")
}

fn servers(doc: &Json, r: &mut Resolver<'_>) -> Vec<Server> {
    let list = doc["servers"].as_array().cloned().unwrap_or_default();
    if list.is_empty() {
        r.warn("/servers", "no servers: connections will need a base URL");
    }
    list.iter()
        .filter_map(|s| {
            let url = text(&s["url"])?;
            let variables = s["variables"]
                .as_object()
                .map(|vars| {
                    vars.iter()
                        .map(|(name, v)| ServerVariable {
                            name: name.clone(),
                            default: match &v["default"] {
                                Json::String(d) => d.clone(),
                                Json::Null => String::new(),
                                other => other.to_string(),
                            },
                            options: v["enum"]
                                .as_array()
                                .map(|e| {
                                    e.iter()
                                        .filter_map(Json::as_str)
                                        .map(str::to_owned)
                                        .collect()
                                })
                                .unwrap_or_default(),
                            description: text(&v["description"]),
                        })
                        .collect()
                })
                .unwrap_or_default();
            Some(Server {
                url,
                description: text(&s["description"]),
                variables,
            })
        })
        .collect()
}

/// The schemes, and each spec scheme name's keys among them (an `OAuth2`
/// scheme with two usable flows becomes two schemes).
fn auth_schemes(
    doc: &Json,
    r: &mut Resolver<'_>,
) -> (Vec<AuthScheme>, BTreeMap<String, Vec<String>>) {
    let mut schemes = Vec::new();
    let mut keys: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let Some(defs) = doc["components"]["securitySchemes"].as_object() else {
        return (schemes, keys);
    };
    for (name, def) in defs {
        let at = format!("/components/securitySchemes/{}", escape(name));
        let Some(def) = r.deref(def, &at) else {
            continue;
        };
        let description = text(&def["description"]);
        let mut push = |key: String, kind: AuthKind| {
            keys.entry(name.clone()).or_default().push(key.clone());
            schemes.push(AuthScheme {
                key,
                description: description.clone(),
                kind,
            });
        };
        match def["type"].as_str() {
            Some("apiKey") => match location(&def["in"]) {
                Some(location) if location != ParamLocation::Path => push(
                    name.clone(),
                    AuthKind::ApiKey {
                        location,
                        name: def["name"].as_str().unwrap_or_default().to_owned(),
                    },
                ),
                _ => r.warn(&at, "an API key must go in a header, query or cookie"),
            },
            Some("http") => match def["scheme"]
                .as_str()
                .map(str::to_ascii_lowercase)
                .as_deref()
            {
                Some("basic") => push(name.clone(), AuthKind::Basic),
                Some("bearer") => push(
                    name.clone(),
                    AuthKind::Bearer {
                        format: text(&def["bearerFormat"]),
                    },
                ),
                other => r.warn(&at, format!("HTTP auth scheme {other:?} isn't supported")),
            },
            Some("oauth2") => {
                let flows = &def["flows"];
                let scopes = |flow: &Json| -> Vec<String> {
                    flow["scopes"]
                        .as_object()
                        .map(|s| s.keys().cloned().collect())
                        .unwrap_or_default()
                };
                let code = &flows["authorizationCode"];
                let client = &flows["clientCredentials"];
                if code.is_object() {
                    push(
                        name.clone(),
                        AuthKind::OAuth2AuthorizationCode {
                            authorization_url: text(&code["authorizationUrl"]).unwrap_or_default(),
                            token_url: text(&code["tokenUrl"]).unwrap_or_default(),
                            refresh_url: text(&code["refreshUrl"]),
                            scopes: scopes(code),
                        },
                    );
                }
                if client.is_object() {
                    let key = if code.is_object() {
                        format!("{name}_client_credentials")
                    } else {
                        name.clone()
                    };
                    push(
                        key,
                        AuthKind::OAuth2ClientCredentials {
                            token_url: text(&client["tokenUrl"]).unwrap_or_default(),
                            scopes: scopes(client),
                        },
                    );
                }
                if !code.is_object() && !client.is_object() {
                    r.warn(
                        &at,
                        "only the authorization code and client credentials OAuth2 flows are supported",
                    );
                }
            }
            Some("openIdConnect") => match text(&def["openIdConnectUrl"]) {
                Some(url) => push(name.clone(), AuthKind::OpenIdConnect { discovery_url: url }),
                None => r.warn(&at, "openIdConnect without an openIdConnectUrl"),
            },
            other => r.warn(&at, format!("auth type {other:?} isn't supported")),
        }
    }
    (schemes, keys)
}

fn location(value: &Json) -> Option<ParamLocation> {
    match value.as_str()? {
        "path" => Some(ParamLocation::Path),
        "query" => Some(ParamLocation::Query),
        "header" => Some(ParamLocation::Header),
        "cookie" => Some(ParamLocation::Cookie),
        _ => None,
    }
}

/// A `security` requirement list as scheme keys: any one of them works.
/// (Requirements that need several schemes at once count as each.)
fn security(value: &Json, keys: &BTreeMap<String, Vec<String>>) -> Vec<String> {
    let mut out = BTreeSet::new();
    for requirement in value.as_array().into_iter().flatten() {
        for name in requirement.as_object().into_iter().flat_map(Map::keys) {
            out.extend(keys.get(name).into_iter().flatten().cloned());
        }
    }
    out.into_iter().collect()
}

/// An identifier from an operation id or a method and path.
fn identifier(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut last_underscore = true;
    for c in raw.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c);
            last_underscore = false;
        } else if !last_underscore {
            out.push('_');
            last_underscore = true;
        }
    }
    let out = out.trim_end_matches('_').to_owned();
    if out.starts_with(|c: char| c.is_ascii_digit()) {
        format!("op_{out}")
    } else {
        out
    }
}

fn actions(
    doc: &Json,
    r: &mut Resolver<'_>,
    default_auth: &[String],
    auth_keys: &BTreeMap<String, Vec<String>>,
) -> Result<Vec<Action>, ImportError> {
    let mut actions = Vec::new();
    let mut taken = BTreeSet::new();
    let Some(paths) = doc["paths"].as_object() else {
        r.warn("/paths", "no paths: nothing to import");
        return Ok(actions);
    };
    for (path, item) in paths {
        let item_at = format!("/paths/{}", escape(path));
        let Some(item) = r.deref(item, &item_at) else {
            continue;
        };
        for method in METHODS {
            let Some(op) = item.get(method).filter(|o| o.is_object()) else {
                continue;
            };
            if actions.len() >= MAX_ACTIONS {
                return Err(ImportError::TooManyActions);
            }
            let at = format!("{item_at}/{method}");
            let mut key = text(&op["operationId"])
                .map(|id| identifier(&id))
                .filter(|k| !k.is_empty())
                .unwrap_or_else(|| identifier(&format!("{method} {path}")));
            if !taken.insert(key.clone()) {
                let base = key.clone();
                let mut n = 2;
                while !taken.insert(format!("{base}_{n}")) {
                    n += 1;
                }
                key = format!("{base}_{n}");
                r.warn(
                    &at,
                    format!("operation id {base:?} is used twice; this one is {key:?}"),
                );
            }
            let auth = if op.get("security").is_some() {
                security(&op["security"], auth_keys)
            } else {
                default_auth.to_vec()
            };
            actions.push(action(r, &at, key, method, path, item, op, auth));
        }
    }
    Ok(actions)
}

/// An action's parameters, as `Parameter`s and as input schema properties.
struct Inputs {
    parameters: Vec<Parameter>,
    properties: Map<String, Json>,
    required: Vec<Json>,
}

fn inputs(r: &mut Resolver<'_>, at: &str, path: &str, item: &Json, op: &Json) -> Inputs {
    // Operation parameters override path-level ones with the same name and place.
    let mut raw: Vec<Json> = Vec::new();
    for p in item["parameters"]
        .as_array()
        .into_iter()
        .flatten()
        .chain(op["parameters"].as_array().into_iter().flatten())
    {
        let Some(p) = r.deref(p, at) else { continue };
        raw.retain(|q| !(q["name"] == p["name"] && q["in"] == p["in"]));
        raw.push(p.clone());
    }

    let mut properties = Map::new();
    let mut required = Vec::new();
    let mut parameters = Vec::new();
    for p in &raw {
        let (Some(name), Some(location)) = (text(&p["name"]), location(&p["in"])) else {
            r.warn(
                at,
                "a parameter without a usable name or location was skipped",
            );
            continue;
        };
        // An API key's header is set by the connection, not the action.
        if location == ParamLocation::Header
            && ["authorization", "content-type", "accept"]
                .contains(&name.to_ascii_lowercase().as_str())
        {
            continue;
        }
        if properties.contains_key(&name) {
            r.warn(
                at,
                format!("two parameters are named {name:?}; the second is skipped"),
            );
            continue;
        }
        let is_required = location == ParamLocation::Path || p["required"].as_bool() == Some(true);
        let mut schema = p
            .get("schema")
            .map_or_else(|| json!({}), |s| r.inline(s, at));
        if let (Some(d), Json::Object(map)) = (text(&p["description"]), &mut schema) {
            map.entry("description").or_insert(Json::String(d));
        }
        properties.insert(name.clone(), schema);
        if is_required {
            required.push(Json::String(name.clone()));
        }
        parameters.push(Parameter {
            name,
            location,
            required: is_required,
            description: text(&p["description"]),
        });
    }

    // Path placeholders the spec forgot to declare: without them the URL
    // can't be built.
    for placeholder in path
        .split('{')
        .skip(1)
        .filter_map(|s| s.split_once('}'))
        .map(|(n, _)| n)
    {
        if !parameters
            .iter()
            .any(|p| p.location == ParamLocation::Path && p.name == placeholder)
        {
            r.warn(
                at,
                format!("path parameter {{{placeholder}}} isn't declared; added as a string"),
            );
            properties.insert(placeholder.to_owned(), json!({ "type": "string" }));
            required.push(Json::String(placeholder.to_owned()));
            parameters.push(Parameter {
                name: placeholder.to_owned(),
                location: ParamLocation::Path,
                required: true,
                description: None,
            });
        }
    }

    Inputs {
        parameters,
        properties,
        required,
    }
}

#[allow(clippy::too_many_arguments)]
fn action(
    r: &mut Resolver<'_>,
    at: &str,
    key: String,
    method: &str,
    path: &str,
    item: &Json,
    op: &Json,
    auth: Vec<String>,
) -> Action {
    let Inputs {
        parameters,
        mut properties,
        mut required,
    } = inputs(r, at, path, item, op);
    let body = request_body(r, at, op);
    let body = body.map(|(body, schema)| {
        if properties.contains_key("body") {
            r.warn(
                at,
                "a parameter named \"body\" hides the request body in the inputs",
            );
        } else {
            properties.insert("body".into(), schema);
            if body.required {
                required.push(json!("body"));
            }
        }
        body
    });

    Action {
        key,
        name: text(&op["summary"])
            .or_else(|| text(&op["operationId"]))
            .unwrap_or_else(|| format!("{} {path}", method.to_ascii_uppercase())),
        description: text(&op["description"]),
        group: op["tags"].get(0).and_then(text),
        method: method.to_ascii_uppercase(),
        path: path.to_owned(),
        parameters,
        body,
        input_schema: json!({ "type": "object", "properties": properties, "required": required }),
        output_schema: output(r, at, op),
        auth,
        deprecated: op["deprecated"].as_bool() == Some(true),
    }
}

/// The request body: its preferred content type (JSON, then forms, then
/// whatever comes first) and that type's schema, inlined.
fn request_body(r: &mut Resolver<'_>, at: &str, op: &Json) -> Option<(Body, Json)> {
    let b = r.deref(op.get("requestBody")?, at)?;
    let content = b["content"].as_object()?;
    let content_type = content
        .keys()
        .find(|k| *k == "application/json" || k.ends_with("+json"))
        .or_else(|| {
            content
                .keys()
                .find(|k| k.starts_with("application/x-www-form-urlencoded"))
        })
        .or_else(|| {
            content
                .keys()
                .find(|k| k.starts_with("multipart/form-data"))
        })
        .or_else(|| content.keys().next())?
        .clone();
    let schema = content[&content_type]
        .get("schema")
        .map_or_else(|| json!({}), |s| r.inline(s, at));
    Some((
        Body {
            content_type,
            required: b["required"].as_bool() == Some(true),
        },
        schema,
    ))
}

/// The schema of the first successful response with a JSON body.
fn output(r: &mut Resolver<'_>, at: &str, op: &Json) -> Option<Json> {
    let responses = op["responses"].as_object()?;
    let mut codes: Vec<&String> = responses.keys().filter(|c| c.starts_with('2')).collect();
    codes.sort();
    for code in codes {
        let Some(response) = r.deref(&responses[code], at) else {
            continue;
        };
        let Some(content) = response["content"].as_object() else {
            continue;
        };
        let media = content
            .keys()
            .find(|k| *k == "application/json" || k.ends_with("+json"))
            .or_else(|| content.keys().next())?;
        if let Some(schema) = content[media].get("schema") {
            return Some(r.inline(schema, at));
        }
    }
    None
}
