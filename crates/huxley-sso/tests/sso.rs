//! SSO sign-in against a real PostgreSQL and a mock OIDC provider served on
//! localhost. The mock signs RS256 ID tokens with a test key, checks PKCE and
//! the client secret, and lets each test choose the claims.

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
use huxley_auth::{AuthConfig, AuthService};
use huxley_core::{RequestContext, Via};
use huxley_crypto::Crypto;
use huxley_db::testing::{TestPools, ensure_root, pools, unique};
use huxley_db::{OrgScope, begin_scoped};
use huxley_sso::{MfaTrust, Preset, ProviderInput, RoleMapping, SsoError, SsoService, providers};
use openidconnect::core::{
    CoreGenderClaim, CoreJsonWebKeySet, CoreJweContentEncryptionAlgorithm, CoreJwsSigningAlgorithm,
    CoreProviderMetadata, CoreResponseType, CoreRsaPrivateSigningKey, CoreSubjectIdentifierType,
};
use openidconnect::{
    AdditionalClaims, Audience, AuthUrl, AuthenticationMethodReference,
    EmptyAdditionalProviderMetadata, EndUserEmail, IdToken, IdTokenClaims, IssuerUrl, JsonWebKeyId,
    JsonWebKeySetUrl, Nonce, PrivateSigningKey as _, ResponseTypes, StandardClaims,
    SubjectIdentifier, TokenUrl,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use uuid::Uuid;

const KEY_PEM: &str = include_str!("fixtures/test-idp-key.pem");
const CLIENT_ID: &str = "huxley-test";
const CLIENT_SECRET: &str = "s3cret-client";
const PUBLIC_URL: &str = "https://huxley.example.com";

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Extra {
    #[serde(skip_serializing_if = "Option::is_none")]
    groups: Option<Vec<String>>,
}
impl AdditionalClaims for Extra {}

/// What the mock puts in the ID token for a code.
#[derive(Debug, Clone)]
struct Identity {
    subject: String,
    email: String,
    email_verified: Option<bool>,
    groups: Option<Vec<String>>,
    amr: Vec<&'static str>,
}

#[derive(Debug, Clone)]
struct Pending {
    identity: Identity,
    nonce: String,
    challenge: String,
}

#[derive(Clone)]
struct Mock {
    issuer: String,
    codes: Arc<Mutex<HashMap<String, Pending>>>,
}

impl Mock {
    fn signing_key() -> CoreRsaPrivateSigningKey {
        CoreRsaPrivateSigningKey::from_pem(KEY_PEM, Some(JsonWebKeyId::new("test".into()))).unwrap()
    }

    /// Simulates the user approving at the provider: returns the code the
    /// provider would redirect back with.
    fn authorize(&self, authorization_url: &str, identity: Identity) -> String {
        let url = url::Url::parse(authorization_url).unwrap();
        let q: HashMap<_, _> = url.query_pairs().into_owned().collect();
        assert_eq!(q["client_id"], CLIENT_ID);
        assert_eq!(
            q["redirect_uri"],
            format!("{PUBLIC_URL}/login/oidc/callback")
        );
        assert_eq!(q["code_challenge_method"], "S256");
        assert!(q["scope"].split(' ').any(|s| s == "openid"));
        let code = Uuid::new_v4().to_string();
        self.codes.lock().unwrap().insert(
            code.clone(),
            Pending {
                identity,
                nonce: q["nonce"].clone(),
                challenge: q["code_challenge"].clone(),
            },
        );
        code
    }
}

async fn discovery(State(mock): State<Mock>) -> Json<CoreProviderMetadata> {
    let metadata = CoreProviderMetadata::new(
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
    ));
    Json(metadata)
}

