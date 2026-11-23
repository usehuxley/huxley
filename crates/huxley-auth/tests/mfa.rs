//! TOTP MFA, recovery codes, and challenges against a real PostgreSQL.

use huxley_auth::totp::{code_at_step, step_at};
use huxley_auth::{
    AuthConfig, AuthError, AuthService, LoginOutcome, MfaChallenge, SecondFactor, create_user, mfa,
};
use huxley_core::{RequestContext, Via};
use huxley_crypto::Crypto;
use huxley_db::testing::{TestPools, ensure_root, pools, unique};
use time::OffsetDateTime;
use uuid::Uuid;

const PASSWORD: &str = "a perfectly fine passphrase";

fn ctx() -> RequestContext {
    RequestContext::system(Via::Api)
}

fn service_with(pools: &TestPools, config: AuthConfig) -> AuthService {
    AuthService::new(
        pools.app.clone(),
        config,
        Crypto::new(pools.app.clone(), huxley_crypto::testing::key_ring()),
    )
}

fn service(pools: &TestPools) -> AuthService {
    service_with(pools, AuthConfig::default())
}

async fn user(pools: &TestPools) -> (Uuid, String) {
    ensure_root(pools).await;
    let email = format!("{}@example.com", unique("mfa").replace(' ', "."));
    let mut tx = pools.app.begin().await.unwrap();
    let user = create_user(&mut tx, &ctx(), &email, "Mfa User", PASSWORD)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    (user.id, email)
}

fn base32_decode(s: &str) -> Vec<u8> {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
    let (mut buffer, mut bits, mut out) = (0u32, 0u32, Vec::new());
    for c in s.bytes() {
        let value = u32::try_from(ALPHABET.iter().position(|&a| a == c).unwrap()).unwrap();
        buffer = (buffer << 5) | value;
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            out.push(u8::try_from((buffer >> bits) & 0xff).unwrap());
        }
    }
    out
}

fn now_step() -> i64 {
    step_at(OffsetDateTime::now_utc().unix_timestamp())
}

/// A user with a confirmed authenticator. Returns (user id, email, secret,
/// recovery codes, session kept during setup). The current step's code has
/// been used by the confirmation.
async fn enrolled(
    pools: &TestPools,
    auth: &AuthService,
) -> (Uuid, String, Vec<u8>, Vec<String>, Uuid) {
    let (id, email) = user(pools).await;
    let session = signed_in(auth, &email).await;
    let enrollment = auth.begin_totp(&ctx(), id, &email, PASSWORD).await.unwrap();
    let secret = base32_decode(&enrollment.secret);
    let codes = auth
        .confirm_totp(&ctx(), id, session, &code_at_step(&secret, now_step()))
        .await
        .unwrap();
    (id, email, secret, codes, session)
}

async fn signed_in(auth: &AuthService, email: &str) -> Uuid {
    auth.login(&ctx(), email, PASSWORD)
        .await
        .unwrap()
        .into_session()
        .expect("no MFA yet")
        .session_id
}

async fn challenge(auth: &AuthService, email: &str) -> MfaChallenge {
    match auth.login(&ctx(), email, PASSWORD).await.unwrap() {
        LoginOutcome::MfaRequired(challenge) => challenge,
        LoginOutcome::SignedIn(_) => panic!("expected an MFA challenge"),
    }
}

