//! Password reset, invitations, and email verification against a real
//! PostgreSQL, with an in-memory mailer.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use huxley_auth::{AuthConfig, AuthError, AuthService, LoginOutcome, User, create_user};
use huxley_core::{RequestContext, Via};
use huxley_crypto::Crypto;
use huxley_db::testing::{TestPools, ensure_root, pools, unique};
use huxley_email::{Email, Mailer};

const PASSWORD: &str = "a perfectly fine passphrase";
const NEW_PASSWORD: &str = "a brand new passphrase";

fn ctx() -> RequestContext {
    RequestContext::system(Via::Api)
}

fn service(pools: &TestPools) -> (AuthService, Arc<Mutex<Vec<Email>>>) {
    let (mailer, outbox) = Mailer::memory();
    let auth = AuthService::new(
        pools.app.clone(),
        AuthConfig {
            public_url: Some("https://huxley.example.com".into()),
            ..AuthConfig::default()
        },
        Crypto::new(pools.app.clone(), huxley_crypto::testing::key_ring()),
    )
    .with_mailer(mailer);
    (auth, outbox)
}

async fn user(pools: &TestPools) -> User {
    ensure_root(pools).await;
    let email = format!("{}@example.com", unique("mail").replace(' ', "."));
    let mut tx = pools.app.begin().await.unwrap();
    let user = create_user(&mut tx, &ctx(), &email, "Mel Mail", PASSWORD)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    user
}

