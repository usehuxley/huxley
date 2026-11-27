//! SIEM audit streaming (ADR-087): audit records are shipped, in chain
//! order and as they're sealed, to an HTTPS destination: a generic NDJSON
//! webhook or Splunk's HTTP Event Collector. Each stream keeps its place in
//! the chain and backs off while its destination fails. Several schedulers
//! may run the shipper: each stream is shipped by one at a time.

use std::time::Duration;

use huxley_core::RequestContext;
use huxley_crypto::Binding;
use huxley_db::audit::{self, AuditRecord};
use huxley_db::{OrgScope, begin_scoped};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::PgConnection;
use time::OffsetDateTime;
use tokio_util::sync::CancellationToken;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::{Engine, EngineError, EngineResult};

const SECRET_PURPOSE: &str = "audit_stream_secret";
/// Records per request.
const BATCH: i64 = 500;
/// Requests per stream per pass, so one busy stream doesn't hold the rest.
const MAX_BATCHES: usize = 20;
/// The longest a failing stream waits before trying again.
const MAX_BACKOFF_SECS: i64 = 600;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema, sqlx::Type)]
#[serde(rename_all = "snake_case")]
#[sqlx(type_name = "text", rename_all = "snake_case")]
pub enum StreamKind {
    /// Records as NDJSON, one per line, sent to the URL with POST.
    Webhook,
    /// Splunk's HTTP Event Collector (`/services/collector/event`).
    SplunkHec,
}

/// A stream as it's shown: never its secret.
#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct AuditStream {
    pub id: Uuid,
    pub name: String,
    pub kind: StreamKind,
    pub url: String,
    pub header_name: Option<String>,
    /// Whether a token or header value is set.
    pub has_secret: bool,
    pub enabled: bool,
    /// The last record shipped.
    pub last_seq: i64,
    #[serde(with = "time::serde::rfc3339::option")]
    pub last_shipped_at: Option<OffsetDateTime>,
    /// Failed attempts in a row.
    pub failures: i32,
    pub last_error: Option<String>,
    /// Sealed records not shipped yet.
    pub behind: i64,
}

/// A new stream, or changes to one.
#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StreamInput {
    pub name: String,
    pub kind: StreamKind,
    pub url: String,
    /// Webhook only: the header the secret goes in (default Authorization).
    #[serde(default)]
    pub header_name: Option<String>,
    /// The token (Splunk) or header value (webhook). Left out when
    /// changing a stream, the current one stays.
    #[serde(default)]
    pub secret: Option<String>,
    #[serde(default = "yes")]
    pub enabled: bool,
    /// New streams only: ship the whole audit log, not just what's recorded
    /// from now on.
    #[serde(default)]
    pub from_start: bool,
}

const fn yes() -> bool {
    true
}

macro_rules! select_streams {
    ($rest:literal) => {
        concat!(
            "SELECT s.id, s.name, s.kind, s.url, s.header_name, s.secret_sealed IS NOT NULL AS has_secret, \
                    s.enabled, s.last_seq, s.last_shipped_at, s.failures, s.last_error, \
                    greatest((SELECT seq FROM audit_chain_head WHERE id = 1) - s.last_seq, 0) AS behind \
               FROM audit_streams s ",
            $rest
        )
    };
}

pub async fn list(conn: &mut PgConnection) -> EngineResult<Vec<AuditStream>> {
    Ok(sqlx::query_as(select_streams!("ORDER BY s.name"))
        .fetch_all(conn)
        .await?)
}

pub async fn get(conn: &mut PgConnection, id: Uuid) -> EngineResult<Option<AuditStream>> {
    Ok(sqlx::query_as(select_streams!("WHERE s.id = $1"))
        .bind(id)
        .fetch_optional(conn)
        .await?)
}

fn invalid(field: &str, what: &str) -> EngineError {
    EngineError::Invalid(format!("{field}: {what}"))
}

