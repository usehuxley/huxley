//! Passkeys against a real PostgreSQL, driven by 1Password's software
//! authenticator (an independent WebAuthn implementation), so these tests also
//! check that webauthn-rs interoperates with a real client.

use huxley_auth::{
    AuthConfig, AuthError, AuthService, LoginOutcome, MfaMethod, PublicKeyCredential,
    RegisterPublicKeyCredential, SecondFactor, User, create_user, mfa,
};
use huxley_core::{RequestContext, Via};
use huxley_crypto::Crypto;
use huxley_db::testing::{TestPools, ensure_root, pools, unique};
use passkey::authenticator::{
    Authenticator, CredentialStore, DiscoverabilitySupport, StoreInfo, UiHint, UserCheck,
    UserValidationMethod,
};
use passkey::client::{Client, DefaultClientData};
use passkey::crypto::AvailableBackend;
use passkey::types::Passkey as SoftPasskey;
use passkey::types::ctap2::make_credential::{
    Options, PublicKeyCredentialRpEntity, PublicKeyCredentialUserEntity,
};
use passkey::types::ctap2::{Aaguid, Ctap2Error, StatusCode};
use passkey::types::webauthn::PublicKeyCredentialDescriptor;
use passkey::types::webauthn::{CredentialCreationOptions, CredentialRequestOptions};
use serde_json::Value;
use url::Url;
use uuid::Uuid;

const PASSWORD: &str = "a perfectly fine passphrase";
const ORIGIN: &str = "https://huxley.example.com";

/// A user who always approves, with verification (like a fingerprint).
struct AlwaysVerify;

#[async_trait::async_trait]
impl UserValidationMethod for AlwaysVerify {
    type PasskeyItem = SoftPasskey;

    async fn check_user<'a>(
        &self,
        _hint: UiHint<'a, SoftPasskey>,
        _presence: bool,
        _verification: bool,
    ) -> Result<UserCheck, Ctap2Error> {
        Ok(UserCheck {
            presence: true,
            verification: true,
        })
    }

    fn is_presence_enabled(&self) -> bool {
        true
    }

    fn is_verification_enabled(&self) -> Option<bool> {
        Some(true)
    }
}

/// Passkeys on a device. Unlike the library's `MemoryStore`, it can find
/// credentials without an allow-list, as passwordless sign-in needs.
#[derive(Default)]
struct Keychain(Vec<SoftPasskey>);

#[async_trait::async_trait]
impl CredentialStore for Keychain {
    type PasskeyItem = SoftPasskey;

    async fn find_credentials(
        &self,
        ids: Option<&[PublicKeyCredentialDescriptor]>,
        rp_id: &str,
        _user_handle: Option<&[u8]>,
    ) -> Result<Vec<SoftPasskey>, StatusCode> {
        let found: Vec<SoftPasskey> = self
            .0
            .iter()
            .filter(|p| p.rp_id == rp_id)
            .filter(|p| match ids {
                Some(ids) if !ids.is_empty() => ids.iter().any(|d| *d.id == *p.credential_id),
                _ => true,
            })
            .cloned()
            .collect();
        if found.is_empty() {
            Err(Ctap2Error::NoCredentials.into())
        } else {
            Ok(found)
        }
    }

    async fn save_credential(
        &mut self,
        cred: SoftPasskey,
        _user: PublicKeyCredentialUserEntity,
        _rp: PublicKeyCredentialRpEntity,
        _options: Options,
    ) -> Result<(), StatusCode> {
        self.0.push(cred);
        Ok(())
    }

    async fn update_credential(&mut self, cred: &SoftPasskey) -> Result<(), StatusCode> {
        for existing in &mut self.0 {
            if existing.credential_id == cred.credential_id {
                *existing = cred.clone();
            }
        }
        Ok(())
    }

    async fn get_info(&self) -> StoreInfo {
        StoreInfo {
            discoverability: DiscoverabilitySupport::Full,
        }
    }
}