#[tokio::test]
async fn enrollment_then_sign_in_with_a_code() {
    let Some(pools) = pools().await else { return };
    let auth = service(&pools);
    let (id, email) = user(&pools).await;
    let kept = signed_in(&auth, &email).await;
    let other = signed_in(&auth, &email).await;

    // Setup re-checks the password.
    assert!(matches!(
        auth.begin_totp(&ctx(), id, &email, "wrong password!!")
            .await,
        Err(AuthError::WrongCurrentPassword)
    ));
    let enrollment = auth.begin_totp(&ctx(), id, &email, PASSWORD).await.unwrap();
    assert!(enrollment.otpauth_uri.starts_with("otpauth://totp/Huxley:"));
    assert!(enrollment.otpauth_uri.contains(&enrollment.secret));
    let secret = base32_decode(&enrollment.secret);
    assert_eq!(secret.len(), 20);

    // The secret is stored encrypted.
    let (stored,): (Vec<u8>,) =
        sqlx::query_as("SELECT secret_enc FROM user_totp WHERE user_id = $1")
            .bind(id)
            .fetch_one(&pools.owner)
            .await
            .unwrap();
    assert!(!stored.windows(secret.len()).any(|w| w == secret.as_slice()));

    // Not required at sign-in until confirmed.
    signed_in(&auth, &email).await;
    assert!(matches!(
        auth.confirm_totp(&ctx(), id, kept, "000000").await,
        Err(AuthError::InvalidMfaCode)
    ));
    let step = now_step();
    let codes = auth
        .confirm_totp(&ctx(), id, kept, &code_at_step(&secret, step))
        .await
        .unwrap();
    assert_eq!(codes.len(), 10);
    let status = auth.mfa_status(id).await.unwrap();
    assert!(status.totp_enabled);
    assert_eq!(status.recovery_codes_remaining, 10);

    // Confirming signed out the other sessions but kept this one.
    let sessions: Vec<Uuid> = auth
        .list_sessions(id)
        .await
        .unwrap()
        .iter()
        .map(|s| s.id)
        .collect();
    assert!(sessions.contains(&kept));
    assert!(!sessions.contains(&other));

    // Setting up again is refused.
    assert!(matches!(
        auth.begin_totp(&ctx(), id, &email, PASSWORD).await,
        Err(AuthError::MfaAlreadyEnabled)
    ));

    // Sign-in now needs a code. The confirmation's code can't be replayed.
    let c = challenge(&auth, &email).await;
    assert!(c.token.starts_with("hxm_"));
    assert!(matches!(
        auth.complete_mfa(
            &ctx(),
            &c.token,
            SecondFactor::Totp(&code_at_step(&secret, step))
        )
        .await,
        Err(AuthError::InvalidMfaCode)
    ));
    let next = code_at_step(&secret, step + 1);
    let login = auth
        .complete_mfa(&ctx(), &c.token, SecondFactor::Totp(&next))
        .await
        .unwrap();
    assert_eq!(login.user.id, id);
    assert!(auth.authenticate(&login.token).await.unwrap().is_some());

    // A challenge works once.
    assert!(matches!(
        auth.complete_mfa(&ctx(), &c.token, SecondFactor::Totp(&next))
            .await,
        Err(AuthError::InvalidMfaChallenge)
    ));
    assert!(matches!(
        auth.complete_mfa(&ctx(), "hxm_garbage", SecondFactor::Totp(&next))
            .await,
        Err(AuthError::InvalidMfaChallenge)
    ));
}

#[tokio::test]
async fn recovery_codes_work_once() {
    let Some(pools) = pools().await else { return };
    let auth = service(&pools);
    let (id, email, _secret, codes, _) = enrolled(&pools, &auth).await;

    let c = challenge(&auth, &email).await;
    // Typed loosely: lower case, no dash.
    let typed = codes[0].to_lowercase().replace('-', "");
    auth.complete_mfa(&ctx(), &c.token, SecondFactor::RecoveryCode(&typed))
        .await
        .unwrap();
    assert_eq!(
        auth.mfa_status(id).await.unwrap().recovery_codes_remaining,
        9
    );

    let c = challenge(&auth, &email).await;
    assert!(matches!(
        auth.complete_mfa(&ctx(), &c.token, SecondFactor::RecoveryCode(&codes[0]))
            .await,
        Err(AuthError::InvalidMfaCode)
    ));

    // Regenerating replaces the whole set.
    let fresh = auth
        .regenerate_recovery_codes(&ctx(), id, PASSWORD)
        .await
        .unwrap();
    let c = challenge(&auth, &email).await;
    assert!(matches!(
        auth.complete_mfa(&ctx(), &c.token, SecondFactor::RecoveryCode(&codes[1]))
            .await,
        Err(AuthError::InvalidMfaCode)
    ));
    auth.complete_mfa(&ctx(), &c.token, SecondFactor::RecoveryCode(&fresh[0]))
        .await
        .unwrap();
}

