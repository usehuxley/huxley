//! Attachments: files on records, stored in object storage.
//!
//! Upload writes the object first and the row second, so a row never points
//! at a missing object; a failed insert deletes the object again. Deleting
//! soft-deletes the row and removes the object.

use bytes::Bytes;
use huxley_core::RequestContext;
use huxley_db::audit::{self, AuditRecord};
use huxley_storage::Storage;
use serde::Serialize;
use serde_json::json;
use sha2::{Digest as _, Sha256};
use sqlx::PgConnection;
use time::OffsetDateTime;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::{EntityError, EntityResult, entity_type, owning_org};

#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct Attachment {
    pub id: Uuid,
    pub entity_type: String,
    pub entity_id: Uuid,
    pub filename: String,
    pub content_type: String,
    pub size_bytes: i64,
    /// Hex SHA-256 of the content.
    pub sha256: String,
    pub created_by: Option<Uuid>,
    pub created_by_name: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(skip)]
    pub storage_key: String,
    #[serde(skip)]
    pub org_id: Uuid,
}

const SELECT: &str = "SELECT a.id, a.entity_type, a.entity_id, a.filename, a.content_type, a.size_bytes, \
    encode(a.sha256, 'hex') AS sha256, a.created_by, u.display_name AS created_by_name, a.created_at, \
    a.storage_key, a.org_id \
    FROM attachments a LEFT JOIN users u ON u.id = a.created_by";

/// A safe file name: no path, no control characters, at most 255 bytes.
pub fn clean_filename(name: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or_default();
    let cleaned: String = base
        .chars()
        .filter(|c| !c.is_control())
        .collect::<String>()
        .trim()
        .trim_start_matches('.')
        .to_owned();
    let mut out = String::new();
    for c in cleaned.chars() {
        if out.len() + c.len_utf8() > 255 {
            break;
        }
        out.push(c);
    }
    if out.is_empty() {
        "file".to_owned()
    } else {
        out
    }
}

fn clean_content_type(declared: Option<&str>) -> String {
    match declared.map(str::trim) {
        Some(ct)
            if !ct.is_empty()
                && ct.len() <= 255
                && ct.contains('/')
                && !ct.chars().any(char::is_control) =>
        {
            ct.to_ascii_lowercase()
        }
        _ => "application/octet-stream".into(),
    }
}

/// Content types browsers may be told as-is when downloading. Everything
/// else (HTML, SVG, JavaScript, …) goes out as `application/octet-stream`.
pub fn safe_content_type(stored: &str) -> &str {
    const SAFE: &[&str] = &[
        "image/png",
        "image/jpeg",
        "image/gif",
        "image/webp",
        "application/pdf",
        "text/plain",
        "text/csv",
        "application/zip",
        "application/json",
    ];
    if SAFE.contains(&stored) {
        stored
    } else {
        "application/octet-stream"
    }
}

fn db_err(e: huxley_db::DbError) -> EntityError {
    match e {
        huxley_db::DbError::Sqlx(e) => EntityError::Db(e),
        other => EntityError::Db(sqlx::Error::Protocol(other.to_string())),
    }
}

/// A file to attach.
#[derive(Debug)]
pub struct Upload<'a> {
    pub entity_type: &'a str,
    pub entity_id: Uuid,
    pub filename: &'a str,
    pub content_type: Option<&'a str>,
    pub data: Bytes,
}