/// A software device holding passkeys, as a browser would use it.
struct Device {
    client: Client<Keychain, AlwaysVerify, AvailableBackend, public_suffix::PublicSuffixList, ()>,
    origin: Url,
}

impl Device {
    fn new() -> Self {
        Self::at(ORIGIN)
    }

    fn at(origin: &str) -> Self {
        let authenticator = Authenticator::new(
            Aaguid([7; 16]),
            Keychain::default(),
            AlwaysVerify,
            AvailableBackend,
        );
        Self {
            client: Client::new(authenticator),
            origin: Url::parse(origin).unwrap(),
        }
    }

    /// Answers registration options from the server.
    async fn register(&mut self, options: &Value) -> RegisterPublicKeyCredential {
        let request: CredentialCreationOptions = serde_json::from_value(options.clone()).unwrap();
        let created = self
            .client
            .register(&self.origin, request, DefaultClientData)
            .await
            .unwrap();
        serde_json::from_value(browser_json(serde_json::to_value(created).unwrap())).unwrap()
    }

    /// Answers authentication options from the server.
    async fn sign(&mut self, options: &Value) -> PublicKeyCredential {
        let request: CredentialRequestOptions = serde_json::from_value(options.clone()).unwrap();
        let signed = self
            .client
            .authenticate(&self.origin, request, DefaultClientData)
            .await
            .unwrap();
        serde_json::from_value(browser_json(serde_json::to_value(signed).unwrap())).unwrap()
    }
}

/// passkey-types serializes binary fields as arrays of numbers; browsers'
/// `PublicKeyCredential.toJSON()` uses base64url strings. Convert, so the
/// server sees what a browser would send.
fn browser_json(value: Value) -> Value {
    use base64::Engine as _;
    match value {
        Value::Array(items) if !items.is_empty() && items.iter().all(Value::is_u64) => {
            let bytes: Vec<u8> = items
                .iter()
                .map(|v| u8::try_from(v.as_u64().unwrap()).unwrap())
                .collect();
            Value::String(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes))
        }
        Value::Array(items) => Value::Array(items.into_iter().map(browser_json).collect()),
        Value::Object(map) => {
            Value::Object(map.into_iter().map(|(k, v)| (k, browser_json(v))).collect())
        }
        other => other,
    }
}

fn ctx() -> RequestContext {
    RequestContext::system(Via::Api)
}

fn service(pools: &TestPools) -> AuthService {
    AuthService::new(
        pools.app.clone(),
        AuthConfig {
            public_url: Some(ORIGIN.to_owned()),
            ..AuthConfig::default()
        },
        Crypto::new(pools.app.clone(), huxley_crypto::testing::key_ring()),
    )
}

async fn user(pools: &TestPools) -> User {
    ensure_root(pools).await;
    let email = format!("{}@example.com", unique("pk").replace(' ', "."));
    let mut tx = pools.app.begin().await.unwrap();
    let user = create_user(&mut tx, &ctx(), &email, "Pat Passkey", PASSWORD)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    user
}

/// Registers a passkey on `device`. Returns the recovery codes, if any.
async fn add_passkey(
    auth: &AuthService,
    user: &User,
    device: &mut Device,
    name: &str,
) -> Option<Vec<String>> {
    let ceremony = auth
        .begin_passkey_registration(&ctx(), user, PASSWORD)
        .await
        .unwrap();
    let credential = device.register(&ceremony.options).await;
    auth.finish_passkey_registration(&ctx(), user.id, ceremony.id, name, &credential)
        .await
        .unwrap()
        .recovery_codes
}

async fn passwordless(auth: &AuthService, device: &mut Device) -> Result<Uuid, AuthError> {
    let ceremony = auth.begin_passkey_login().await.unwrap();
    let credential = device.sign(&ceremony.options).await;
    auth.finish_passkey_login(&ctx(), ceremony.id, &credential)
        .await
        .map(|s| s.user.id)
}