fn check(input: &StreamInput) -> EngineResult<()> {
    let name = input.name.trim();
    if name.is_empty() || name.chars().count() > 100 {
        return Err(invalid("name", "a name of up to 100 characters"));
    }
    let url = reqwest::Url::parse(input.url.trim())
        .map_err(|_| invalid("url", "an http or https URL"))?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return Err(invalid("url", "an http or https URL"));
    }
    if let Some(h) = input.header_name.as_deref().map(str::trim)
        && !h.is_empty()
        && reqwest::header::HeaderName::from_bytes(h.as_bytes()).is_err()
    {
        return Err(invalid(
            "headerName",
            "a header name, such as Authorization",
        ));
    }
    Ok(())
}

impl Engine {
    async fn seal_secret(&self, id: Uuid, secret: &str) -> EngineResult<Vec<u8>> {
        let mut tx = begin_scoped(self.db(), &OrgScope::System).await?;
        let root = huxley_orgs::root(&mut tx)
            .await
            .map_err(|e| EngineError::Document(e.to_string()))?
            .ok_or_else(|| invalid("secret", "an instance with a root organization"))?;
        tx.commit().await?;
        Ok(self
            .crypto
            .encrypt(
                root.id,
                Binding {
                    purpose: SECRET_PURPOSE,
                    record: id,
                },
                secret.as_bytes(),
            )
            .await?)
    }

    /// Adds a stream.
    pub async fn create_audit_stream(
        &self,
        ctx: &RequestContext,
        input: &StreamInput,
    ) -> EngineResult<AuditStream> {
        check(input)?;
        let id = huxley_core::new_id();
        let sealed = match input.secret.as_deref().map(str::trim) {
            Some(s) if !s.is_empty() => Some(self.seal_secret(id, s).await?),
            _ => None,
        };
        let mut tx = begin_scoped(self.db(), &OrgScope::System).await?;
        sqlx::query(
            "INSERT INTO audit_streams (id, name, kind, url, header_name, secret_sealed, enabled, last_seq, created_by) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, \
                     CASE WHEN $8 THEN 0 ELSE (SELECT seq FROM audit_chain_head WHERE id = 1) END, $9)",
        )
        .bind(id)
        .bind(input.name.trim())
        .bind(input.kind)
        .bind(input.url.trim())
        .bind(header(input))
        .bind(sealed)
        .bind(input.enabled)
        .bind(input.from_start)
        .bind(ctx.actor.id())
        .execute(&mut *tx)
        .await?;
        audit::record(
            &mut tx,
            ctx,
            AuditRecord::new("audit_stream.created")
                .target("audit_stream", id)
                .details(json!({ "name": input.name.trim(), "kind": input.kind, "url": input.url.trim(), "fromStart": input.from_start })),
        )
        .await?;
        let stream = get(&mut tx, id).await?.expect("just added");
        tx.commit().await?;
        Ok(stream)
    }

    /// Changes a stream. `None` when there's no such stream.
    pub async fn update_audit_stream(
        &self,
        ctx: &RequestContext,
        id: Uuid,
        input: &StreamInput,
    ) -> EngineResult<Option<AuditStream>> {
        check(input)?;
        let sealed = match input.secret.as_deref().map(str::trim) {
            Some(s) if !s.is_empty() => Some(self.seal_secret(id, s).await?),
            _ => None,
        };
        let mut tx = begin_scoped(self.db(), &OrgScope::System).await?;
        // Re-enabling or fixing a stream tries it again straight away.
        let changed = sqlx::query(
            "UPDATE audit_streams SET name = $2, kind = $3, url = $4, header_name = $5, \
                    secret_sealed = COALESCE($6, secret_sealed), enabled = $7, \
                    next_attempt_at = now(), updated_at = now() WHERE id = $1",
        )
        .bind(id)
        .bind(input.name.trim())
        .bind(input.kind)
        .bind(input.url.trim())
        .bind(header(input))
        .bind(sealed)
        .bind(input.enabled)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        if changed == 0 {
            return Ok(None);
        }
        audit::record(
            &mut tx,
            ctx,
            AuditRecord::new("audit_stream.changed")
                .target("audit_stream", id)
                .details(json!({
                    "name": input.name.trim(), "kind": input.kind, "url": input.url.trim(),
                    "enabled": input.enabled, "secretChanged": input.secret.as_deref().is_some_and(|s| !s.trim().is_empty()),
                })),
        )
        .await?;
        let stream = get(&mut tx, id).await?;
        tx.commit().await?;
        Ok(stream)
    }

