//! Restoring from backup (`docs/backup.md`): preparing an empty database
//! for `pg_restore`, and checking a restored instance end to end.

use std::fmt::Write as _;

use anyhow::{Context as _, bail};
use futures_util::{StreamExt as _, stream};
use huxley_db::audit::{self, ChainStatus};
use huxley_db::{OrgScope, begin_scoped, sqlx};

use crate::config::Config;

/// Creates the database roles Huxley's grants refer to. They belong to the
/// PostgreSQL server, not the database, so `pg_dump` doesn't carry them:
/// restoring into a new server without them loses every grant.
pub async fn prepare(db_config: &huxley_db::DatabaseConfig) -> anyhow::Result<()> {
    let owner = huxley_db::connect_owner(db_config)?;
    for role in ["huxley_app", "huxley_datasets_reader"] {
        let exists: bool =
            sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = $1)")
                .bind(role)
                .fetch_one(&owner)
                .await?;
        if exists {
            println!("Role {role} exists.");
        } else {
            sqlx::query(sqlx::AssertSqlSafe(format!("CREATE ROLE {role} NOLOGIN")))
                .execute(&owner)
                .await
                .with_context(|| {
                    format!("couldn't create role {role} (the connecting user needs CREATEROLE)")
                })?;
            println!("Created role {role}.");
        }
    }
    sqlx::query("GRANT huxley_app TO CURRENT_USER")
        .execute(&owner)
        .await?;
    let tables: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM pg_tables WHERE schemaname IN ('public', 'datasets')",
    )
    .fetch_one(&owner)
    .await?;
    if tables > 0 {
        println!("Note: this database already has {tables} tables. Restore into an empty one.");
    }
    println!("Ready: restore with pg_restore, then run `huxley admin verify-restore`.");
    Ok(())
}

/// One check's outcome.
enum Outcome {
    Ok(String),
    Warn(String),
    Fail(String),
}

struct Report {
    failed: bool,
}

impl Report {
    fn line(&mut self, name: &str, outcome: Outcome) {
        let (mark, text) = match outcome {
            Outcome::Ok(t) => ("ok  ", t),
            Outcome::Warn(t) => ("warn", t),
            Outcome::Fail(t) => {
                self.failed = true;
                ("FAIL", t)
            }
        };
        println!("[{mark}] {name}: {text}");
    }
}

/// Where stored objects are referred to, and what they are.
const OBJECTS: [(&str, &str); 4] = [
    (
        "Attachments",
        "SELECT storage_key FROM attachments WHERE deleted_at IS NULL",
    ),
    (
        "Expense receipts",
        "SELECT receipt_key FROM exp_entries WHERE receipt_key IS NOT NULL",
    ),
    ("Inbound email", "SELECT raw_key FROM sd_inbound_emails"),
    (
        "Spilled step payloads",
        "SELECT input_ref FROM step_runs WHERE input_ref IS NOT NULL \
         UNION ALL SELECT output_ref FROM step_runs WHERE output_ref IS NOT NULL",
    ),
];

/// Checks a restored instance: migrations, the application role, the
/// key-encryption keys, the audit chain, and that the files the database
/// refers to are in object storage. Fails if anything is wrong.
pub async fn verify(
    config: &Config,
    db_config: &huxley_db::DatabaseConfig,
    sample: Option<i64>,
) -> anyhow::Result<()> {
    let mut report = Report { failed: false };
    let owner = huxley_db::connect_owner(db_config)?;
    report.line("Migrations", migrations(&owner).await?);

    let db = huxley_db::connect(db_config)?;
    let role = app_role(&db).await;
    let role_ok = matches!(role, Outcome::Ok(_));
    report.line("Application role", role);

    // It reads through the application role, so it can't run without it.
    let keys = if role_ok {
        let crypto = huxley_crypto::Crypto::new(db.clone(), config.crypto.key_ring()?);
        match crypto.verify_keys().await {
            Ok(()) => Outcome::Ok("every data key unwraps".into()),
            Err(e) => Outcome::Fail(format!(
                "{e} (is crypto.kek, or the key service, the one this backup was made with?)"
            )),
        }
    } else {
        Outcome::Fail("not checked: needs the application role".into())
    };
    report.line("Key-encryption keys", keys);

    report.line(
        "Audit log",
        match audit::verify_chain(&owner).await {
            Ok(ChainStatus::Intact { records }) => {
                Outcome::Ok(format!("chain intact, {records} records"))
            }
            Ok(ChainStatus::Broken { seq, reason }) => {
                Outcome::Fail(format!("chain broken at record {seq}: {reason}"))
            }
            Err(e) => Outcome::Fail(e.to_string()),
        },
    );

    let storage = huxley_storage::Storage::from_config(&config.storage)?;
    report.line(
        "Object storage",
        match storage.check().await {
            Ok(()) => Outcome::Ok(format!("{} is writable", storage.describe())),
            Err(e) => Outcome::Fail(format!("{}: {e}", storage.describe())),
        },
    );
    for (name, sql) in OBJECTS {
        let outcome = objects(&owner, &storage, sql, sample).await;
        report.line(name, outcome);
    }

    if report.failed {
        bail!("the restored instance has problems (above)");
    }
    println!("The restored instance checks out.");
    Ok(())
}

