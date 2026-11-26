//! Portal sign-in with a client's identity provider, against a real
//! PostgreSQL and a mock OIDC provider on localhost (RS256 ID tokens signed
//! with a test key; PKCE and the client secret checked).

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use axum::extract::{Form, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use base64::Engine as _;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use huxley_core::{RequestContext, Via};
use huxley_crypto::Crypto;
use huxley_db::testing::{TestPools, ensure_root, isolated};
use huxley_db::{OrgScope, begin_scoped};
use huxley_portal::accounts::{self, Status};
use huxley_portal::sso::{self, Provider};
use huxley_portal::{PortalError, Role};
use openidconnect::core::{
    CoreGenderClaim, CoreJsonWebKeySet, CoreJweContentEncryptionAlgorithm, CoreJwsSigningAlgorithm,
    CoreProviderMetadata, CoreResponseType, CoreRsaPrivateSigningKey, CoreSubjectIdentifierType,
};
use openidconnect::{
    Audience, AuthUrl, EmptyAdditionalClaims, EmptyAdditionalProviderMetadata, EndUserEmail,
    IdToken, IdTokenClaims, IssuerUrl, JsonWebKeyId, JsonWebKeySetUrl, Nonce,
    PrivateSigningKey as _, ResponseTypes, StandardClaims, SubjectIdentifier, TokenUrl,
};
use serde::Deserialize;
use serde_json::json;
use sha2::{Digest as _, Sha256};
use sqlx::PgConnection;
use uuid::Uuid;

const KEY_PEM: &str = include_str!("../../huxley-sso/tests/fixtures/test-idp-key.pem");
const CLIENT_ID: &str = "contoso-portal";
const CLIENT_SECRET: &str = "tenant-secret";
const REDIRECT: &str = "https://support.example.test/auth/sso/callback";

/// code → (subject, email, nonce, challenge)
type Codes = Arc<Mutex<HashMap<String, (String, String, String, String)>>>;

#[derive(Clone)]
struct Mock {
    issuer: String,
    codes: Codes,
}

fn signing_key() -> CoreRsaPrivateSigningKey {
    CoreRsaPrivateSigningKey::from_pem(KEY_PEM, Some(JsonWebKeyId::new("test".into()))).unwrap()
}

impl Mock {
    /// The person approving at the provider: the code it redirects back with.
    fn approve(&self, authorization_url: &str, subject: &str, email: &str) -> String {
        let url = url::Url::parse(authorization_url).unwrap();
        let q: HashMap<_, _> = url.query_pairs().into_owned().collect();
        assert_eq!(
            (q["client_id"].as_str(), q["redirect_uri"].as_str()),
            (CLIENT_ID, REDIRECT)
        );
        assert_eq!(q["code_challenge_method"], "S256");
        let code = Uuid::new_v4().to_string();
        self.codes.lock().unwrap().insert(
            code.clone(),
            (
                subject.into(),
                email.into(),
                q["nonce"].clone(),
                q["code_challenge"].clone(),
            ),
        );
        code
    }
}

async fn discovery(State(mock): State<Mock>) -> Json<CoreProviderMetadata> {
    Json(
        CoreProviderMetadata::new(
            IssuerUrl::new(mock.issuer.clone()).unwrap(),
            AuthUrl::new(format!("{}/authorize", mock.issuer)).unwrap(),
            JsonWebKeySetUrl::new(format!("{}/jwks", mock.issuer)).unwrap(),
            vec![ResponseTypes::new(vec![CoreResponseType::Code])],
            vec![CoreSubjectIdentifierType::Public],
            vec![CoreJwsSigningAlgorithm::RsaSsaPkcs1V15Sha256],
            EmptyAdditionalProviderMetadata {},
        )
        .set_token_endpoint(Some(
            TokenUrl::new(format!("{}/token", mock.issuer)).unwrap(),
        )),
    )
}

async fn jwks() -> Json<CoreJsonWebKeySet> {
    Json(CoreJsonWebKeySet::new(vec![
        signing_key().as_verification_key(),
    ]))
}

#[derive(Deserialize)]
struct TokenForm {
    code: String,
    code_verifier: String,
}

async fn token(
    State(mock): State<Mock>,
    headers: HeaderMap,
    Form(form): Form<TokenForm>,
) -> Response {
    let expected = format!(
        "Basic {}",
        STANDARD.encode(format!("{CLIENT_ID}:{CLIENT_SECRET}"))
    );
    if headers.get("authorization").and_then(|v| v.to_str().ok()) != Some(expected.as_str()) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "invalid_client" })),
        )
            .into_response();
    }
    let Some((subject, email, nonce, challenge)) = mock.codes.lock().unwrap().remove(&form.code)
    else {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "invalid_grant" })),
        )
            .into_response();
    };
    if URL_SAFE_NO_PAD.encode(Sha256::digest(form.code_verifier.as_bytes())) != challenge {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "invalid_grant" })),
        )
            .into_response();
    }
    let now = chrono::Utc::now();
    let claims = IdTokenClaims::<EmptyAdditionalClaims, CoreGenderClaim>::new(
        IssuerUrl::new(mock.issuer.clone()).unwrap(),
        vec![Audience::new(CLIENT_ID.into())],
        now + chrono::Duration::minutes(5),
        now,
        StandardClaims::new(SubjectIdentifier::new(subject))
            .set_email(Some(EndUserEmail::new(email))),
        EmptyAdditionalClaims {},
    )
    .set_nonce(Some(Nonce::new(nonce)));
    let id_token = IdToken::<
        EmptyAdditionalClaims,
        CoreGenderClaim,
        CoreJweContentEncryptionAlgorithm,
        CoreJwsSigningAlgorithm,
    >::new(
        claims,
        &signing_key(),
        CoreJwsSigningAlgorithm::RsaSsaPkcs1V15Sha256,
        None,
        None,
    )
    .unwrap();
    Json(json!({ "access_token": "opaque", "token_type": "Bearer", "expires_in": 300, "id_token": id_token.to_string() }))
        .into_response()
}