async fn jwks() -> Json<CoreJsonWebKeySet> {
    Json(CoreJsonWebKeySet::new(vec![
        Mock::signing_key().as_verification_key(),
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
    // Client authentication: client_secret_basic.
    let expected = format!(
        "Basic {}",
        STANDARD.encode(format!("{CLIENT_ID}:{CLIENT_SECRET}"))
    );
    if headers.get("authorization").and_then(|v| v.to_str().ok()) != Some(expected.as_str()) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({ "error": "invalid_client" })),
        )
            .into_response();
    }
    let Some(pending) = mock.codes.lock().unwrap().remove(&form.code) else {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "invalid_grant" })),
        )
            .into_response();
    };
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(form.code_verifier.as_bytes()));
    if challenge != pending.challenge {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "invalid_grant" })),
        )
            .into_response();
    }
    let id = pending.identity;
    let now = chrono_now();
    let mut standard = StandardClaims::new(SubjectIdentifier::new(id.subject))
        .set_email(Some(EndUserEmail::new(id.email)));
    if let Some(v) = id.email_verified {
        standard = standard.set_email_verified(Some(v));
    }
    let claims = IdTokenClaims::<Extra, CoreGenderClaim>::new(
        IssuerUrl::new(mock.issuer.clone()).unwrap(),
        vec![Audience::new(CLIENT_ID.into())],
        now + chrono::Duration::minutes(5),
        now,
        standard,
        Extra { groups: id.groups },
    )
    .set_nonce(Some(Nonce::new(pending.nonce)))
    .set_auth_method_refs(Some(
        id.amr
            .iter()
            .map(|m| AuthenticationMethodReference::new((*m).into()))
            .collect(),
    ));
    let token = IdToken::<
        Extra,
        CoreGenderClaim,
        CoreJweContentEncryptionAlgorithm,
        CoreJwsSigningAlgorithm,
    >::new(
        claims,
        &Mock::signing_key(),
        CoreJwsSigningAlgorithm::RsaSsaPkcs1V15Sha256,
        None,
        None,
    )
    .unwrap();
    Json(serde_json::json!({
        "access_token": "opaque",
        "token_type": "Bearer",
        "expires_in": 300,
        "id_token": token.to_string(),
    }))
    .into_response()
}

fn chrono_now() -> chrono::DateTime<chrono::Utc> {
    chrono::Utc::now()
}

