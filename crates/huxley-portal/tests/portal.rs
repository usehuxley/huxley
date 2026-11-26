//! Portal accounts and sign-in against a real PostgreSQL.

use huxley_core::{RequestContext, Via};
use huxley_db::testing::{ensure_root, isolated};
use huxley_db::{OrgScope, begin_scoped};
use huxley_portal::accounts::{self, Status};
use huxley_portal::{PortalError, Role, signin};
use serde_json::json;
use sqlx::PgConnection;
use uuid::Uuid;

async fn contact(tx: &mut PgConnection, org: Uuid, first: &str, email: Option<&str>) -> Uuid {
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

#[tokio::test]
async fn contacts_are_invited_sign_in_and_are_disabled() {
    let Some(db) = isolated("portal").await else {
        return;
    };
    let pools = db.pools.clone();
    let root = ensure_root(&pools).await;
    let ctx = RequestContext::system(Via::System);
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    let tx = &mut *tx;
    let acme = huxley_orgs::create(tx, &ctx, root, huxley_orgs::OrgKind::Client, "Acme")
        .await
        .unwrap()
        .id;
    let ada = contact(tx, acme, "Ada", Some("Ada@Acme.example")).await;
    let nomail = contact(tx, acme, "Nomail", None).await;

    // Invited, with a role; an address is needed.
    let account = accounts::invite(tx, &ctx, ada, Role::ClientAdmin)
        .await
        .unwrap();
    assert_eq!(
        (account.status, account.role, account.email.as_str()),
        (Status::Invited, Role::ClientAdmin, "ada@acme.example")
    );
    assert!(matches!(
        accounts::invite(tx, &ctx, nomail, Role::ClientUser).await,
        Err(PortalError::Invalid { .. })
    ));

    // A link for them (any case), at most once a minute; none for strangers.
    let (id, _, secret) = signin::request_link(tx, &ctx, "ADA@acme.example")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(id, account.id);
    assert!(
        signin::request_link(tx, &ctx, "ada@acme.example")
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        signin::request_link(tx, &ctx, "nobody@acme.example")
            .await
            .unwrap()
            .is_none()
    );

    // Redeemed once: active, signed in.
    let session = signin::redeem(tx, &ctx, &secret, Some("192.0.2.1"), Some("test"))
        .await
        .unwrap()
        .session()
        .unwrap();
    assert_eq!(
        (session.principal.org_id, session.principal.role),
        (acme, Role::ClientAdmin)
    );
    assert_eq!(session.principal.name, "Ada Client");
    assert!(matches!(
        signin::redeem(tx, &ctx, &secret, None, None).await,
        Err(PortalError::BadCredentials)
    ));
    assert_eq!(
        accounts::get(tx, account.id).await.unwrap().status,
        Status::Active
    );
    let me = signin::authenticate(tx, &session.secret)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(me.account_id, account.id);
    // A staff-shaped or made-up token is nobody.
    assert!(
        signin::authenticate(tx, "hxs_notaportaltoken")
            .await
            .unwrap()
            .is_none()
    );

    let by_password = passwords(tx, &ctx, &me).await;
    disabling(
        tx,
        &ctx,
        account.id,
        ada,
        acme,
        &[session.secret, by_password],
    )
    .await;
}

/// Passwords: set, used, and locked after five misses. A session's secret.
async fn passwords(
    tx: &mut PgConnection,
    ctx: &RequestContext,
    me: &huxley_portal::PortalPrincipal,
) -> String {
    // Passwords: set, used, and locked after five misses.
    signin::set_password(tx, ctx, me, "correct horse battery staple")
        .await
        .unwrap();
    let by_password = signin::sign_in_with_password(
        tx,
        ctx,
        "ada@acme.example",
        "correct horse battery staple",
        None,
        None,
    )
    .await
    .unwrap()
    .session()
    .unwrap();
    assert_eq!(by_password.principal.account_id, me.account_id);
    for _ in 0..5 {
        let wrong =
            signin::sign_in_with_password(tx, ctx, "ada@acme.example", "wrong", None, None).await;
        assert!(matches!(wrong, Err(PortalError::BadCredentials)));
    }
    let locked = signin::sign_in_with_password(
        tx,
        ctx,
        "ada@acme.example",
        "correct horse battery staple",
        None,
        None,
    )
    .await;
    assert!(matches!(locked, Err(PortalError::Locked(_))), "{locked:?}");

    by_password.secret
}

/// Disabled: sessions end, no links; invited again.
async fn disabling(
    tx: &mut PgConnection,
    ctx: &RequestContext,
    account: Uuid,
    ada: Uuid,
    acme: Uuid,
    sessions: &[String],
) {
    // Disabled: sessions end, no links.
    let fresh = accounts::get(tx, account).await.unwrap();
    accounts::update(
        tx,
        ctx,
        account,
        Role::ClientUser,
        Status::Disabled,
        Some(fresh.version),
    )
    .await
    .unwrap();
    for secret in sessions {
        assert!(signin::authenticate(tx, secret).await.unwrap().is_none());
    }
    sqlx::query("UPDATE portal_login_tokens SET created_at = now() - interval '5 minutes'")
        .execute(&mut *tx)
        .await
        .unwrap();
    assert!(
        signin::request_link(tx, ctx, "ada@acme.example")
            .await
            .unwrap()
            .is_none()
    );
    // Invited again: invited (they had signed in, but disabling resets nothing else).
    let again = accounts::invite(tx, ctx, ada, Role::ClientUser)
        .await
        .unwrap();
    assert_eq!(again.status, Status::Invited);
    assert_eq!(accounts::list(tx, acme).await.unwrap().len(), 1);
}
