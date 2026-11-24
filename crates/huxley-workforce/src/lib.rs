//! The `workforce` module (design doc §8.15): the skills and
//! certifications catalogs, what people have, what work requires, and who
//! is qualified for it. Records are instance-wide (they describe the MSP's
//! people); the API checks permissions and that the module is on.

pub mod catalog;
pub mod people;
pub mod requirements;

/// The module's key in the registry.
pub const MODULE: &str = "workforce";

/// Skill levels, 1 to 4.
pub const LEVELS: &[&str] = &["learning", "working", "proficient", "expert"];

#[derive(Debug, thiserror::Error)]
pub enum WorkforceError {
    #[error("not found")]
    NotFound,
    #[error("{field}: {message}")]
    Invalid { field: String, message: String },
    #[error("it was changed by someone else; reload and try again")]
    VersionConflict,
    #[error(transparent)]
    Db(#[from] huxley_db::DbError),
    #[error(transparent)]
    Sqlx(#[from] sqlx::Error),
}

impl WorkforceError {
    pub(crate) fn invalid(field: &str, message: impl Into<String>) -> Self {
        Self::Invalid {
            field: field.to_owned(),
            message: message.into(),
        }
    }
}

pub type WorkforceResult<T> = Result<T, WorkforceError>;

/// Trims text; empty becomes `None`. Rejects text over `max` characters and
/// control characters.
pub(crate) fn text(
    field: &str,
    value: Option<&str>,
    max: usize,
) -> WorkforceResult<Option<String>> {
    let Some(value) = value.map(str::trim).filter(|v| !v.is_empty()) else {
        return Ok(None);
    };
    if value.chars().count() > max {
        return Err(WorkforceError::invalid(
            field,
            format!("at most {max} characters"),
        ));
    }
    if value
        .chars()
        .any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t'))
    {
        return Err(WorkforceError::invalid(
            field,
            "contains control characters",
        ));
    }
    Ok(Some(value.to_owned()))
}

/// Maps a unique-index violation to a field error.
pub(crate) fn taken(e: sqlx::Error, index: &str, field: &str, message: &str) -> WorkforceError {
    match &e {
        sqlx::Error::Database(d) if d.constraint() == Some(index) => {
            WorkforceError::invalid(field, message)
        }
        _ => WorkforceError::Sqlx(e),
    }
}

/// How often expiring certifications are looked for.
const SWEEP_EVERY: std::time::Duration = std::time::Duration::from_secs(3600);

/// Announces expiring and expired certifications until shutdown, while the
/// module is on. Safe in several processes: each is claimed once.
pub async fn run_sweeper(db: sqlx::PgPool, shutdown: tokio_util::sync::CancellationToken) {
    loop {
        if let Err(e) = sweep_once(&db).await {
            tracing::error!(error = %e, "certification sweep failed");
        }
        tokio::select! {
            () = shutdown.cancelled() => return,
            () = tokio::time::sleep(SWEEP_EVERY) => {}
        }
    }
}

async fn sweep_once(db: &sqlx::PgPool) -> WorkforceResult<()> {
    let mut tx = huxley_db::begin_scoped(db, &huxley_db::OrgScope::System).await?;
    if huxley_modules::is_active(&mut tx, MODULE).await? {
        let announced = requirements::sweep(&mut tx).await?;
        if announced > 0 {
            tracing::info!(announced, "announced expiring and expired certifications");
        }
    }
    tx.commit().await?;
    Ok(())
}
