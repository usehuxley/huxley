//! Runner identity end to end (ADR-025): enrolment, the mTLS gateway,
//! renewal, revocation, and certificates from the operator's own CA.

use huxley_core::{RequestContext, Via};
use huxley_crypto::Crypto;
use huxley_db::testing::{IsolatedDb, ensure_root, isolated};
use huxley_db::{OrgScope, begin_scoped};
use huxley_runners::ca::{self, Ca};
use huxley_runners::gateway::{self, ClientTls, Gateway, GatewayTls};
use huxley_runners::proto::{HeartbeatRequest, RenewRequest};
use huxley_runners::registry::{self, RegistryError};
use tokio_util::sync::CancellationToken;

struct Harness {
    db: IsolatedDb,
    crypto: Crypto,
    endpoint: String,
    ca_pem: String,
    stop: CancellationToken,
}

impl Drop for Harness {
    fn drop(&mut self) {
        self.stop.cancel();
    }
}

/// Starts a gateway trusting the instance CA and `extra_ca` (if any).
async fn harness(extra_ca: Option<&str>) -> Option<Harness> {
    let db = isolated("runners").await?;
    ensure_root(&db.pools).await;
    let crypto = Crypto::new(db.pools.app.clone(), huxley_crypto::testing::key_ring());
    let tls: GatewayTls = gateway::tls(&db.pools.app, &crypto, &["localhost".into()], extra_ca)
        .await
        .unwrap();
    let ca_pem = tls
        .client_ca_pem
        .split("\n-----BEGIN")
        .next()
        .unwrap()
        .to_owned();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let stop = CancellationToken::new();
    let server = Gateway::new(db.pools.app.clone(), crypto.clone());
    let token = stop.clone();
    tokio::spawn(async move { gateway::serve(server, listener, &tls, token).await });
    Some(Harness {
        db,
        crypto,
        endpoint: format!("https://localhost:{port}"),
        ca_pem,
        stop,
    })
}

impl Harness {
    async fn enrol(&self, name: &str) -> (registry::Enrolled, String) {
        let ctx = RequestContext::system(Via::System);
        let mut tx = begin_scoped(&self.db.pools.app, &OrgScope::System)
            .await
            .unwrap();
        let (token, _) = registry::create_enrollment(&mut tx, &ctx, name)
            .await
            .unwrap();
        tx.commit().await.unwrap();
        let (key, csr) = ca::runner_key_and_csr().unwrap();
        let mut tx = begin_scoped(&self.db.pools.app, &OrgScope::System)
            .await
            .unwrap();
        let enrolled = registry::enroll(&mut tx, &self.crypto, &token, &csr)
            .await
            .unwrap();
        // The token works once.
        let (_, csr2) = ca::runner_key_and_csr().unwrap();
        let again = registry::enroll(&mut tx, &self.crypto, &token, &csr2).await;
        assert!(matches!(again, Err(RegistryError::BadToken)));
        tx.commit().await.unwrap();
        (enrolled, key)
    }

    async fn heartbeat(&self, cert_pem: &str, key_pem: &str) -> Result<String, String> {
        let tls = ClientTls {
            ca_pem: self.ca_pem.clone(),
            cert_pem: cert_pem.to_owned(),
            key_pem: key_pem.to_owned(),
            domain: "localhost".into(),
        };
        let mut client = gateway::connect(&self.endpoint, &tls)
            .await
            .map_err(|e| format!("connect: {e}"))?;
        client
            .heartbeat(HeartbeatRequest {
                languages: vec!["pwsh".into()],
                version: "test".into(),
            })
            .await
            .map(|r| r.into_inner().runner_name)
            .map_err(|s| format!("{:?}: {}", s.code(), s.message()))
    }
}

#[tokio::test]
async fn enrolled_runners_connect_renew_and_can_be_revoked() {
    let Some(h) = harness(None).await else { return };
    let (enrolled, key) = h.enrol("runner-1").await;
    assert_eq!(enrolled.runner.name, "runner-1");
    assert_eq!(
        h.heartbeat(&enrolled.certificate_pem, &key).await,
        Ok("runner-1".into())
    );
    let mut tx = begin_scoped(&h.db.pools.app, &OrgScope::System)
        .await
        .unwrap();
    let seen = registry::get(&mut tx, enrolled.runner.id).await.unwrap();
    tx.commit().await.unwrap();
    assert!(seen.last_seen_at.is_some());
    assert_eq!(seen.languages, vec!["pwsh"]);

    // Renewing with a new key: the new certificate works, the old one doesn't.
    let tls = ClientTls {
        ca_pem: h.ca_pem.clone(),
        cert_pem: enrolled.certificate_pem.clone(),
        key_pem: key.clone(),
        domain: "localhost".into(),
    };
    let mut client = gateway::connect(&h.endpoint, &tls).await.unwrap();
    let (new_key, csr) = ca::runner_key_and_csr().unwrap();
    let renewed = client
        .renew(RenewRequest { csr_pem: csr })
        .await
        .unwrap()
        .into_inner();
    assert_eq!(
        h.heartbeat(&renewed.certificate_pem, &new_key).await,
        Ok("runner-1".into())
    );
    let old = h
        .heartbeat(&enrolled.certificate_pem, &key)
        .await
        .unwrap_err();
    assert!(old.contains("PermissionDenied"), "{old}");

    // Revoked: refused at once.
    let ctx = RequestContext::system(Via::System);
    let mut tx = begin_scoped(&h.db.pools.app, &OrgScope::System)
        .await
        .unwrap();
    registry::revoke(&mut tx, &ctx, enrolled.runner.id)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let revoked = h
        .heartbeat(&renewed.certificate_pem, &new_key)
        .await
        .unwrap_err();
    assert!(revoked.contains("PermissionDenied"), "{revoked}");
}

#[tokio::test]
async fn certificates_from_the_operators_ca_need_a_registered_name() {
    // The operator's own CA, trusted by the gateway.
    let theirs = ca::create().unwrap();
    let Some(h) = harness(Some(&theirs.cert_pem)).await else {
        return;
    };
    let their_ca = Ca::load(&theirs.cert_pem, &theirs.key_pem).unwrap();
    let (key, csr) = ca::runner_key_and_csr().unwrap();
    let cert = their_ca.issue_runner(&csr, "byo-1").unwrap().cert_pem;

    // Not registered yet: refused.
    let refused = h.heartbeat(&cert, &key).await.unwrap_err();
    assert!(refused.contains("PermissionDenied"), "{refused}");
    let ctx = RequestContext::system(Via::System);
    let mut tx = begin_scoped(&h.db.pools.app, &OrgScope::System)
        .await
        .unwrap();
    registry::register_external(&mut tx, &ctx, "byo-1")
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(h.heartbeat(&cert, &key).await, Ok("byo-1".into()));

    // A certificate from a CA nobody trusts doesn't get through TLS.
    let stranger = ca::create().unwrap();
    let stranger = Ca::load(&stranger.cert_pem, &stranger.key_pem).unwrap();
    let (key, csr) = ca::runner_key_and_csr().unwrap();
    let cert = stranger.issue_runner(&csr, "byo-1").unwrap().cert_pem;
    assert!(h.heartbeat(&cert, &key).await.is_err());
}