#[tokio::test]
async fn register_then_sign_in_without_a_password() {
    let Some(pools) = pools().await else { return };
    let auth = service(&pools);
    let user = user(&pools).await;
    let mut device = Device::new();

    // Adding a passkey re-checks the password.
    assert!(matches!(
        auth.begin_passkey_registration(&ctx(), &user, "wrong password!!")
            .await,
        Err(AuthError::WrongCurrentPassword)
    ));
    let ceremony = auth
        .begin_passkey_registration(&ctx(), &user, PASSWORD)
        .await
        .unwrap();
    assert_eq!(
        ceremony.options["publicKey"]["authenticatorSelection"]["residentKey"],
        "required"
    );
    assert_eq!(
        ceremony.options["publicKey"]["rp"]["id"],
        "huxley.example.com"
    );
    let credential = device.register(&ceremony.options).await;

    // The ceremony belongs to this user and works once.
    let stranger = self::user(&pools).await;
    assert!(matches!(
        auth.finish_passkey_registration(&ctx(), stranger.id, ceremony.id, "Laptop", &credential)
            .await,
        Err(AuthError::InvalidCeremony)
    ));
    let registered = auth
        .finish_passkey_registration(&ctx(), user.id, ceremony.id, " Laptop ", &credential)
        .await
        .unwrap();
    assert_eq!(registered.passkey.name, "Laptop");
    // The first second factor comes with recovery codes.
    assert_eq!(registered.recovery_codes.as_ref().map(Vec::len), Some(10));
    assert!(matches!(
        auth.finish_passkey_registration(&ctx(), user.id, ceremony.id, "Laptop", &credential)
            .await,
        Err(AuthError::InvalidCeremony)
    ));

    let status = auth.mfa_status(user.id).await.unwrap();
    assert_eq!((status.totp_enabled, status.passkeys), (false, 1));

    // Passwordless sign-in finds the user from the credential.
    assert_eq!(passwordless(&auth, &mut device).await.unwrap(), user.id);
    let listed = auth.list_passkeys(user.id).await.unwrap();
    assert!(listed[0].last_used_at.is_some());

    // A second passkey (another device) doesn't issue new recovery codes.
    let mut phone = Device::new();
    assert!(
        add_passkey(&auth, &user, &mut phone, "Phone")
            .await
            .is_none()
    );
    assert_eq!(auth.list_passkeys(user.id).await.unwrap().len(), 2);
}

#[tokio::test]
async fn passwordless_sign_in_rejects_unknown_wrong_origin_and_disabled() {
    let Some(pools) = pools().await else { return };
    let auth = service(&pools);
    let user = user(&pools).await;
    let mut device = Device::new();
    add_passkey(&auth, &user, &mut device, "Laptop").await;

    // A device with no passkey for this site can't answer at all; one whose
    // passkey was removed is rejected.
    let id = auth.list_passkeys(user.id).await.unwrap()[0].id;
    auth.delete_passkey(&ctx(), user.id, id).await.unwrap();
    assert!(matches!(
        passwordless(&auth, &mut device).await,
        Err(AuthError::InvalidPasskey)
    ));
    // Removing the last factor removed the recovery codes too.
    assert_eq!(
        auth.mfa_status(user.id)
            .await
            .unwrap()
            .recovery_codes_remaining,
        0
    );

    // A disabled user can't sign in with a passkey.
    let mut device = Device::new();
    add_passkey(&auth, &user, &mut device, "Laptop").await;
    let mut tx = pools.app.begin().await.unwrap();
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
        passwordless(&auth, &mut device).await,
        Err(AuthError::InvalidPasskey)
    ));

    // A login ceremony can't be replayed.
    let ceremony = auth.begin_passkey_login().await.unwrap();
    let credential = device.sign(&ceremony.options).await;
    let _ = auth
        .finish_passkey_login(&ctx(), ceremony.id, &credential)
        .await;
    assert!(matches!(
        auth.finish_passkey_login(&ctx(), ceremony.id, &credential)
            .await,
        Err(AuthError::InvalidPasskey)
    ));
}