/// Stores a file on a record. Run in system scope after checking the
/// record's update permission.
pub async fn upload(
    conn: &mut PgConnection,
    storage: &Storage,
    ctx: &RequestContext,
    file: Upload<'_>,
) -> EntityResult<Attachment> {
    let Upload {
        entity_type,
        entity_id,
        filename,
        content_type,
        data,
    } = file;
    let ty = self::entity_type(entity_type)?;
    if data.len() as u64 > storage.max_upload_bytes() {
        return Err(EntityError::TooLarge(
            storage.max_upload_bytes() / (1024 * 1024),
        ));
    }
    let (org_id, _) = owning_org(conn, entity_type, entity_id).await?;
    let id = huxley_core::new_id();
    let key = format!("attachments/{org_id}/{id}");
    let sha = Sha256::digest(&data).to_vec();
    let size = i64::try_from(data.len()).unwrap_or(i64::MAX);
    let filename = clean_filename(filename);
    let content_type = clean_content_type(content_type);
    storage.put(&key, data).await?;
    let inserted = sqlx::query(
        "INSERT INTO attachments (id, entity_type, entity_id, org_id, filename, content_type, size_bytes, sha256, storage_key, created_by) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)",
    )
    .bind(id)
    .bind(ty.key)
    .bind(entity_id)
    .bind(org_id)
    .bind(&filename)
    .bind(&content_type)
    .bind(size)
    .bind(&sha)
    .bind(&key)
    .bind(ctx.actor.id())
    .execute(&mut *conn)
    .await;
    if let Err(e) = inserted {
        let _ = storage.delete(&key).await;
        return Err(e.into());
    }
    audit::record(
        conn,
        ctx,
        AuditRecord::new("entity.attachment_added")
            .org(org_id)
            .target(ty.key, entity_id)
            .details(json!({ "attachment": id, "filename": filename, "size": size })),
    )
    .await
    .map_err(db_err)?;
    huxley_db::outbox::emit(
        conn,
        huxley_db::outbox::DomainEvent {
            event_type: &format!("{}.attachment_added", ty.key),
            org_id: Some(org_id),
            subject_type: ty.key,
            subject_id: entity_id,
            payload: json!({ "attachment": id, "filename": filename }),
        },
    )
    .await
    .map_err(db_err)?;
    get(conn, id).await
}

pub async fn get(conn: &mut PgConnection, id: Uuid) -> EntityResult<Attachment> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{SELECT} WHERE a.id = $1 AND a.deleted_at IS NULL"
    )))
    .bind(id)
    .fetch_optional(conn)
    .await?
    .ok_or(EntityError::NotFound)
}

/// A record's attachments, newest first.
pub async fn of_entity(
    conn: &mut PgConnection,
    entity_type: &str,
    entity_id: Uuid,
) -> EntityResult<Vec<Attachment>> {
    self::entity_type(entity_type)?;
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{SELECT} WHERE a.entity_type = $1 AND a.entity_id = $2 AND a.deleted_at IS NULL ORDER BY a.id DESC"
    )))
    .bind(entity_type)
    .bind(entity_id)
    .fetch_all(conn)
    .await?)
}

/// The file's bytes.
pub async fn content(storage: &Storage, attachment: &Attachment) -> EntityResult<Bytes> {
    Ok(storage.get(&attachment.storage_key).await?)
}

/// Removes an attachment. Run after checking the record's update permission.
pub async fn delete(
    conn: &mut PgConnection,
    storage: &Storage,
    ctx: &RequestContext,
    id: Uuid,
) -> EntityResult<()> {
    let attachment = get(conn, id).await?;
    sqlx::query("UPDATE attachments SET deleted_at = now() WHERE id = $1")
        .bind(id)
        .execute(&mut *conn)
        .await?;
    audit::record(
        conn,
        ctx,
        AuditRecord::new("entity.attachment_removed")
            .org(attachment.org_id)
            .target(&attachment.entity_type, attachment.entity_id)
            .details(json!({ "attachment": id, "filename": attachment.filename })),
    )
    .await
    .map_err(db_err)?;
    // The row is gone from view either way; a leftover object is harmless
    // (and unreachable), so a storage hiccup shouldn't fail the request.
    if let Err(error) = storage.delete(&attachment.storage_key).await {
        tracing::warn!(%error, attachment = %id, "failed to delete an attachment's object");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filenames_lose_paths_and_control_characters() {
        assert_eq!(clean_filename("../../etc/passwd"), "passwd");
        assert_eq!(clean_filename("C:\\Users\\me\\report.pdf"), "report.pdf");
        assert_eq!(clean_filename("evil\u{0}\nname.txt"), "evilname.txt");
        assert_eq!(clean_filename(".htaccess"), "htaccess");
        assert_eq!(clean_filename("   "), "file");
        assert!(clean_filename(&"é".repeat(300)).len() <= 255);
    }

    #[test]
    fn only_safe_types_pass_through() {
        assert_eq!(safe_content_type("image/png"), "image/png");
        assert_eq!(safe_content_type("text/html"), "application/octet-stream");
        assert_eq!(
            safe_content_type("image/svg+xml"),
            "application/octet-stream"
        );
        assert_eq!(clean_content_type(Some("Text/Plain")), "text/plain");
        assert_eq!(
            clean_content_type(Some("nonsense")),
            "application/octet-stream"
        );
        assert_eq!(clean_content_type(None), "application/octet-stream");
    }
}