/// Waits for background sends, then returns the messages to `to`.
async fn mail_to(outbox: &Arc<Mutex<Vec<Email>>>, to: &str, expected: usize) -> Vec<Email> {
    for _ in 0..50 {
        let found: Vec<Email> = outbox
            .lock()
            .unwrap()
            .iter()
            .filter(|e| e.to == to)
            .cloned()
            .collect();
        if found.len() >= expected {
            return found;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    outbox
        .lock()
        .unwrap()
        .iter()
        .filter(|e| e.to == to)
        .cloned()
        .collect()
}

fn token_from(email: &Email, path: &str) -> String {
    let marker = format!("https://huxley.example.com{path}?token=");
    let start = email.text.find(&marker).expect("link in email") + marker.len();
    email.text[start..]
        .split_whitespace()
        .next()
        .unwrap()
        .to_owned()
}

#[tokio::test]
async fn password_reset_sets_a_new_password_and_signs_out_everywhere() {
    let Some(pools) = pools().await else { return };
    let (auth, outbox) = service(&pools);
    let user = user(&pools).await;
    assert!(!user.email_verified);
    let session = auth
        .login(&ctx(), &user.email, PASSWORD)
        .await
        .unwrap()
        .into_session()
        .unwrap();

    auth.request_password_reset(&ctx(), &user.email.to_uppercase())
        .await
        .unwrap();
    let mail = mail_to(&outbox, &user.email, 1).await;
    assert_eq!(mail.len(), 1);
    assert!(mail[0].subject.contains("Reset"));
    let token = token_from(&mail[0], "/reset-password");
    assert!(token.starts_with("hxe_"));

    // Too short: refused without using up the link.
    assert!(matches!(
        auth.reset_password(&ctx(), &token, "short").await,
        Err(AuthError::WeakPassword(_))
    ));
    let updated = auth
        .reset_password(&ctx(), &token, NEW_PASSWORD)
        .await
        .unwrap();
    assert!(
        updated.email_verified,
        "receiving the link proves the address"
    );
    assert!(
        auth.authenticate(&session.token).await.unwrap().is_none(),
        "signed out"
    );
    assert!(matches!(
        auth.login(&ctx(), &user.email, PASSWORD).await,
        Err(AuthError::InvalidCredentials)
    ));
    assert!(matches!(
        auth.login(&ctx(), &user.email, NEW_PASSWORD).await.unwrap(),
        LoginOutcome::SignedIn(_)
    ));
    // Single use.
    assert!(matches!(
        auth.reset_password(&ctx(), &token, "yet another passphrase")
            .await,
        Err(AuthError::InvalidEmailToken)
    ));
}

#[tokio::test]
async fn reset_requests_reveal_nothing_and_skip_ineligible_accounts() {
    let Some(pools) = pools().await else { return };
    let (auth, outbox) = service(&pools);
    ensure_root(&pools).await;

    // Unknown address: success, no mail.
    auth.request_password_reset(&ctx(), "nobody-at-all@example.com")
        .await
        .unwrap();

    // SSO-only account (no password): no mail, so reset can't bypass SSO.
    let sso_email = format!("{}@example.com", unique("sso").replace(' ', "."));
    let mut tx = pools.app.begin().await.unwrap();
    huxley_auth::create_external_user(&mut tx, &ctx(), &sso_email, "Sam Sso", "idp")
        .await
        .unwrap();
    tx.commit().await.unwrap();
    auth.request_password_reset(&ctx(), &sso_email)
        .await
        .unwrap();

    // Throttled: a second request within a minute sends nothing more.
    let user = user(&pools).await;
    auth.request_password_reset(&ctx(), &user.email)
        .await
        .unwrap();
    auth.request_password_reset(&ctx(), &user.email)
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(mail_to(&outbox, &user.email, 1).await.len(), 1);
    assert_eq!(mail_to(&outbox, &sso_email, 0).await, vec![]);
    assert_eq!(
        mail_to(&outbox, "nobody-at-all@example.com", 0).await,
        vec![]
    );

    // Garbage tokens.
    assert!(matches!(
        auth.reset_password(&ctx(), "hxe_garbage", NEW_PASSWORD)
            .await,
        Err(AuthError::InvalidEmailToken)
    ));
}

#[tokio::test]
async fn invitations_let_new_users_choose_a_password() {
    let Some(pools) = pools().await else { return };
    let (auth, outbox) = service(&pools);
    let user = user(&pools).await;
    auth.send_invitation(&ctx(), user.id, "Ada Admin")
        .await
        .unwrap();
    let mail = mail_to(&outbox, &user.email, 1).await;
    assert!(mail[0].subject.contains("invited"));
    assert!(mail[0].text.contains("Ada Admin"));
    let token = token_from(&mail[0], "/reset-password");
    auth.reset_password(&ctx(), &token, NEW_PASSWORD)
        .await
        .unwrap();
    assert!(matches!(
        auth.login(&ctx(), &user.email, NEW_PASSWORD).await.unwrap(),
        LoginOutcome::SignedIn(_)
    ));
}

#[tokio::test]
async fn verification_links_verify_the_current_address_once() {
    let Some(pools) = pools().await else { return };
    let (auth, outbox) = service(&pools);
    let user = user(&pools).await;
    auth.request_email_verification(&ctx(), &user)
        .await
        .unwrap();
    let mail = mail_to(&outbox, &user.email, 1).await;
    let token = token_from(&mail[0], "/verify-email");
    // A verification link can't reset a password.
    assert!(matches!(
        auth.reset_password(&ctx(), &token, NEW_PASSWORD).await,
        Err(AuthError::InvalidEmailToken)
    ));
    // That attempt didn't consume it (the transaction rolled back).
    let verified = auth.verify_email(&ctx(), &token).await.unwrap();
    assert!(verified.email_verified);
    assert!(matches!(
        auth.verify_email(&ctx(), &token).await,
        Err(AuthError::InvalidEmailToken)
    ));
}

#[tokio::test]
async fn without_email_configured_the_flows_say_so() {
    let Some(pools) = pools().await else { return };
    let auth = AuthService::new(
        pools.app.clone(),
        AuthConfig::default(),
        Crypto::new(pools.app.clone(), huxley_crypto::testing::key_ring()),
    );
    assert!(!auth.email_enabled());
    assert!(matches!(
        auth.request_password_reset(&ctx(), "a@example.com").await,
        Err(AuthError::EmailNotConfigured)
    ));
}