async fn start_mock() -> Mock {
    let listener = tokio::net::TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], 0)))
        .await
        .unwrap();
    let mock = Mock {
        issuer: format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port()),
        codes: Arc::default(),
    };
    let app = Router::new()
        .route("/.well-known/openid-configuration", get(discovery))
        .route("/jwks", get(jwks))
        .route("/token", post(token))
        .with_state(mock.clone());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    mock
}

fn crypto(pools: &TestPools) -> Crypto {
    Crypto::new(pools.app.clone(), huxley_crypto::testing::key_ring())
}

async fn contact(tx: &mut PgConnection, org: Uuid, first: &str, email: &str) -> Uuid {
    let ctx = RequestContext::system(Via::System);
    huxley_crm::contacts::create(
        tx,
        &ctx,
        org,
        &serde_json::from_value(
            json!({ "firstName": first, "lastName": "Client", "email": email }),
        )
        .unwrap(),
    )
    .await
    .unwrap()
    .id
}

struct World {
    pools: TestPools,
    sso: huxley_sso::SsoService,
    mock: Mock,
    provider: Provider,
    acme: Uuid,
    globex: Uuid,
}

async fn world(pools: &TestPools) -> World {
    let root = ensure_root(pools).await;
    let ctx = RequestContext::system(Via::System);
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    let org = async |tx: &mut PgConnection, name: &str| {
        huxley_orgs::create(tx, &ctx, root, huxley_orgs::OrgKind::Client, name)
            .await
            .unwrap()
            .id
    };
    let acme = org(&mut tx, "Acme").await;
    let globex = org(&mut tx, "Globex").await;
    tx.commit().await.unwrap();
    let mock = start_mock().await;
    let crypto = crypto(pools);
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    let provider = sso::create(
        &mut tx,
        &crypto,
        &ctx,
        acme,
        &serde_json::from_value(json!({
            "name": "Acme (Microsoft)", "issuerUrl": mock.issuer, "clientId": CLIENT_ID,
            "clientSecret": CLIENT_SECRET, "emailDomains": ["Acme.example"],
        }))
        .unwrap(),
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    let auth = huxley_auth::AuthService::new(
        pools.app.clone(),
        huxley_auth::AuthConfig::default(),
        crypto.clone(),
    );
    let sso = huxley_sso::SsoService::new(auth, crypto).unwrap();
    World {
        pools: pools.clone(),
        sso,
        mock,
        provider,
        acme,
        globex,
    }
}

impl World {
    /// Signs in through the mock as `subject`/`email`.
    async fn sign_in(
        &self,
        subject: &str,
        email: &str,
    ) -> Result<huxley_portal::signin::NewSession, PortalError> {
        let ctx = RequestContext::system(Via::System);
        let mut tx = begin_scoped(&self.pools.app, &OrgScope::System)
            .await
            .unwrap();
        let provider = sso::for_email(&mut tx, email)
            .await
            .unwrap()
            .expect("a provider for the domain");
        let (url, state) = sso::start(&mut tx, &self.sso, &provider, REDIRECT)
            .await
            .unwrap();
        tx.commit().await.unwrap();
        let code = self.mock.approve(&url, subject, email);
        let mut tx = begin_scoped(&self.pools.app, &OrgScope::System)
            .await
            .unwrap();
        let result = sso::callback(
            &mut tx,
            &self.sso,
            &crypto(&self.pools),
            &ctx,
            &state,
            &code,
            None,
            None,
        )
        .await;
        tx.commit().await.unwrap();
        result
    }
}

#[tokio::test]
async fn contacts_sign_in_with_their_companys_provider() {
    let Some(db) = isolated("portal_sso").await else {
        return;
    };
    let w = world(&db.pools).await;
    let ctx = RequestContext::system(Via::System);
    assert!(w.provider.has_secret);
    assert_eq!(w.provider.email_domains, ["acme.example"]);
    let mut tx = begin_scoped(&w.pools.app, &OrgScope::System).await.unwrap();
    let ada = contact(&mut tx, w.acme, "Ada", "ada@acme.example").await;
    let account = accounts::invite(&mut tx, &ctx, ada, Role::ClientAdmin)
        .await
        .unwrap();
    contact(&mut tx, w.acme, "Bea", "bea@acme.example").await;
    contact(&mut tx, w.globex, "Gil", "gil@acme.example").await;
    tx.commit().await.unwrap();

    // Ada has access: signed in, activated, the identity linked.
    let session = w.sign_in("entra-ada", "ada@acme.example").await.unwrap();
    assert_eq!(
        (session.principal.account_id, session.principal.role),
        (account.id, Role::ClientAdmin)
    );
    let mut tx = begin_scoped(&w.pools.app, &OrgScope::System).await.unwrap();
    assert_eq!(
        accounts::get(&mut tx, account.id).await.unwrap().status,
        Status::Active
    );
    tx.commit().await.unwrap();
    // Later, the link is used whatever the address says.
    let again = w
        .sign_in("entra-ada", "ada.lovelace@acme.example")
        .await
        .unwrap();
    assert_eq!(again.principal.account_id, account.id);

    // Bea has no access, and the provider doesn't provision: refused.
    let refused = w.sign_in("entra-bea", "bea@acme.example").await;
    assert!(
        matches!(refused, Err(PortalError::SignIn("no_account"))),
        "{refused:?}"
    );

    provisioning_and_boundaries(&w).await;
}

/// With provisioning, a contact under the provider's company gets an account;
/// a contact elsewhere doesn't; states are single use; domains are one
/// provider's.
async fn provisioning_and_boundaries(w: &World) {
    let ctx = RequestContext::system(Via::System);
    let crypto = crypto(&w.pools);
    let mut tx = begin_scoped(&w.pools.app, &OrgScope::System).await.unwrap();
    let mut input: sso::ProviderInput = serde_json::from_value(json!({
        "name": "Acme (Microsoft)", "issuerUrl": w.mock.issuer, "clientId": CLIENT_ID,
        "emailDomains": ["acme.example"], "provision": true, "defaultRole": "client_approver",
        "version": w.provider.version,
    }))
    .unwrap();
    let updated = sso::update(&mut tx, &crypto, &ctx, w.provider.id, &input)
        .await
        .unwrap();
    assert!(updated.has_secret, "a left-out secret is kept");
    tx.commit().await.unwrap();
    let bea = w.sign_in("entra-bea", "bea@acme.example").await.unwrap();
    assert_eq!(bea.principal.role, Role::ClientApprover);
    assert_eq!(bea.principal.org_id, w.acme);
    // Gil is Globex's contact: not under Acme, so no account.
    let gil = w.sign_in("entra-gil", "gil@acme.example").await;
    assert!(
        matches!(gil, Err(PortalError::SignIn("no_account"))),
        "{gil:?}"
    );

    // A state works once.
    let mut tx = begin_scoped(&w.pools.app, &OrgScope::System).await.unwrap();
    let (url, state) = sso::start(&mut tx, &w.sso, &updated, REDIRECT)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let code = w.mock.approve(&url, "entra-bea", "bea@acme.example");
    for expect_ok in [true, false] {
        let mut tx = begin_scoped(&w.pools.app, &OrgScope::System).await.unwrap();
        let r = sso::callback(&mut tx, &w.sso, &crypto, &ctx, &state, &code, None, None).await;
        tx.commit().await.unwrap();
        assert_eq!(r.is_ok(), expect_ok, "{r:?}");
    }

    // Another provider can't take acme.example.
    let mut tx = begin_scoped(&w.pools.app, &OrgScope::System).await.unwrap();
    input.version = None;
    let clash = sso::create(&mut tx, &crypto, &ctx, w.globex, &input).await;
    assert!(matches!(clash, Err(PortalError::Conflict(_))), "{clash:?}");
}
