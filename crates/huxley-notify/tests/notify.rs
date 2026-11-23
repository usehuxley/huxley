//! Notifications against a real PostgreSQL.

use std::time::Duration;

use huxley_core::{RequestContext, Via};
use huxley_db::testing::{TestPools, pools, unique};
use huxley_email::Mailer;
use huxley_notify::{Broadcaster, Channels, Notice, NotifyError, kinds};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

fn ctx() -> RequestContext {
    RequestContext::system(Via::System)
}

async fn new_user(pools: &TestPools) -> (Uuid, String) {
    let email = format!("{}@example.com", unique("notify").replace(' ', "."));
    let mut tx = pools.app.begin().await.unwrap();
    let u = huxley_auth::create_user(
        &mut tx,
        &ctx(),
        &email,
        "Nia Notify",
        "a fine long passphrase",
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    (u.id, email)
}

fn notice(user_id: Uuid, kind: &'static str) -> Notice<'static> {
    Notice {
        user_id,
        kind,
        title: "Something happened",
        body: "Details",
        link: Some("/profile"),
    }
}

async fn email_status(pools: &TestPools, id: Uuid) -> String {
    let (status,): (String,) =
        sqlx::query_as("SELECT email_status::text FROM notifications WHERE id = $1")
            .bind(id)
            .fetch_one(&pools.owner)
            .await
            .unwrap();
    status
}

#[tokio::test]
async fn preferences_choose_channels_except_for_security_notices() {
    let Some(pools) = pools().await else { return };
    let (user, _) = new_user(&pools).await;
    let mut conn = pools.app.acquire().await.unwrap();

    // Defaults: role grants are in-app only; security notices also email.
    let grant = huxley_notify::notify(&mut conn, notice(user, kinds::ROLE_GRANTED))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(email_status(&pools, grant).await, "skipped");
    let security = huxley_notify::notify(&mut conn, notice(user, kinds::PASSWORD_CHANGED))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(email_status(&pools, security).await, "pending");

    // Opt in to email for role grants; opt out of team notices entirely.
    let both = Channels {
        in_app: true,
        email: true,
    };
    huxley_notify::set_preference(&mut conn, user, kinds::ROLE_GRANTED, both)
        .await
        .unwrap();
    let grant = huxley_notify::notify(&mut conn, notice(user, kinds::ROLE_GRANTED))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(email_status(&pools, grant).await, "pending");
    let none = Channels {
        in_app: false,
        email: false,
    };
    huxley_notify::set_preference(&mut conn, user, kinds::TEAM_JOINED, none)
        .await
        .unwrap();
    assert!(
        huxley_notify::notify(&mut conn, notice(user, kinds::TEAM_JOINED))
            .await
            .unwrap()
            .is_none()
    );

    // Security notices can't be turned off.
    assert!(matches!(
        huxley_notify::set_preference(&mut conn, user, kinds::MFA_CHANGED, none).await,
        Err(NotifyError::Mandatory(_))
    ));
    let prefs = huxley_notify::preferences(&mut conn, user).await.unwrap();
    let mfa = prefs.iter().find(|p| p.kind == kinds::MFA_CHANGED).unwrap();
    assert!(mfa.mandatory && mfa.email && mfa.in_app);
    assert!(matches!(
        huxley_notify::notify(&mut conn, notice(user, "made.up")).await,
        Err(NotifyError::UnknownKind(_))
    ));
}

#[tokio::test]
async fn the_feed_counts_and_marks_unread() {
    let Some(pools) = pools().await else { return };
    let (user, _) = new_user(&pools).await;
    let (other, _) = new_user(&pools).await;
    let mut conn = pools.app.acquire().await.unwrap();
    for _ in 0..3 {
        huxley_notify::notify(&mut conn, notice(user, kinds::ROLE_GRANTED))
            .await
            .unwrap();
    }
    assert_eq!(
        huxley_notify::unread_count(&mut conn, user).await.unwrap(),
        3
    );
    let page = huxley_notify::list(&mut conn, user, None, 2, false)
        .await
        .unwrap();
    assert_eq!(page.len(), 3, "limit + 1 tells the caller there's more");
    assert!(page[0].id > page[1].id, "newest first");

    // Someone else can't mark it read.
    assert!(matches!(
        huxley_notify::mark_read(&mut conn, other, page[0].id).await,
        Err(NotifyError::NotFound)
    ));
    huxley_notify::mark_read(&mut conn, user, page[0].id)
        .await
        .unwrap();
    assert_eq!(
        huxley_notify::unread_count(&mut conn, user).await.unwrap(),
        2
    );
    assert_eq!(
        huxley_notify::list(&mut conn, user, None, 10, true)
            .await
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        huxley_notify::mark_all_read(&mut conn, user).await.unwrap(),
        2
    );
    assert_eq!(
        huxley_notify::unread_count(&mut conn, user).await.unwrap(),
        0
    );
}

#[tokio::test]
async fn service_accounts_get_no_notifications() {
    let Some(pools) = pools().await else { return };
    let mut tx = pools.app.begin().await.unwrap();
    let sa = huxley_auth::api_tokens::create_service_account(&mut tx, &ctx(), &unique("Bot"), "")
        .await
        .unwrap();
    assert!(
        huxley_notify::notify(&mut tx, notice(sa.id, kinds::PASSWORD_CHANGED))
            .await
            .unwrap()
            .is_none()
    );
    tx.rollback().await.unwrap();
}

#[tokio::test]
async fn queued_emails_are_sent_and_retried() {
    let Some(pools) = pools().await else { return };
    let (user, email) = new_user(&pools).await;
    let mut conn = pools.app.acquire().await.unwrap();
    let id = huxley_notify::notify(&mut conn, notice(user, kinds::PASSWORD_CHANGED))
        .await
        .unwrap()
        .unwrap();

    // Other tests queue emails in the same database, and a batch takes the
    // 20 most overdue: make this one the most overdue.
    let overdue =
        "UPDATE notifications SET email_next_at = now() - interval '10 years' WHERE id = $1";
    sqlx::query(overdue)
        .bind(id)
        .execute(&pools.owner)
        .await
        .unwrap();

    // A mailer that can't send: the email stays queued for a retry.
    let broken = Mailer::from_config(&huxley_email::EmailConfig {
        smtp_url: Some(secrecy::SecretString::from("smtp://127.0.0.1:1")),
        from: Some("Huxley <h@example.com>".into()),
    })
    .unwrap();
    huxley_notify::send_batch(&pools.app, &broken, None)
        .await
        .unwrap();
    let (status, attempts): (String, i32) = sqlx::query_as(
        "SELECT email_status::text, email_attempts FROM notifications WHERE id = $1",
    )
    .bind(id)
    .fetch_one(&pools.owner)
    .await
    .unwrap();
    assert_eq!((status.as_str(), attempts), ("pending", 1));

    // Due again now; a working mailer sends it, with a link into the app.
    sqlx::query(overdue)
        .bind(id)
        .execute(&pools.owner)
        .await
        .unwrap();
    let (mailer, outbox) = Mailer::memory();
    let sent = huxley_notify::send_batch(&pools.app, &mailer, Some("https://huxley.example.com"))
        .await
        .unwrap();
    assert!(sent >= 1);
    assert_eq!(email_status(&pools, id).await, "sent");
    let mail = outbox
        .lock()
        .unwrap()
        .iter()
        .find(|m| m.to == email)
        .cloned()
        .unwrap();
    assert_eq!(mail.subject, "Something happened");
    assert!(mail.text.contains("https://huxley.example.com/profile"));
}

#[tokio::test]
async fn in_app_notifications_arrive_in_real_time_after_commit() {
    let Some(pools) = pools().await else { return };
    let (user, _) = new_user(&pools).await;
    let broadcaster = Broadcaster::new();
    let mut rx = broadcaster.subscribe();
    let shutdown = CancellationToken::new();
    tokio::spawn(broadcaster.clone().run(pools.app.clone(), shutdown.clone()));
    tokio::time::sleep(Duration::from_millis(300)).await; // let it LISTEN

    let mut tx = pools.app.begin().await.unwrap();
    huxley_notify::notify(&mut tx, notice(user, kinds::ROLE_GRANTED))
        .await
        .unwrap();
    // Nothing before commit.
    assert!(
        tokio::time::timeout(Duration::from_millis(200), async {
            loop {
                if rx.recv().await.unwrap() == user {
                    break;
                }
            }
        })
        .await
        .is_err()
    );
    tx.commit().await.unwrap();
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if rx.recv().await.unwrap() == user {
                break;
            }
        }
    })
    .await
    .expect("delivered after commit");
    shutdown.cancel();
}
