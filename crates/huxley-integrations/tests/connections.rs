//! Connections against a real PostgreSQL.

use std::collections::BTreeMap;

use huxley_core::{RequestContext, Via};
use huxley_crypto::Crypto;
use huxley_db::testing::{TestPools, ensure_root, pools};
use huxley_db::{OrgScope, begin_scoped};
use huxley_integrations::connections::{
    self, ConnectionError, ConnectionInput, ConnectionStatus, ResolvedAuth,
};
use huxley_integrations::store;
use serde_json::{Map, Value, json};
use uuid::Uuid;

const SPEC: &str = r#"
openapi: 3.0.3
info: { title: Tenant API, version: "1" }
servers:
  - url: "https://{tenant}.api.example.com/v2/"
    variables: { tenant: { default: demo } }
components:
  securitySchemes:
    basic: { type: http, scheme: basic }
    key: { type: apiKey, in: query, name: api_key }
    oauth:
      type: oauth2
      flows:
        authorizationCode:
          authorizationUrl: https://login.example.com/authorize
          tokenUrl: https://login.example.com/token
          scopes: { read: Read }
paths:
  /things:
    get: { responses: { "200": { description: ok } } }
"#;

async fn setup(pools: &TestPools) -> (Crypto, Uuid, Uuid) {
    let crypto = Crypto::new(pools.app.clone(), huxley_crypto::testing::key_ring());
    let org = ensure_root(pools).await;
    let ctx = RequestContext::system(Via::System);
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    let key = format!("tenant-{}", &Uuid::now_v7().simple().to_string()[20..]);
    let integration = store::create(&mut tx, &ctx, org, Some(&key), SPEC)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    (crypto, org, integration.summary.id)
}

fn settings(value: Value) -> Map<String, Value> {
    match value {
        Value::Object(map) => map,
        other => panic!("not an object: {other}"),
    }
}

fn secrets(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
        .collect()
}

#[tokio::test]
async fn credentials_are_encrypted_resolved_and_kept_on_update() {
    let Some(pools) = pools().await else { return };
    let (crypto, org, integration) = setup(&pools).await;
    let ctx = RequestContext::system(Via::System);
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    let input = ConnectionInput {
        name: "Contoso tenant".into(),
        auth_scheme: Some("basic".into()),
        server_variables: settings(json!({ "tenant": "contoso" })),
        settings: settings(json!({ "username": "svc" })),
        secrets: secrets(&[("password", "hunter2-very-secret")]),
        ..ConnectionInput::default()
    };
    let view = connections::create(&mut tx, &crypto, &ctx, org, integration, &input)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert!(view.key.starts_with("contoso-tenant"));
    assert_eq!(view.secret_fields, vec!["password"]);
    assert_eq!(view.status, ConnectionStatus::Ready);
    // The view never carries secrets, and the row doesn't hold them in clear.
    assert!(!serde_json::to_string(&view).unwrap().contains("hunter2"));
    let (sealed,): (Vec<u8>,) =
        sqlx::query_as("SELECT secrets_sealed FROM connections WHERE id = $1")
            .bind(view.id)
            .fetch_one(&pools.owner)
            .await
            .unwrap();
    assert!(!String::from_utf8_lossy(&sealed).contains("hunter2"));

    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    let resolved = connections::resolve(&mut tx, &crypto, view.id)
        .await
        .unwrap();
    assert_eq!(resolved.base_url, "https://contoso.api.example.com/v2");
    assert_eq!(
        resolved.auth,
        ResolvedAuth::Basic {
            username: "svc".into(),
            password: "hunter2-very-secret".into()
        }
    );

    // Renamed, with no secrets given: the password stays.
    let renamed = ConnectionInput {
        name: "Contoso".into(),
        secrets: BTreeMap::new(),
        base_url: Some("https://eu.api.example.com/v2/".into()),
        ..input.clone()
    };
    let updated = connections::update(&mut tx, &crypto, &ctx, view.id, &renamed)
        .await
        .unwrap();
    assert_eq!(updated.name, "Contoso");
    let resolved = connections::resolve(&mut tx, &crypto, view.id)
        .await
        .unwrap();
    assert_eq!(resolved.base_url, "https://eu.api.example.com/v2");
    assert!(
        matches!(resolved.auth, ResolvedAuth::Basic { password, .. } if password == "hunter2-very-secret")
    );

    // Switching to the API key scheme needs the key, and drops the password.
    let switch = ConnectionInput {
        auth_scheme: Some("key".into()),
        settings: Map::new(),
        ..renamed.clone()
    };
    let err = connections::update(&mut tx, &crypto, &ctx, view.id, &switch)
        .await
        .unwrap_err();
    assert!(matches!(err, ConnectionError::Invalid(field, _) if field == "secrets.key"));
    let switch = ConnectionInput {
        secrets: secrets(&[("key", "k-123")]),
        ..switch
    };
    let switched = connections::update(&mut tx, &crypto, &ctx, view.id, &switch)
        .await
        .unwrap();
    assert_eq!(switched.secret_fields, vec!["key"]);
    let resolved = connections::resolve(&mut tx, &crypto, view.id)
        .await
        .unwrap();
    assert!(
        matches!(resolved.auth, ResolvedAuth::ApiKey { value, name, .. } if value == "k-123" && name == "api_key")
    );

    connections::delete(&mut tx, &ctx, view.id).await.unwrap();
    assert!(matches!(
        connections::get(&mut tx, view.id).await,
        Err(ConnectionError::NotFound)
    ));
    tx.commit().await.unwrap();
}