async fn start_mock() -> Mock {
    let listener = tokio::net::TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], 0)))
        .await
        .unwrap();
    let issuer = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
    let mock = Mock {
        issuer,
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

fn ctx() -> RequestContext {
    RequestContext::system(Via::Api)
}

fn crypto(pools: &TestPools) -> Crypto {
    Crypto::new(pools.app.clone(), huxley_crypto::testing::key_ring())
}

fn service(pools: &TestPools) -> SsoService {
    let auth = AuthService::new(
        pools.app.clone(),
        AuthConfig {
            public_url: Some(PUBLIC_URL.into()),
            ..AuthConfig::default()
        },
        crypto(pools),
    );
    SsoService::new(auth, crypto(pools)).unwrap()
}

struct Setup {
    pools: TestPools,
    sso: SsoService,
    mock: Mock,
    provider: providers::Provider,
    domain: String,
}

async fn setup(configure: impl FnOnce(&mut ProviderInput)) -> Option<Setup> {
    let pools = pools().await?;
    let root = ensure_root(&pools).await;
    let mock = start_mock().await;
    let domain = format!("{}.example.com", Uuid::now_v7().simple());
    let mut input = ProviderInput {
        key: format!("t-{}", &Uuid::now_v7().simple().to_string()[20..]),
        name: "Test IdP".into(),
        preset: Preset::Generic,
        issuer_url: Some(mock.issuer.clone()),
        tenant_id: None,
        client_id: CLIENT_ID.into(),
        client_secret: Some(CLIENT_SECRET.into()),
        scopes: None,
        enabled: true,
        jit_provisioning: true,
        allowed_domains: vec![domain.clone()],
        trust_email: None,
        role_claim: Some("groups".into()),
        mfa_trust: None,
    };
    configure(&mut input);
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    let provider = providers::create(&mut tx, &crypto(&pools), root, &ctx(), &input)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    Some(Setup {
        sso: service(&pools),
        pools,
        mock,
        provider,
        domain,
    })
}

impl Setup {
    fn identity(&self, local: &str) -> Identity {
        Identity {
            subject: format!("sub-{local}"),
            email: format!("{local}@{}", self.domain),
            email_verified: Some(true),
            groups: None,
            amr: vec!["pwd"],
        }
    }

    async fn sign_in(&self, identity: Identity) -> Result<huxley_sso::SignIn, SsoError> {
        let started = self
            .sso
            .start(&self.provider.key, Some("/after"))
            .await
            .unwrap();
        let code = self.mock.authorize(&started.authorization_url, identity);
        self.sso.callback(&ctx(), &started.state, &code).await
    }
}

#[tokio::test]
async fn jit_provisioning_creates_and_then_reuses_the_user() {
    let Some(s) = setup(|_| {}).await else { return };
    let first = s.sign_in(s.identity("ada")).await.unwrap();
    assert_eq!(first.next.as_deref(), Some("/after"));
    assert_eq!(first.session.user.email, format!("ada@{}", s.domain));
    assert!(first.session.token.starts_with("hxs_"));
    let again = s.sign_in(s.identity("ada")).await.unwrap();
    assert_eq!(again.session.user.id, first.session.user.id);

    // The SSO-created user has no password.
    let (has_password,): (bool,) =
        sqlx::query_as("SELECT EXISTS (SELECT 1 FROM user_passwords WHERE user_id = $1)")
            .bind(first.session.user.id)
            .fetch_one(&s.pools.owner)
            .await
            .unwrap();
    assert!(!has_password);
}

#[tokio::test]
async fn without_jit_only_existing_users_with_verified_email_can_sign_in() {
    let Some(s) = setup(|i| i.jit_provisioning = false).await else {
        return;
    };
    assert!(matches!(
        s.sign_in(s.identity("nobody")).await,
        Err(SsoError::NoAccount)
    ));

    // An existing local user is linked by verified email.
    let email = format!("grace@{}", s.domain);
    let mut tx = s.pools.app.begin().await.unwrap();
    let user = huxley_auth::create_user(&mut tx, &ctx(), &email, "Grace", "a fine long passphrase")
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let mut unverified = s.identity("grace");
    unverified.email_verified = None;
    assert!(matches!(
        s.sign_in(unverified).await,
        Err(SsoError::UnverifiedEmail)
    ));
    let signed_in = s.sign_in(s.identity("grace")).await.unwrap();
    assert_eq!(signed_in.session.user.id, user.id);

    // Once linked, the subject identifies the user even if the address changes
    // within the allowed domain.
    let mut renamed = s.identity("grace");
    renamed.email = format!("grace.hopper@{}", s.domain);
    assert_eq!(s.sign_in(renamed).await.unwrap().session.user.id, user.id);
}

#[tokio::test]
async fn other_domains_and_disabled_users_are_refused() {
    let Some(s) = setup(|_| {}).await else { return };
    let mut outsider = s.identity("eve");
    outsider.email = "eve@evil.example.org".into();
    assert!(matches!(
        s.sign_in(outsider).await,
        Err(SsoError::DomainNotAllowed)
    ));

    let user = s.sign_in(s.identity("dan")).await.unwrap().session.user;
    let mut tx = s.pools.app.begin().await.unwrap();
    huxley_auth::update_user(
        &mut tx,
        &ctx(),
        user.id,
        None,
        Some(huxley_auth::UserStatus::Disabled),
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    assert!(matches!(
        s.sign_in(s.identity("dan")).await,
        Err(SsoError::Disabled)
    ));
}

#[tokio::test]
async fn state_is_single_use_and_bound_to_its_nonce_and_verifier() {
    let Some(s) = setup(|_| {}).await else { return };
    let started = s.sso.start(&s.provider.key, None).await.unwrap();
    let code = s
        .mock
        .authorize(&started.authorization_url, s.identity("max"));
    s.sso.callback(&ctx(), &started.state, &code).await.unwrap();
    // Replaying the callback fails.
    assert!(matches!(
        s.sso.callback(&ctx(), &started.state, &code).await,
        Err(SsoError::InvalidState)
    ));
    assert!(matches!(
        s.sso.callback(&ctx(), "hxo_forged", "x").await,
        Err(SsoError::InvalidState)
    ));

    // A code issued for another sign-in (other nonce and PKCE challenge)
    // can't complete this one.
    let a = s.sso.start(&s.provider.key, None).await.unwrap();
    let b = s.sso.start(&s.provider.key, None).await.unwrap();
    let code_for_b = s.mock.authorize(&b.authorization_url, s.identity("max"));
    assert!(matches!(
        s.sso.callback(&ctx(), &a.state, &code_for_b).await,
        Err(SsoError::Provider(_))
    ));
}

#[tokio::test]
async fn a_wrong_client_secret_fails() {
    let Some(s) = setup(|i| i.client_secret = Some("wrong".into())).await else {
        return;
    };
    assert!(matches!(
        s.sign_in(s.identity("kim")).await,
        Err(SsoError::Provider(_))
    ));
}

#[tokio::test]
async fn amr_decides_whether_the_session_is_multi_factor() {
    let Some(s) = setup(|_| {}).await else { return };
    assert!(!s.sign_in(s.identity("lee")).await.unwrap().mfa);
    let mut with_mfa = s.identity("lee");
    with_mfa.amr = vec!["pwd", "mfa"];
    let signed_in = s.sign_in(with_mfa).await.unwrap();
    assert!(signed_in.mfa);
    let (mfa, method): (bool, String) =
        sqlx::query_as("SELECT mfa, auth_method FROM sessions WHERE id = $1")
            .bind(signed_in.session.session_id)
            .fetch_one(&s.pools.owner)
            .await
            .unwrap();
    assert!(mfa);
    assert_eq!(method, format!("sso:{}", s.provider.key));

    let Some(always) = setup(|i| i.mfa_trust = Some(MfaTrust::Always)).await else {
        return;
    };
    assert!(always.sign_in(always.identity("lee")).await.unwrap().mfa);
}

#[tokio::test]
async fn role_mappings_sync_on_each_sign_in_and_leave_manual_roles_alone() {
    let Some(s) = setup(|_| {}).await else { return };
    let root = ensure_root(&s.pools).await;
    let roles: Vec<(Uuid, String)> = sqlx::query_as("SELECT id, key FROM roles")
        .fetch_all(&s.pools.owner)
        .await
        .unwrap();
    let role = |k: &str| roles.iter().find(|(_, key)| key == k).unwrap().0;
    let mut tx = begin_scoped(&s.pools.app, &OrgScope::System).await.unwrap();
    let client = huxley_orgs_create(&mut tx, root).await;
    providers::set_role_mappings(
        &mut tx,
        &ctx(),
        s.provider.id,
        &[
            RoleMapping {
                claim_value: "techs".into(),
                role_id: role("technician"),
                org_id: client,
                inherit: true,
            },
            RoleMapping {
                claim_value: "auditors".into(),
                role_id: role("read_only"),
                org_id: root,
                inherit: true,
            },
        ],
    )
    .await
    .unwrap();
    // Owner can't be mapped.
    let owner = providers::set_role_mappings(
        &mut tx,
        &ctx(),
        s.provider.id,
        &[RoleMapping {
            claim_value: "x".into(),
            role_id: role("owner"),
            org_id: root,
            inherit: true,
        }],
    )
    .await;
    assert!(matches!(
        owner,
        Err(SsoError::Invalid {
            field: "roleId",
            ..
        })
    ));
    tx.commit().await.unwrap();

    let assignments = |user: Uuid| {
        let pool = s.pools.owner.clone();
        async move {
            let mut rows: Vec<(String, bool)> = sqlx::query_as(
                "SELECT r.key, a.oidc_provider_id IS NOT NULL FROM role_assignments a \
                   JOIN roles r ON r.id = a.role_id WHERE a.user_id = $1 ORDER BY r.key",
            )
            .bind(user)
            .fetch_all(&pool)
            .await
            .unwrap();
            rows.sort();
            rows
        }
    };

    let mut ident = s.identity("ivy");
    ident.groups = Some(vec!["techs".into(), "auditors".into(), "unmapped".into()]);
    let user = s.sign_in(ident.clone()).await.unwrap().session.user.id;
    assert_eq!(
        assignments(user).await,
        vec![("read_only".into(), true), ("technician".into(), true)]
    );

    // A hand-made assignment survives syncs.
    sqlx::query(
        "INSERT INTO role_assignments (id, user_id, role_id, org_id, inherit) VALUES ($1, $2, $3, $4, true)",
    )
    .bind(Uuid::now_v7())
    .bind(user)
    .bind(role("admin"))
    .bind(client)
    .execute(&s.pools.owner)
    .await
    .unwrap();

    // Leaving a group removes its role at the next sign-in.
    ident.groups = Some(vec!["auditors".into()]);
    s.sign_in(ident).await.unwrap();
    assert_eq!(
        assignments(user).await,
        vec![("admin".into(), false), ("read_only".into(), true)]
    );

    // Deleting the provider removes what it granted, and keeps the user.
    let mut tx = begin_scoped(&s.pools.app, &OrgScope::System).await.unwrap();
    providers::delete(&mut tx, &ctx(), s.provider.id)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(assignments(user).await, vec![("admin".into(), false)]);
}

async fn huxley_orgs_create(conn: &mut sqlx::PgConnection, root: Uuid) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query("INSERT INTO organizations (id, parent_id, kind, name, path) VALUES ($1, $2, 'client', $3, '')")
        .bind(id)
        .bind(root)
        .bind(unique("Sso client"))
        .execute(conn)
        .await
        .unwrap();
    id
}

#[tokio::test]
async fn the_client_secret_is_encrypted_and_write_only() {
    let Some(s) = setup(|_| {}).await else { return };
    assert!(s.provider.has_client_secret);
    let (enc,): (Vec<u8>,) =
        sqlx::query_as("SELECT client_secret_enc FROM oidc_providers WHERE id = $1")
            .bind(s.provider.id)
            .fetch_one(&s.pools.owner)
            .await
            .unwrap();
    assert!(
        !enc.windows(CLIENT_SECRET.len())
            .any(|w| w == CLIENT_SECRET.as_bytes())
    );
    let json = serde_json::to_string(&s.provider).unwrap();
    assert!(!json.contains(CLIENT_SECRET));
}
