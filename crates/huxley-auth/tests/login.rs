//! Login, sessions, and lockout against a real PostgreSQL.

use huxley_auth::{AuthConfig, AuthError, AuthService, create_user};
use huxley_core::{RequestContext, Via};
use huxley_crypto::Crypto;
use huxley_db::testing::{TestPools, pools, unique};

const PASSWORD: &str = "a perfectly fine passphrase";

fn ctx() -> RequestContext {
    RequestContext::system(Via::Api)
}

async fn user(pools: &TestPools) -> String {
    let email = format!("{}@example.com", unique("user").replace(' ', "."));
    let mut tx = pools.app.begin().await.unwrap();
    create_user(&mut tx, &ctx(), &email, "Test User", PASSWORD)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    email
}

fn service(pools: &TestPools) -> AuthService {
    AuthService::new(
        pools.app.clone(),
        AuthConfig::default(),
        Crypto::new(pools.app.clone(), huxley_crypto::testing::key_ring()),
    )
}

#[tokio::test]
async fn login_creates_a_session_that_authenticates() {
    let Some(pools) = pools().await else { return };
    let email = user(&pools).await;
    let auth = service(&pools);

    // Emails match case-insensitively.
    let login = auth
        .login(&ctx(), &email.to_uppercase(), PASSWORD)
        .await
        .unwrap()
        .into_session()
        .unwrap();
    assert!(login.token.starts_with("hxs_"));

    let session = auth.authenticate(&login.token).await.unwrap().unwrap();
    assert_eq!(session.user.id, login.user.id);
    assert_eq!(session.session_id, login.session_id);

    let sessions = auth.list_sessions(login.user.id).await.unwrap();
    assert!(sessions.iter().any(|s| s.id == login.session_id));
}

#[tokio::test]
async fn wrong_password_and_unknown_email_fail_identically() {
    let Some(pools) = pools().await else { return };
    let email = user(&pools).await;
    let auth = service(&pools);

    let wrong = auth.login(&ctx(), &email, "not the password!!").await;
    let unknown = auth
        .login(&ctx(), "nobody-here@example.com", PASSWORD)
        .await;
    assert!(matches!(wrong, Err(AuthError::InvalidCredentials)));
    assert!(matches!(unknown, Err(AuthError::InvalidCredentials)));
}

#[tokio::test]
async fn repeated_failures_lock_the_account() {
    let Some(pools) = pools().await else { return };
    let email = user(&pools).await;
    let auth = AuthService::new(
        pools.app.clone(),
        AuthConfig {
            lockout_threshold: 3,
            ..AuthConfig::default()
        },
        Crypto::new(pools.app.clone(), huxley_crypto::testing::key_ring()),
    );

    for _ in 0..3 {
        let _ = auth.login(&ctx(), &email, "not the password!!").await;
    }
    // Even the right password fails while locked.
    let locked = auth.login(&ctx(), &email, PASSWORD).await;
    assert!(matches!(locked, Err(AuthError::InvalidCredentials)));

    let (locked_audits,): (i64,) = sqlx::query_as(
        "SELECT count(*) FROM audit_events \
         WHERE action = 'auth.account_locked' AND details->>'failures' = '3' \
           AND target_id = (SELECT id FROM users WHERE email = $1)",
    )
    .bind(&email)
    .fetch_one(&pools.owner)
    .await
    .unwrap();
    assert_eq!(locked_audits, 1);
}

#[tokio::test]
async fn logout_and_revocation_end_sessions() {
    let Some(pools) = pools().await else { return };
    let email = user(&pools).await;
    let auth = service(&pools);

    let first = auth
        .login(&ctx(), &email, PASSWORD)
        .await
        .unwrap()
        .into_session()
        .unwrap();
    let second = auth
        .login(&ctx(), &email, PASSWORD)
        .await
        .unwrap()
        .into_session()
        .unwrap();

    auth.logout(&ctx(), first.session_id).await.unwrap();
    assert!(auth.authenticate(&first.token).await.unwrap().is_none());

    // A user may revoke their own other session, but not someone else's.
    let other = user(&pools).await;
    let stranger = auth
        .login(&ctx(), &other, PASSWORD)
        .await
        .unwrap()
        .into_session()
        .unwrap();
    let denied = auth
        .revoke_own_session(&ctx(), stranger.user.id, second.session_id)
        .await;
    assert!(matches!(denied, Err(AuthError::SessionNotFound)));

    auth.revoke_own_session(&ctx(), second.user.id, second.session_id)
        .await
        .unwrap();
    assert!(auth.authenticate(&second.token).await.unwrap().is_none());
}

#[tokio::test]
async fn disabled_users_cannot_log_in_or_use_sessions() {
    let Some(pools) = pools().await else { return };
    let email = user(&pools).await;
    let auth = service(&pools);
    let login = auth
        .login(&ctx(), &email, PASSWORD)
        .await
        .unwrap()
        .into_session()
        .unwrap();

    sqlx::query("UPDATE users SET status = 'disabled' WHERE id = $1")
        .bind(login.user.id)
        .execute(&pools.owner)
        .await
        .unwrap();

    assert!(auth.authenticate(&login.token).await.unwrap().is_none());
    assert!(matches!(
        auth.login(&ctx(), &email, PASSWORD).await,
        Err(AuthError::InvalidCredentials)
    ));
}

#[tokio::test]
async fn garbage_tokens_are_rejected_without_a_query() {
    let Some(pools) = pools().await else { return };
    let auth = service(&pools);
    assert!(auth.authenticate("hxs_nope").await.unwrap().is_none());
    assert!(auth.authenticate("").await.unwrap().is_none());
}

#[tokio::test]
async fn duplicate_emails_and_weak_passwords_are_rejected() {
    let Some(pools) = pools().await else { return };
    let email = user(&pools).await;

    let mut tx = pools.app.begin().await.unwrap();
    let dup = create_user(&mut tx, &ctx(), &email.to_uppercase(), "Dup", PASSWORD).await;
    assert!(matches!(dup, Err(AuthError::EmailTaken)), "got {dup:?}");
    tx.rollback().await.unwrap();

    let mut tx = pools.app.begin().await.unwrap();
    let weak = create_user(&mut tx, &ctx(), "weak@example.com", "Weak", "short").await;
    assert!(matches!(weak, Err(AuthError::WeakPassword(_))));
}

#[tokio::test]
async fn changing_the_password_revokes_other_sessions() {
    let Some(pools) = pools().await else { return };
    let email = user(&pools).await;
    let auth = service(&pools);
    let here = auth
        .login(&ctx(), &email, PASSWORD)
        .await
        .unwrap()
        .into_session()
        .unwrap();
    let elsewhere = auth
        .login(&ctx(), &email, PASSWORD)
        .await
        .unwrap()
        .into_session()
        .unwrap();

    let wrong = auth
        .change_password(
            &ctx(),
            here.user.id,
            here.session_id,
            "not it at all",
            "another fine passphrase",
        )
        .await;
    assert!(matches!(wrong, Err(AuthError::WrongCurrentPassword)));

    auth.change_password(
        &ctx(),
        here.user.id,
        here.session_id,
        PASSWORD,
        "another fine passphrase",
    )
    .await
    .unwrap();

    assert!(auth.authenticate(&here.token).await.unwrap().is_some());
    assert!(auth.authenticate(&elsewhere.token).await.unwrap().is_none());
    assert!(auth.login(&ctx(), &email, PASSWORD).await.is_err());
    assert!(
        auth.login(&ctx(), &email, "another fine passphrase")
            .await
            .is_ok()
    );
}