#[tokio::test]
async fn an_assertion_from_another_origin_is_rejected() {
    let Some(pools) = pools().await else { return };
    let auth = service(&pools);
    let user = user(&pools).await;
    let mut device = Device::new();
    add_passkey(&auth, &user, &mut device, "Laptop").await;

    // A page on a subdomain may legitimately ask for the parent domain's
    // passkey (browsers allow it), and the browser records that page's origin
    // in the signed client data. Huxley accepts only its own origin, so a
    // compromised or user-content subdomain can't sign users in.
    device.origin = Url::parse("https://evil.huxley.example.com").unwrap();
    assert!(matches!(
        passwordless(&auth, &mut device).await,
        Err(AuthError::InvalidPasskey)
    ));

    device.origin = Url::parse(ORIGIN).unwrap();
    assert_eq!(passwordless(&auth, &mut device).await.unwrap(), user.id);
}

#[tokio::test]
async fn passkey_as_a_second_factor_after_the_password() {
    let Some(pools) = pools().await else { return };
    let auth = service(&pools);
    let user = user(&pools).await;
    let mut device = Device::new();
    add_passkey(&auth, &user, &mut device, "Laptop").await;

    let LoginOutcome::MfaRequired(challenge) =
        auth.login(&ctx(), &user.email, PASSWORD).await.unwrap()
    else {
        panic!("expected an MFA challenge");
    };
    assert_eq!(
        challenge.methods,
        vec![MfaMethod::Passkey, MfaMethod::RecoveryCode]
    );

    let ceremony = auth.begin_mfa_passkey(&challenge.token).await.unwrap();
    // The options list this user's credential.
    assert_eq!(
        ceremony.options["publicKey"]["allowCredentials"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let credential = device.sign(&ceremony.options).await;
    let factor = SecondFactor::Passkey {
        ceremony: ceremony.id,
        credential: &credential,
    };
    let login = auth
        .complete_mfa(&ctx(), &challenge.token, factor)
        .await
        .unwrap();
    assert_eq!(login.user.id, user.id);

    // A ceremony from one challenge can't answer another.
    let LoginOutcome::MfaRequired(first) = auth.login(&ctx(), &user.email, PASSWORD).await.unwrap()
    else {
        panic!()
    };
    let LoginOutcome::MfaRequired(second) =
        auth.login(&ctx(), &user.email, PASSWORD).await.unwrap()
    else {
        panic!()
    };
    let ceremony = auth.begin_mfa_passkey(&first.token).await.unwrap();
    let credential = device.sign(&ceremony.options).await;
    let factor = SecondFactor::Passkey {
        ceremony: ceremony.id,
        credential: &credential,
    };
    assert!(matches!(
        auth.complete_mfa(&ctx(), &second.token, factor).await,
        Err(AuthError::InvalidMfaCode)
    ));

    // Admin reset removes passkeys too.
    let mut tx = pools.app.begin().await.unwrap();
    assert!(mfa::reset_mfa(&mut tx, &ctx(), user.id).await.unwrap());
    tx.commit().await.unwrap();
    assert!(auth.list_passkeys(user.id).await.unwrap().is_empty());
    assert!(matches!(
        auth.login(&ctx(), &user.email, PASSWORD).await.unwrap(),
        LoginOutcome::SignedIn(_)
    ));
}

#[tokio::test]
async fn passkeys_need_a_public_url() {
    let Some(pools) = pools().await else { return };
    let auth = AuthService::new(
        pools.app.clone(),
        AuthConfig::default(),
        Crypto::new(pools.app.clone(), huxley_crypto::testing::key_ring()),
    );
    assert!(!auth.passkeys_enabled());
    assert!(matches!(
        auth.begin_passkey_login().await,
        Err(AuthError::PasskeysNotConfigured)
    ));
    assert!(
        AuthConfig {
            public_url: Some("not a url".into()),
            ..AuthConfig::default()
        }
        .validate()
        .is_err()
    );
}