    /// Removes a stream. False when there was none.
    pub async fn delete_audit_stream(&self, ctx: &RequestContext, id: Uuid) -> EngineResult<bool> {
        let mut tx = begin_scoped(self.db(), &OrgScope::System).await?;
        let name: Option<String> =
            sqlx::query_scalar("DELETE FROM audit_streams WHERE id = $1 RETURNING name")
                .bind(id)
                .fetch_optional(&mut *tx)
                .await?;
        let Some(name) = name else { return Ok(false) };
        audit::record(
            &mut tx,
            ctx,
            AuditRecord::new("audit_stream.deleted")
                .target("audit_stream", id)
                .details(json!({ "name": name })),
        )
        .await?;
        tx.commit().await?;
        Ok(true)
    }

    /// Sends one test event to a stream's destination, outside the chain.
    /// `None` when there's no such stream; otherwise what went wrong, if
    /// anything.
    pub async fn test_audit_stream(&self, id: Uuid) -> EngineResult<Option<Result<(), String>>> {
        let mut tx = begin_scoped(self.db(), &OrgScope::System).await?;
        let Some(dest) = destination(&mut tx, id).await? else {
            return Ok(None);
        };
        tx.commit().await?;
        let event = json!({
            "test": true,
            "action": "audit_stream.test",
            "occurredAt": OffsetDateTime::now_utc().format(&time::format_description::well_known::Rfc3339).unwrap_or_default(),
            "message": "A test event from Huxley. It isn't part of the audit log.",
        });
        let secret = self.open_secret(&dest).await?;
        Ok(Some(self.post(&dest, secret.as_deref(), &[event]).await))
    }

    async fn open_secret(&self, dest: &Destination) -> EngineResult<Option<String>> {
        let Some(sealed) = dest.secret_sealed.as_deref() else {
            return Ok(None);
        };
        let plain = self
            .crypto
            .decrypt(
                Binding {
                    purpose: SECRET_PURPOSE,
                    record: dest.id,
                },
                sealed,
            )
            .await?;
        Ok(Some(String::from_utf8_lossy(&plain).into_owned()))
    }

    async fn post(
        &self,
        dest: &Destination,
        secret: Option<&str>,
        records: &[Value],
    ) -> Result<(), String> {
        let (url, body, content_type) = match dest.kind {
            StreamKind::Webhook => {
                let mut body = String::new();
                for r in records {
                    body.push_str(&r.to_string());
                    body.push('\n');
                }
                (dest.url.clone(), body, "application/x-ndjson")
            }
            StreamKind::SplunkHec => {
                let mut body = String::new();
                for r in records {
                    let time = r["occurredAt"]
                        .as_str()
                        .and_then(|t| {
                            OffsetDateTime::parse(t, &time::format_description::well_known::Rfc3339)
                                .ok()
                        })
                        .map_or_else(
                            || OffsetDateTime::now_utc().unix_timestamp(),
                            OffsetDateTime::unix_timestamp,
                        );
                    body.push_str(
                        &json!({ "time": time, "source": "huxley", "sourcetype": "huxley:audit", "event": r })
                            .to_string(),
                    );
                    body.push('\n');
                }
                (hec_url(&dest.url), body, "application/json")
            }
        };
        let mut request = self
            .egress
            .client
            .post(&url)
            .timeout(Duration::from_secs(20))
            .header(reqwest::header::CONTENT_TYPE, content_type)
            .body(body);
        if let Some(secret) = secret {
            request = match dest.kind {
                StreamKind::SplunkHec => {
                    request.header(reqwest::header::AUTHORIZATION, format!("Splunk {secret}"))
                }
                StreamKind::Webhook => request.header(
                    dest.header_name.as_deref().unwrap_or("Authorization"),
                    secret,
                ),
            };
        }
        let response = request.send().await.map_err(|e| describe(&e))?;
        let status = response.status();
        if status.is_success() {
            return Ok(());
        }
        let text = response.text().await.unwrap_or_default();
        let text: String = text.chars().take(300).collect();
        Err(format!("{} answered {status}: {text}", host_of(&url))
            .trim_end_matches(": ")
            .to_owned())
    }