async fn migrations(owner: &sqlx::PgPool) -> anyhow::Result<Outcome> {
    let applied: Vec<(i64, Vec<u8>)> = sqlx::query_as(
        "SELECT version, checksum FROM _sqlx_migrations WHERE success ORDER BY version",
    )
    .fetch_all(owner)
    .await
    .context("no migration history: is this a Huxley database?")?;
    let known = &huxley_db::MIGRATOR;
    let unknown: Vec<i64> = applied
        .iter()
        .filter(|(v, _)| !known.iter().any(|m| m.version == *v))
        .map(|(v, _)| *v)
        .collect();
    let changed: Vec<i64> = applied
        .iter()
        .filter(|(v, sum)| {
            known
                .iter()
                .any(|m| m.version == *v && m.checksum.as_ref() != sum.as_slice())
        })
        .map(|(v, _)| *v)
        .collect();
    let pending = known
        .iter()
        .filter(|m| {
            !m.migration_type.is_down_migration() && !applied.iter().any(|(v, _)| *v == m.version)
        })
        .count();
    Ok(if !unknown.is_empty() {
        Outcome::Fail(format!(
            "{} applied that this binary doesn't know (it's older than the backup): {unknown:?}",
            unknown.len()
        ))
    } else if !changed.is_empty() {
        Outcome::Fail(format!(
            "{} differ from this binary's: {changed:?}",
            changed.len()
        ))
    } else if pending > 0 {
        Outcome::Warn(format!(
            "{} applied, {pending} newer to apply: run `huxley migrate`",
            applied.len()
        ))
    } else {
        Outcome::Ok(format!("{} applied, matching this binary", applied.len()))
    })
}

async fn app_role(db: &sqlx::PgPool) -> Outcome {
    let checked = async {
        let mut tx = begin_scoped(db, &OrgScope::System).await?;
        let (role, orgs, users): (String, i64, i64) = sqlx::query_as(
            "SELECT current_user::text, (SELECT count(*) FROM organizations), (SELECT count(*) FROM users)",
        )
        .fetch_one(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok::<_, anyhow::Error>((role, orgs, users))
    }
    .await;
    match checked {
        Ok((role, orgs, users)) if role == "huxley_app" && orgs > 0 => Outcome::Ok(format!(
            "runs as huxley_app; {orgs} organizations, {users} users"
        )),
        Ok((role, _, _)) if role != "huxley_app" => Outcome::Fail(format!(
            "queries run as {role}, not huxley_app: run `huxley admin prepare-restore` before restoring"
        )),
        Ok(_) => Outcome::Fail(
            "no organizations are visible: the restore is empty, or its grants were lost".into(),
        ),
        Err(e) => Outcome::Fail(format!(
            "{e} (were the roles created before restoring? `huxley admin prepare-restore`)"
        )),
    }
}

async fn objects(
    owner: &sqlx::PgPool,
    storage: &huxley_storage::Storage,
    sql: &'static str,
    sample: Option<i64>,
) -> Outcome {
    let query = match sample {
        Some(n) => format!(
            "SELECT k FROM ({sql}) refs(k) ORDER BY random() LIMIT {}",
            n.max(1)
        ),
        None => format!("SELECT k FROM ({sql}) refs(k)"),
    };
    let keys: Vec<String> = match sqlx::query_scalar(sqlx::AssertSqlSafe(query))
        .fetch_all(owner)
        .await
    {
        Ok(keys) => keys,
        Err(e) => return Outcome::Fail(e.to_string()),
    };
    if keys.is_empty() {
        return Outcome::Ok("none".into());
    }
    let checked: Vec<(String, Result<bool, huxley_storage::StorageError>)> = stream::iter(keys)
        .map(|key| async move {
            let found = storage.exists(&key).await;
            (key, found)
        })
        .buffer_unordered(16)
        .collect()
        .await;
    let total = checked.len();
    let mut missing = Vec::new();
    let mut errors = Vec::new();
    for (key, found) in checked {
        match found {
            Ok(true) => {}
            Ok(false) => missing.push(key),
            Err(e) => errors.push(format!("{key}: {e}")),
        }
    }
    let of = if sample.is_some() {
        format!("{total} sampled")
    } else {
        format!("all {total}")
    };
    if missing.is_empty() && errors.is_empty() {
        return Outcome::Ok(format!("{of} present"));
    }
    let mut text = format!("{} of {of} missing", missing.len() + errors.len());
    for key in missing.iter().chain(errors.iter()).take(5) {
        let _ = write!(text, "\n         {key}");
    }
    Outcome::Fail(text)
}
