//! Two-factor sign-in for portal accounts, against a real PostgreSQL.

use huxley_auth::totp;
use huxley_core::{RequestContext, Via};
use huxley_crypto::Crypto;
use huxley_db::testing::{TestPools, ensure_root, isolated};
use huxley_db::{OrgScope, begin_scoped};
use huxley_portal::signin::{self, NewSession, SignedIn};
use huxley_portal::{PortalError, PortalPrincipal, Role, accounts, mfa};
use serde_json::json;
use sqlx::PgConnection;
use uuid::Uuid;

fn ctx() -> RequestContext {
    RequestContext::system(Via::System)
}

fn base32_decode(s: &str) -> Vec<u8> {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
    let (mut bits, mut value, mut out) = (0u32, 0u32, Vec::new());
    for c in s.trim_end_matches('=').bytes() {
        let i = u32::try_from(ALPHABET.iter().position(|&a| a == c).unwrap()).unwrap();
        value = (value << 5) | i;
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            out.push(u8::try_from((value >> bits) & 0xff).unwrap());
        }
    }
    out
}

/// The app's code for the next time step (the current one may be used).
fn next_code(secret: &[u8]) -> String {
    let now = time::OffsetDateTime::now_utc().unix_timestamp();
    totp::code_at_step(secret, totp::step_at(now) + 1)
}

async fn contact(tx: &mut PgConnection, org: Uuid, first: &str, email: &str) -> Uuid {
    huxley_crm::contacts::create(
        tx,
        &ctx(),
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
    crypto: Crypto,
}

impl World {
    async fn tx(&self) -> sqlx::Transaction<'static, sqlx::Postgres> {
        begin_scoped(&self.pools.app, &OrgScope::System)
            .await
            .unwrap()
    }

    /// Signs in with a fresh link.
    async fn link(&self, account: Uuid) -> SignedIn {
        let mut tx = self.tx().await;
        let secret = signin::issue_link(&mut tx, account).await.unwrap();
        let r = signin::redeem(&mut tx, &ctx(), &secret, None, None)
            .await
            .unwrap();
        tx.commit().await.unwrap();
        r
    }

    async fn complete(&self, challenge: &str, code: &str) -> Result<NewSession, PortalError> {
        let mut tx = self.tx().await;
        let r = mfa::complete(&mut tx, &self.crypto, &ctx(), challenge, code, None, None).await;
        tx.commit().await.unwrap();
        r
    }
}

fn challenge_of(signed_in: SignedIn) -> String {
    match signed_in {
        SignedIn::MfaRequired { challenge } => challenge,
        SignedIn::Session(_) => panic!("expected a code to be asked for"),
    }
}

