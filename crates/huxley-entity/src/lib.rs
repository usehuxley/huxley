//! Extensions every entity shares (design doc §8): **custom fields** (typed,
//! per entity type, optionally scoped to an org subtree, values stored as
//! JSONB on the entity row), **tags**, and **attachments** (bytes in object
//! storage, metadata in Postgres).
//!
//! An entity type opts in by being listed in [`TYPES`] with its table (which
//! must have `id` and a `custom_fields jsonb` column) and a way to find its
//! owning org, which decides RLS and permissions. Organizations come first;
//! modules add theirs.

pub mod attachments;
pub mod custom_fields;
pub mod tags;

use serde::Serialize;
use sqlx::PgConnection;
use utoipa::ToSchema;
use uuid::Uuid;

/// An entity type that supports custom fields and tags.
#[derive(Debug, Clone, Copy, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct EntityType {
    pub key: &'static str,
    pub label: &'static str,
    #[serde(skip)]
    table: &'static str,
}

pub const ORGANIZATION: &str = "organization";
/// A contact (the `crm` module).
pub const CONTACT: &str = "contact";
/// A site (the `crm` module).
pub const SITE: &str = "site";
/// A ticket (the `servicedesk` module).
pub const TICKET: &str = "ticket";

pub const TYPES: &[EntityType] = &[
    EntityType {
        key: ORGANIZATION,
        label: "Organization",
        table: "organizations",
    },
    EntityType {
        key: CONTACT,
        label: "Contact",
        table: "crm_contacts",
    },
    EntityType {
        key: SITE,
        label: "Site",
        table: "crm_sites",
    },
    EntityType {
        key: TICKET,
        label: "Ticket",
        table: "sd_tickets",
    },
];

/// The feature module an entity type belongs to (`None`: the core).
pub fn module_of(entity_type: &str) -> Option<&'static str> {
    match entity_type {
        CONTACT | SITE => Some("crm"),
        TICKET => Some("servicedesk"),
        _ => None,
    }
}

pub fn entity_type(key: &str) -> EntityResult<&'static EntityType> {
    TYPES
        .iter()
        .find(|t| t.key == key)
        .ok_or_else(|| EntityError::UnknownEntityType(key.to_owned()))
}

#[derive(Debug, thiserror::Error)]
pub enum EntityError {
    #[error("unknown entity type `{0}`")]
    UnknownEntityType(String),
    #[error("not found")]
    NotFound,
    #[error("{field}: {message}")]
    Invalid { field: String, message: String },
    #[error("a field with this key already exists")]
    KeyTaken,
    #[error("a tag with this name already exists")]
    TagNameTaken,
    #[error("the file is larger than the {0} MB limit")]
    TooLarge(u64),
    #[error(transparent)]
    Storage(#[from] huxley_storage::StorageError),
    #[error(transparent)]
    Db(#[from] sqlx::Error),
}

impl EntityError {
    pub(crate) fn invalid(field: impl Into<String>, message: impl Into<String>) -> Self {
        Self::Invalid {
            field: field.into(),
            message: message.into(),
        }
    }
}

pub type EntityResult<T> = Result<T, EntityError>;

/// An entity's owning org: its id and ltree path. For permission checks and
/// RLS. Run in system scope (the caller checks access first).
pub async fn owning_org(
    conn: &mut PgConnection,
    entity_type: &str,
    id: Uuid,
) -> EntityResult<(Uuid, String)> {
    let ty = self::entity_type(entity_type)?;
    let row: Option<(Uuid, String)> = match ty.key {
        ORGANIZATION => {
            sqlx::query_as("SELECT id, path::text FROM organizations WHERE id = $1")
                .bind(id)
                .fetch_optional(conn)
                .await?
        }
        CONTACT | SITE | TICKET => {
            let table = ty.table;
            sqlx::query_as(sqlx::AssertSqlSafe(format!(
                "SELECT o.id, o.path::text FROM {table} r JOIN organizations o ON o.id = r.org_id \
                  WHERE r.id = $1"
            )))
            .bind(id)
            .fetch_optional(conn)
            .await?
        }
        other => return Err(EntityError::UnknownEntityType(other.to_owned())),
    };
    row.ok_or(EntityError::NotFound)
}