    /// Ships what's waiting on every enabled stream that's due. Seals what's
    /// committed first. Returns how many records were shipped.
    pub async fn ship_audit_streams(&self) -> EngineResult<u64> {
        {
            let mut conn = self.db().acquire().await?;
            audit::seal(&mut conn).await?;
        }
        let mut tx = begin_scoped(self.db(), &OrgScope::System).await?;
        let due: Vec<Uuid> = sqlx::query_scalar(
            "SELECT id FROM audit_streams WHERE enabled AND next_attempt_at <= now() \
               AND last_seq < (SELECT seq FROM audit_chain_head WHERE id = 1) ORDER BY id",
        )
        .fetch_all(&mut *tx)
        .await?;
        tx.commit().await?;
        let mut shipped = 0;
        for id in due {
            shipped += self.ship_stream(id).await?;
        }
        Ok(shipped)
    }

    async fn ship_stream(&self, id: Uuid) -> EngineResult<u64> {
        let mut shipped = 0;
        for _ in 0..MAX_BATCHES {
            let mut tx = begin_scoped(self.db(), &OrgScope::System).await?;
            // Another scheduler shipping this stream has it locked.
            let Some(dest) = destination_locked(&mut tx, id).await? else {
                return Ok(shipped);
            };
            let records: Vec<(i64, Value)> = sqlx::query_as(RECORDS)
                .bind(dest.last_seq)
                .bind(BATCH)
                .fetch_all(&mut *tx)
                .await?;
            let Some(&(last, _)) = records.last() else {
                return Ok(shipped);
            };
            let full = records.len() == usize::try_from(BATCH).unwrap_or(usize::MAX);
            let secret = self.open_secret(&dest).await?;
            let values: Vec<Value> = records.into_iter().map(|(_, r)| r).collect();
            match self.post(&dest, secret.as_deref(), &values).await {
                Ok(()) => {
                    sqlx::query(
                        "UPDATE audit_streams SET last_seq = $2, last_shipped_at = now(), failures = 0, \
                                last_error = NULL, next_attempt_at = now() WHERE id = $1",
                    )
                    .bind(id)
                    .bind(last)
                    .execute(&mut *tx)
                    .await?;
                    tx.commit().await?;
                    shipped += values.len() as u64;
                    if !full {
                        return Ok(shipped);
                    }
                }
                Err(error) => {
                    sqlx::query(
                        "UPDATE audit_streams SET failures = failures + 1, last_error = $2, \
                                next_attempt_at = now() + make_interval(secs => least(5 * power(2, least(failures, 10)), $3)) \
                          WHERE id = $1",
                    )
                    .bind(id)
                    .bind(&error)
                    .bind(MAX_BACKOFF_SECS)
                    .execute(&mut *tx)
                    .await?;
                    tx.commit().await?;
                    tracing::warn!(stream = %id, %error, "shipping audit records failed");
                    return Ok(shipped);
                }
            }
        }
        Ok(shipped)
    }
}

/// Sealed records after `$1`, oldest first, as the export writes them.
const RECORDS: &str = "SELECT e.seq, jsonb_build_object( \
        'seq', e.seq, 'id', e.id, \
        'occurredAt', to_char(e.occurred_at AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS.US\"Z\"'), \
        'actorType', e.actor_type, 'actorId', e.actor_id, 'actorName', u.display_name, \
        'via', e.via, 'ip', host(e.ip), 'userAgent', e.user_agent, 'requestId', e.request_id, \
        'action', e.action, 'orgId', e.org_id, 'orgName', o.name, \
        'targetType', e.target_type, 'targetId', e.target_id, 'details', e.details, \
        'prevHash', encode(e.prev_hash, 'hex'), 'hash', encode(e.hash, 'hex')) \
       FROM audit_events e \
       LEFT JOIN users u ON u.id = e.actor_id \
       LEFT JOIN organizations o ON o.id = e.org_id \
      WHERE e.seq > $1 ORDER BY e.seq LIMIT $2";