#[tokio::test]
async fn codes_are_asked_for_once_set_up() {
    let Some(db) = isolated("portal_mfa").await else {
        return;
    };
    let root = ensure_root(&db.pools).await;
    let w = World {
        pools: db.pools.clone(),
        crypto: Crypto::new(db.pools.app.clone(), huxley_crypto::testing::key_ring()),
    };
    let mut tx = w.tx().await;
    let acme = huxley_orgs::create(&mut tx, &ctx(), root, huxley_orgs::OrgKind::Client, "Acme")
        .await
        .unwrap()
        .id;
    let ada = contact(&mut tx, acme, "Ada", "ada@acme.example").await;
    let account = accounts::invite(&mut tx, &ctx(), ada, Role::ClientUser)
        .await
        .unwrap()
        .id;
    tx.commit().await.unwrap();

    // Set up from a signed-in session.
    let me = w
        .link(account)
        .await
        .session()
        .expect("no code before setup")
        .principal;
    assert!(!me.mfa_setup_required);
    let mut tx = w.tx().await;
    let enrollment = mfa::begin(&mut tx, &w.crypto, &ctx(), &me, "Acme support")
        .await
        .unwrap();
    assert!(
        enrollment
            .otpauth_uri
            .starts_with("otpauth://totp/Acme%20support:"),
        "{}",
        enrollment.otpauth_uri
    );
    assert!(
        enrollment
            .otpauth_uri
            .contains(&format!("secret={}", enrollment.secret))
    );
    let secret = base32_decode(&enrollment.secret);
    let wrong = mfa::confirm(&mut tx, &w.crypto, &ctx(), &me, "000000").await;
    assert!(
        matches!(wrong, Err(PortalError::Invalid { .. })),
        "{wrong:?}"
    );
    let now = time::OffsetDateTime::now_utc().unix_timestamp();
    let codes = mfa::confirm(
        &mut tx,
        &w.crypto,
        &ctx(),
        &me,
        &totp::code_at_step(&secret, totp::step_at(now)),
    )
    .await
    .unwrap();
    assert_eq!(codes.len(), 10);
    let status = mfa::status(&mut tx, &me).await.unwrap();
    assert!(status.enabled && !status.required && status.recovery_codes_left == 10);
    tx.commit().await.unwrap();

    // Now a link asks for a code.
    let challenge = challenge_of(w.link(account).await);
    assert!(matches!(
        w.complete(&challenge, "123456").await,
        Err(PortalError::BadCode)
    ));
    let session = w.complete(&challenge, &next_code(&secret)).await.unwrap();
    assert_eq!(session.principal.account_id, account);
    // The challenge is used up, and so is that code.
    assert!(matches!(
        w.complete(&challenge, &next_code(&secret)).await,
        Err(PortalError::BadCode)
    ));
    let challenge = challenge_of(w.link(account).await);
    assert!(matches!(
        w.complete(&challenge, &next_code(&secret)).await,
        Err(PortalError::BadCode)
    ));
    // A recovery code works once.
    w.complete(&challenge, &codes[0]).await.unwrap();
    let challenge = challenge_of(w.link(account).await);
    assert!(matches!(
        w.complete(&challenge, &codes[0]).await,
        Err(PortalError::BadCode)
    ));

    required_and_reset(&w, acme, account, &session.principal).await;
}

/// A company requiring it: newcomers can only set it up; it can't be turned
/// off; staff reset it.
async fn required_and_reset(w: &World, acme: Uuid, ada: Uuid, ada_me: &PortalPrincipal) {
    let mut tx = w.tx().await;
    huxley_orgs::settings::set(
        &mut tx,
        &ctx(),
        acme,
        huxley_orgs::settings::PORTAL_REQUIRE_MFA,
        "TRUE",
        false,
    )
    .await
    .unwrap();
    let bea = contact(&mut tx, acme, "Bea", "bea@acme.example").await;
    let bea = accounts::invite(&mut tx, &ctx(), bea, Role::ClientUser)
        .await
        .unwrap()
        .id;
    tx.commit().await.unwrap();
    let session = w.link(bea).await.session().unwrap();
    assert!(session.principal.mfa_setup_required);
    // Setting it up lifts the limit on this session.
    let mut tx = w.tx().await;
    let enrollment = mfa::begin(&mut tx, &w.crypto, &ctx(), &session.principal, "Acme")
        .await
        .unwrap();
    let now = time::OffsetDateTime::now_utc().unix_timestamp();
    let code = totp::code_at_step(&base32_decode(&enrollment.secret), totp::step_at(now));
    mfa::confirm(&mut tx, &w.crypto, &ctx(), &session.principal, &code)
        .await
        .unwrap();
    let me = signin::authenticate(&mut tx, &session.secret)
        .await
        .unwrap()
        .unwrap();
    assert!(!me.mfa_setup_required);

    // Ada can't turn it off while it's required.
    let off = mfa::disable(&mut tx, &w.crypto, &ctx(), ada_me, "123456").await;
    assert!(matches!(off, Err(PortalError::Invalid { .. })), "{off:?}");
    // Staff reset it: her sessions end, and she must set it up again.
    mfa::reset(&mut tx, &ctx(), ada).await.unwrap();
    assert!(!accounts::get(&mut tx, ada).await.unwrap().mfa_enabled);
    tx.commit().await.unwrap();
    let again = w.link(ada).await.session().expect("no code after a reset");
    assert!(again.principal.mfa_setup_required);
}