#[tokio::test]
async fn a_challenge_allows_five_attempts() {
    let Some(pools) = pools().await else { return };
    let auth = service_with(
        &pools,
        AuthConfig {
            lockout_threshold: 100,
            ..AuthConfig::default()
        },
    );
    let (_, email, secret, _, _) = enrolled(&pools, &auth).await;
    let c = challenge(&auth, &email).await;
    for _ in 0..5 {
        assert!(matches!(
            auth.complete_mfa(&ctx(), &c.token, SecondFactor::Totp("000000"))
                .await,
            Err(AuthError::InvalidMfaCode)
        ));
    }
    let good = code_at_step(&secret, now_step() + 1);
    assert!(matches!(
        auth.complete_mfa(&ctx(), &c.token, SecondFactor::Totp(&good))
            .await,
        Err(AuthError::InvalidMfaChallenge)
    ));
}

#[tokio::test]
async fn wrong_codes_count_toward_lockout() {
    let Some(pools) = pools().await else { return };
    let auth = service_with(
        &pools,
        AuthConfig {
            lockout_threshold: 3,
            ..AuthConfig::default()
        },
    );
    let (_, email, _, _, _) = enrolled(&pools, &auth).await;
    // Spread across challenges: a fresh challenge doesn't reset the count.
    for _ in 0..3 {
        let c = challenge(&auth, &email).await;
        let _ = auth
            .complete_mfa(&ctx(), &c.token, SecondFactor::Totp("000000"))
            .await;
    }
    // Locked: the password no longer gets a challenge.
    assert!(matches!(
        auth.login(&ctx(), &email, PASSWORD).await,
        Err(AuthError::InvalidCredentials)
    ));
}

#[tokio::test]
async fn a_challenge_dies_when_the_account_is_locked_or_disabled() {
    let Some(pools) = pools().await else { return };
    let auth = service(&pools);
    let (id, email, secret, _, _) = enrolled(&pools, &auth).await;
    let c = challenge(&auth, &email).await;
    let mut tx = pools.app.begin().await.unwrap();
    huxley_auth::update_user(
        &mut tx,
        &ctx(),
        id,
        None,
        Some(huxley_auth::UserStatus::Disabled),
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    let good = code_at_step(&secret, now_step() + 1);
    assert!(matches!(
        auth.complete_mfa(&ctx(), &c.token, SecondFactor::Totp(&good))
            .await,
        Err(AuthError::InvalidMfaChallenge)
    ));
}

#[tokio::test]
async fn disabling_and_admin_reset_remove_the_requirement() {
    let Some(pools) = pools().await else { return };
    let auth = service(&pools);

    // The user turns it off themselves (password required).
    let (id, email, _, _, _) = enrolled(&pools, &auth).await;
    assert!(matches!(
        auth.disable_totp(&ctx(), id, "wrong password!!").await,
        Err(AuthError::WrongCurrentPassword)
    ));
    auth.disable_totp(&ctx(), id, PASSWORD).await.unwrap();
    assert_eq!(
        auth.mfa_status(id).await.unwrap(),
        huxley_auth::MfaStatus {
            totp_enabled: false,
            recovery_codes_remaining: 0
        }
    );
    signed_in(&auth, &email).await;
    assert!(matches!(
        auth.disable_totp(&ctx(), id, PASSWORD).await,
        Err(AuthError::MfaNotEnabled)
    ));

    // An admin resets it: sessions end and pending challenges die.
    let (id, email, secret, _, kept) = enrolled(&pools, &auth).await;
    let c = challenge(&auth, &email).await;
    let mut tx = pools.app.begin().await.unwrap();
    assert!(mfa::reset_mfa(&mut tx, &ctx(), id).await.unwrap());
    tx.commit().await.unwrap();
    assert!(
        !auth
            .list_sessions(id)
            .await
            .unwrap()
            .iter()
            .any(|s| s.id == kept)
    );
    let good = code_at_step(&secret, now_step() + 1);
    assert!(matches!(
        auth.complete_mfa(&ctx(), &c.token, SecondFactor::Totp(&good))
            .await,
        Err(AuthError::InvalidMfaChallenge)
    ));
    signed_in(&auth, &email).await;
}