#[tokio::test]
async fn inputs_are_checked_against_the_integration() {
    let Some(pools) = pools().await else { return };
    let (crypto, org, integration) = setup(&pools).await;
    let ctx = RequestContext::system(Via::System);
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    let base = ConnectionInput {
        name: "C".into(),
        auth_scheme: Some("basic".into()),
        settings: settings(json!({ "username": "u" })),
        secrets: secrets(&[("password", "p")]),
        ..ConnectionInput::default()
    };
    let field_of = |e: ConnectionError| match e {
        ConnectionError::Invalid(field, _) => field,
        other => panic!("{other:?}"),
    };
    for (input, field) in [
        (
            ConnectionInput {
                auth_scheme: Some("nope".into()),
                ..base.clone()
            },
            "authScheme",
        ),
        (
            ConnectionInput {
                secrets: BTreeMap::new(),
                ..base.clone()
            },
            "secrets.password",
        ),
        (
            ConnectionInput {
                settings: settings(json!({ "username": "u", "extra": 1 })),
                ..base.clone()
            },
            "settings.extra",
        ),
        (
            ConnectionInput {
                base_url: Some("ftp://x".into()),
                ..base.clone()
            },
            "baseUrl",
        ),
        (
            ConnectionInput {
                key: Some("Bad Key".into()),
                ..base.clone()
            },
            "key",
        ),
    ] {
        let err = connections::create(&mut tx, &crypto, &ctx, org, integration, &input)
            .await
            .unwrap_err();
        assert_eq!(field_of(err), field);
    }

    // Sign-in schemes start out waiting for someone to sign in.
    let oauth = ConnectionInput {
        name: "Signed in".into(),
        // The root org is shared between runs: keep the key unique.
        key: Some(format!(
            "signed-in-{}",
            &Uuid::now_v7().simple().to_string()[20..]
        )),
        auth_scheme: Some("oauth".into()),
        settings: settings(json!({ "client_id": "app", "scopes": ["read"] })),
        secrets: secrets(&[("client_secret", "s")]),
        ..ConnectionInput::default()
    };
    let view = connections::create(&mut tx, &crypto, &ctx, org, integration, &oauth)
        .await
        .unwrap();
    assert_eq!(view.status, ConnectionStatus::NeedsAuth);
    let listed = connections::list(&mut tx, Some(integration)).await.unwrap();
    assert_eq!(listed.len(), 1);
    tx.commit().await.unwrap();
}
