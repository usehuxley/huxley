//! The runner gateway (ADR-025): a gRPC listener that only talks to runners
//! with a trusted client certificate. TLS checks the certificate chain (the
//! instance CA, plus the operator's CA bundle if configured); every call
//! then asks the registry who the certificate belongs to, so revoking a
//! runner takes effect on its next call.

use std::net::SocketAddr;

use huxley_crypto::Crypto;
use huxley_db::{OrgScope, begin_scoped};
use sqlx::PgPool;
use tokio_util::sync::CancellationToken;
use tonic::transport::{Certificate, Identity, Server, ServerTlsConfig};
use tonic::{Request, Response, Status};

use crate::proto::runner_server::{Runner, RunnerServer};
use crate::proto::{
    HeartbeatRequest, HeartbeatResponse, LogRequest, LogResponse, NextRequest, NextResponse,
    RenewRequest, RenewResponse, ReportRequest, ReportResponse, Task,
};
use crate::registry::{self, RegistryError, RunnerSource, RunnerView};

/// The gateway's TLS material.
pub struct GatewayTls {
    pub server_cert_pem: String,
    pub server_key_pem: String,
    /// The CAs runners' certificates may come from.
    pub client_ca_pem: String,
}

impl std::fmt::Debug for GatewayTls {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GatewayTls").finish_non_exhaustive()
    }
}

/// Issues the gateway a fresh server certificate for `hosts` from the
/// instance CA, and trusts that CA plus `extra_ca_pem` (the operator's own
/// CA bundle, if any) for runners.
pub async fn tls(
    db: &PgPool,
    crypto: &Crypto,
    hosts: &[String],
    extra_ca_pem: Option<&str>,
) -> Result<GatewayTls, RegistryError> {
    let mut tx = begin_scoped(db, &OrgScope::System)
        .await
        .map_err(|e| RegistryError::Invalid(e.to_string()))?;
    let ca = registry::ca(&mut tx, crypto).await?;
    tx.commit().await?;
    let (server_cert_pem, server_key_pem) = ca.issue_server(hosts)?;
    let mut client_ca_pem = ca.cert_pem.clone();
    if let Some(extra) = extra_ca_pem {
        client_ca_pem.push('\n');
        client_ca_pem.push_str(extra);
    }
    Ok(GatewayTls {
        server_cert_pem,
        server_key_pem,
        client_ca_pem,
    })
}

/// Serves the runner protocol.
#[derive(Debug, Clone)]
pub struct Gateway {
    db: PgPool,
    crypto: Crypto,
    engine: huxley_engine::Engine,
}

impl Gateway {
    pub const fn new(db: PgPool, crypto: Crypto, engine: huxley_engine::Engine) -> Self {
        Self { db, crypto, engine }
    }

    /// Who's calling, from their client certificate.
    async fn runner<T>(&self, request: &Request<T>) -> Result<RunnerView, Status> {
        let certs = request
            .peer_certs()
            .ok_or_else(|| Status::unauthenticated("a client certificate is required"))?;
        let cert = certs
            .first()
            .ok_or_else(|| Status::unauthenticated("a client certificate is required"))?;
        let mut tx = begin_scoped(&self.db, &OrgScope::System)
            .await
            .map_err(|e| Status::internal(e.to_string()))?;
        let found = registry::identify(&mut tx, cert.as_ref())
            .await
            .map_err(|e| Status::internal(e.to_string()))?;
        tx.commit()
            .await
            .map_err(|e| Status::internal(e.to_string()))?;
        found.ok_or_else(|| Status::permission_denied("this runner is unknown or revoked"))
    }
}

#[tonic::async_trait]
impl Runner for Gateway {
    async fn heartbeat(
        &self,
        request: Request<HeartbeatRequest>,
    ) -> Result<Response<HeartbeatResponse>, Status> {
        let runner = self.runner(&request).await?;
        let body = request.into_inner();
        let mut tx = begin_scoped(&self.db, &OrgScope::System)
            .await
            .map_err(|e| Status::internal(e.to_string()))?;
        registry::seen(&mut tx, runner.id, &body.languages, &body.version)
            .await
            .map_err(|e| Status::internal(e.to_string()))?;
        tx.commit()
            .await
            .map_err(|e| Status::internal(e.to_string()))?;
        Ok(Response::new(HeartbeatResponse {
            runner_id: runner.id.to_string(),
            runner_name: runner.name,
            certificate_expires_at: runner
                .cert_expires_at
                .map_or(0, time::OffsetDateTime::unix_timestamp),
        }))
    }

    async fn renew(
        &self,
        request: Request<RenewRequest>,
    ) -> Result<Response<RenewResponse>, Status> {
        let runner = self.runner(&request).await?;
        if runner.source != RunnerSource::Huxley {
            return Err(Status::failed_precondition(
                "external runners get their certificates from their own CA",
            ));
        }
        let csr = request.into_inner().csr_pem;
        let mut tx = begin_scoped(&self.db, &OrgScope::System)
            .await
            .map_err(|e| Status::internal(e.to_string()))?;
        let issued = registry::renew(&mut tx, &self.crypto, runner.id, &csr)
            .await
            .map_err(|e| Status::invalid_argument(e.to_string()))?;
        tx.commit()
            .await
            .map_err(|e| Status::internal(e.to_string()))?;
        Ok(Response::new(RenewResponse {
            certificate_pem: issued.cert_pem,
            expires_at: issued.expires_at.unix_timestamp(),
        }))
    }