struct Destination {
    id: Uuid,
    kind: StreamKind,
    url: String,
    header_name: Option<String>,
    secret_sealed: Option<Vec<u8>>,
    last_seq: i64,
}

type DestinationRow = (
    Uuid,
    StreamKind,
    String,
    Option<String>,
    Option<Vec<u8>>,
    i64,
);

fn to_destination(
    (id, kind, url, header_name, secret_sealed, last_seq): DestinationRow,
) -> Destination {
    Destination {
        id,
        kind,
        url,
        header_name,
        secret_sealed,
        last_seq,
    }
}

async fn destination(conn: &mut PgConnection, id: Uuid) -> EngineResult<Option<Destination>> {
    let row: Option<DestinationRow> = sqlx::query_as(
        "SELECT id, kind, url, header_name, secret_sealed, last_seq FROM audit_streams WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(conn)
    .await?;
    Ok(row.map(to_destination))
}

async fn destination_locked(
    conn: &mut PgConnection,
    id: Uuid,
) -> EngineResult<Option<Destination>> {
    let row: Option<DestinationRow> = sqlx::query_as(
        "SELECT id, kind, url, header_name, secret_sealed, last_seq FROM audit_streams \
          WHERE id = $1 AND enabled FOR UPDATE SKIP LOCKED",
    )
    .bind(id)
    .fetch_optional(conn)
    .await?;
    Ok(row.map(to_destination))
}

fn header(input: &StreamInput) -> Option<String> {
    match input.kind {
        StreamKind::SplunkHec => None,
        StreamKind::Webhook => input
            .header_name
            .as_deref()
            .map(str::trim)
            .filter(|h| !h.is_empty())
            .map(str::to_owned),
    }
}

/// A Splunk URL given as just the collector's address gets the event
/// endpoint's path.
fn hec_url(url: &str) -> String {
    let trimmed = url.trim_end_matches('/');
    if trimmed.contains("/services/collector") {
        trimmed.to_owned()
    } else {
        format!("{trimmed}/services/collector/event")
    }
}

fn host_of(url: &str) -> String {
    reqwest::Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(str::to_owned))
        .unwrap_or_else(|| "the destination".to_owned())
}

/// A failed request, said plainly (with the egress policy's refusal when
/// that's why).
fn describe(e: &reqwest::Error) -> String {
    let mut message = e.to_string();
    let mut source = std::error::Error::source(e);
    while let Some(s) = source {
        message = format!("{message}: {s}");
        source = s.source();
    }
    message
}

/// Ships audit records every few seconds until `shutdown`.
pub async fn run_shipper(engine: Engine, shutdown: CancellationToken) {
    loop {
        tokio::select! {
            () = shutdown.cancelled() => return,
            () = tokio::time::sleep(Duration::from_secs(5)) => {}
        }
        if let Err(e) = engine.ship_audit_streams().await {
            tracing::warn!(error = %e, "audit streaming failed");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splunk_urls_get_the_event_path() {
        assert_eq!(
            hec_url("https://splunk.example:8088"),
            "https://splunk.example:8088/services/collector/event"
        );
        assert_eq!(
            hec_url("https://splunk.example:8088/services/collector/raw"),
            "https://splunk.example:8088/services/collector/raw"
        );
    }

    #[test]
    fn streams_are_checked() {
        let ok = StreamInput {
            name: "SIEM".into(),
            kind: StreamKind::Webhook,
            url: "https://siem.example/ingest".into(),
            header_name: Some("X-Api-Key".into()),
            secret: None,
            enabled: true,
            from_start: false,
        };
        assert!(check(&ok).is_ok());
        assert!(
            check(&StreamInput {
                url: "ftp://x".into(),
                ..ok.clone()
            })
            .is_err()
        );
        assert!(
            check(&StreamInput {
                name: " ".into(),
                ..ok.clone()
            })
            .is_err()
        );
        assert!(
            check(&StreamInput {
                header_name: Some("bad header".into()),
                ..ok
            })
            .is_err()
        );
    }
}