    async fn next(&self, request: Request<NextRequest>) -> Result<Response<NextResponse>, Status> {
        let runner = self.runner(&request).await?;
        let languages: Vec<huxley_scripts::Language> = request
            .into_inner()
            .languages
            .iter()
            .filter_map(|l| serde_json::from_value(serde_json::Value::String(l.clone())).ok())
            .collect();
        if languages.is_empty() {
            return Ok(Response::new(NextResponse { task: None }));
        }
        let claimed = self
            .engine
            .claim_script(runner.id, &languages)
            .await
            .map_err(|e| Status::internal(e.to_string()))?;
        Ok(Response::new(NextResponse {
            task: claimed.map(|t| Task {
                id: t.id.to_string(),
                language: serde_json::to_value(t.language)
                    .ok()
                    .and_then(|v| v.as_str().map(str::to_owned))
                    .unwrap_or_default(),
                source: t.source,
                inputs_json: t.inputs.to_string(),
                timeout_secs: t.timeout_secs,
                env: t.env.into_iter().collect(),
                redact: t.redact,
            }),
        }))
    }

    async fn log(&self, request: Request<LogRequest>) -> Result<Response<LogResponse>, Status> {
        let runner = self.runner(&request).await?;
        let body = request.into_inner();
        let id: uuid::Uuid = body
            .task_id
            .parse()
            .map_err(|_| Status::invalid_argument("task_id isn't a UUID"))?;
        match self
            .engine
            .append_script_logs(runner.id, id, &body.text)
            .await
        {
            Ok(()) => Ok(Response::new(LogResponse {})),
            Err(huxley_engine::script::ReportError::NotYours) => {
                Err(Status::not_found("no such task is running on this runner"))
            }
            Err(e) => Err(Status::internal(e.to_string())),
        }
    }

    async fn report(
        &self,
        request: Request<ReportRequest>,
    ) -> Result<Response<ReportResponse>, Status> {
        let runner = self.runner(&request).await?;
        let body = request.into_inner();
        let id: uuid::Uuid = body
            .task_id
            .parse()
            .map_err(|_| Status::invalid_argument("task_id isn't a UUID"))?;
        let output = match body.output_json.as_deref() {
            Some(text) => Some(
                serde_json::from_str(text)
                    .map_err(|_| Status::invalid_argument("output_json isn't JSON"))?,
            ),
            None => None,
        };
        let report = huxley_engine::script::Report {
            succeeded: body.succeeded,
            output,
            error: body.error,
            exit_code: body.exit_code,
            logs: body.logs,
        };
        match self.engine.report_script(runner.id, id, &report).await {
            Ok(()) => Ok(Response::new(ReportResponse {})),
            Err(huxley_engine::script::ReportError::NotYours) => {
                Err(Status::not_found("no such task is running on this runner"))
            }
            Err(e) => Err(Status::internal(e.to_string())),
        }
    }
}

/// Serves the gateway on `listener` until `shutdown`.
pub async fn serve(
    gateway: Gateway,
    listener: tokio::net::TcpListener,
    tls: &GatewayTls,
    shutdown: CancellationToken,
) -> Result<(), tonic::transport::Error> {
    crate::install_crypto_provider();
    let config = ServerTlsConfig::new()
        .identity(Identity::from_pem(
            &tls.server_cert_pem,
            &tls.server_key_pem,
        ))
        .client_ca_root(Certificate::from_pem(&tls.client_ca_pem));
    Server::builder()
        .tls_config(config)?
        .add_service(RunnerServer::new(gateway))
        .serve_with_incoming_shutdown(
            tokio_stream::wrappers::TcpListenerStream::new(listener),
            shutdown.cancelled_owned(),
        )
        .await
}

/// Where a runner connects, and the credentials it uses.
pub struct ClientTls {
    pub ca_pem: String,
    pub cert_pem: String,
    pub key_pem: String,
    /// The name in the gateway's certificate, e.g. `localhost`.
    pub domain: String,
}

impl std::fmt::Debug for ClientTls {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClientTls")
            .field("domain", &self.domain)
            .finish_non_exhaustive()
    }
}

/// A runner's connection to the gateway at `endpoint` (`https://host:port`).
pub async fn connect(
    endpoint: &str,
    tls: &ClientTls,
) -> Result<
    crate::proto::runner_client::RunnerClient<tonic::transport::Channel>,
    tonic::transport::Error,
> {
    crate::install_crypto_provider();
    let config = tonic::transport::ClientTlsConfig::new()
        .ca_certificate(Certificate::from_pem(&tls.ca_pem))
        .identity(Identity::from_pem(&tls.cert_pem, &tls.key_pem))
        .domain_name(&tls.domain);
    let channel = tonic::transport::Endpoint::from_shared(endpoint.to_owned())?
        .tls_config(config)?
        .connect()
        .await?;
    Ok(crate::proto::runner_client::RunnerClient::new(channel))
}

/// The address a gateway listener ended up on (for tests binding port 0).
pub fn local_addr(listener: &tokio::net::TcpListener) -> std::io::Result<SocketAddr> {
    listener.local_addr()
}
