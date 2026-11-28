//! `huxley admin`: one-off administrative commands.

use std::io::BufRead as _;

use anyhow::{Context as _, bail};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use clap::Subcommand;
use huxley_core::{Actor, RequestContext, Via};
use huxley_db::audit::{self, AuditRecord, ChainStatus};
use huxley_db::{OrgScope, begin_scoped};
use serde_json::json;

use crate::config::Config;

#[derive(Debug, Subcommand)]
pub enum AdminCommand {
    /// Set up a new instance: create the root organization and the first user.
    /// Applies migrations first. Fails if the instance is already set up.
    Bootstrap {
        /// Name of the owning company (the root organization).
        #[arg(long)]
        org_name: String,
        /// Email of the first user.
        #[arg(long)]
        email: String,
        /// Display name of the first user.
        #[arg(long)]
        name: String,
        /// Read the password from the first line of stdin. Without this, a
        /// random password is generated and printed once.
        #[arg(long)]
        password_stdin: bool,
        /// Feature modules to turn on: `automation` (none: automation engine
        /// only), `service-desk`, or `full`. Modules can be changed later.
        #[arg(long, default_value = "automation")]
        preset: String,
    },
    /// Create a local user.
    CreateUser {
        #[arg(long)]
        email: String,
        #[arg(long)]
        name: String,
        /// Read the password from the first line of stdin. Without this, a
        /// random password is generated and printed once.
        #[arg(long)]
        password_stdin: bool,
        /// Also grant the Owner role at the root organization. Use this to
        /// recover access if no owner can sign in.
        #[arg(long)]
        owner: bool,
    },
    /// Recompute the audit log hash chain and report whether it is intact.
    /// Exits non-zero if any record was altered or removed.
    VerifyAudit,
    /// Print a new random key-encryption key for `crypto.kek`. Needs no
    /// database or configuration.
    GenerateKek,
    /// Before restoring a backup into a new PostgreSQL server: create the
    /// roles Huxley's grants refer to (`pg_dump` doesn't carry them). Run it
    /// against the empty target database, then `pg_restore`.
    PrepareRestore,
    /// After restoring: check migrations, the application role, the
    /// key-encryption keys, the audit chain, and that every file the
    /// database refers to is in object storage. Exits non-zero on a problem.
    VerifyRestore {
        /// Check this many files of each kind, chosen at random, instead of
        /// all of them (for large instances).
        #[arg(long)]
        sample: Option<i64>,
    },
    /// Re-wrap every data key under the active key-encryption key. Run after
    /// rotating `crypto.kek` (with the old key in `crypto.retired_keks`) or
    /// moving to a key service (`crypto.active`); once it succeeds the retired
    /// keys can be removed.
    RewrapKeys {
        /// Also re-wrap keys already under the active key-encryption key, e.g.
        /// to move them to a new key version after rotating it in Vault.
        #[arg(long)]
        all: bool,
    },
}

pub async fn run(config: &Config, command: AdminCommand) -> anyhow::Result<()> {
    if let AdminCommand::GenerateKek = command {
        println!("{}", huxley_crypto::generate_key_base64());
        return Ok(());
    }
    let db_config = config.database()?;
    match command {
        AdminCommand::GenerateKek => unreachable!("handled above"),
        AdminCommand::RewrapKeys { all } => rewrap_keys(config, db_config, all).await,
        AdminCommand::PrepareRestore => crate::restore::prepare(db_config).await,
        AdminCommand::VerifyRestore { sample } => {
            crate::restore::verify(config, db_config, sample).await
        }
        AdminCommand::Bootstrap {
            org_name,
            email,
            name,
            password_stdin,
            preset,
        } => {
            let preset = huxley_modules::Preset::parse(&preset).ok_or_else(|| {
                anyhow::anyhow!("unknown preset `{preset}`: use automation, service-desk, or full")
            })?;
            let owner = huxley_db::connect_owner(db_config)?;
            huxley_db::migrate(&owner)
                .await
                .context("migrations failed")?;
            let db = huxley_db::connect(db_config)?;
            let (password, generated) = password(password_stdin)?;
            let ctx = RequestContext::system(Via::Cli);

            let mut tx = begin_scoped(&db, &OrgScope::System).await?;
            if huxley_orgs::root(&mut tx).await?.is_some() {
                bail!("this instance is already set up (a root organization exists)");
            }
            let root = huxley_orgs::create_root(&mut tx, &ctx, &org_name).await?;
            let user = huxley_auth::create_user(&mut tx, &ctx, &email, &name, &password).await?;
            huxley_authz::grant_root_owner(&mut tx, &ctx, user.id).await?;
            huxley_modules::apply_preset(&mut tx, &ctx, preset).await?;
            audit::record(
                &mut tx,
                &ctx,
                AuditRecord::new("instance.bootstrapped")
                    .org(root.id)
                    .target("user", user.id)
                    .details(json!({ "rootOrg": root.name, "firstUser": user.email })),
            )
            .await?;
            tx.commit().await?;

            println!("Instance set up.");
            println!("  Root organization: {} ({})", root.name, root.id);
            println!("  First user:        {} ({})", user.email, user.id);
            print_generated(generated.then_some(password.as_str()));
            Ok(())
        }
        AdminCommand::CreateUser {
            email,
            name,
            password_stdin,
            owner,
        } => {
            let db = huxley_db::connect(db_config)?;
            let (password, generated) = password(password_stdin)?;
            let ctx = RequestContext::system(Via::Cli).with_actor(Actor::System);
            let mut tx = begin_scoped(&db, &OrgScope::System).await?;
            let user = huxley_auth::create_user(&mut tx, &ctx, &email, &name, &password).await?;
            if owner {
                huxley_authz::grant_root_owner(&mut tx, &ctx, user.id).await?;
            }
            tx.commit().await?;
            println!(
                "Created user {} ({}){}",
                user.email,
                user.id,
                if owner { " with the Owner role" } else { "" }
            );
            print_generated(generated.then_some(password.as_str()));
            Ok(())
        }
        AdminCommand::VerifyAudit => {
            let owner = huxley_db::connect_owner(db_config)?;
            match audit::verify_chain(&owner).await? {
                ChainStatus::Intact { records } => {
                    println!("Audit log intact: {records} records verified.");
                    Ok(())
                }
                ChainStatus::Broken { seq, reason } => {
                    bail!("audit log chain is broken at record {seq}: {reason}")
                }
            }
        }
    }
}

async fn rewrap_keys(
    config: &Config,
    db_config: &huxley_db::DatabaseConfig,
    all: bool,
) -> anyhow::Result<()> {
    let db = huxley_db::connect(db_config)?;
    let ring = config.crypto.key_ring()?;
    let active = ring.active_id().to_owned();
    let retired: Vec<String> = ring.retired_ids().map(str::to_owned).collect();
    let crypto = huxley_crypto::Crypto::new(db, ring);
    let count = crypto
        .rewrap(&RequestContext::system(Via::Cli), None, all)
        .await?;
    println!("Re-wrapped {count} data key(s) under key-encryption key {active}.");
    if !retired.is_empty() {
        println!(
            "These key-encryption keys are no longer needed and can be removed from configuration: {}",
            retired.join(", ")
        );
    }
    Ok(())
}

/// Returns `(password, was_generated)`.
fn password(from_stdin: bool) -> anyhow::Result<(String, bool)> {
    if from_stdin {
        let mut line = String::new();
        std::io::stdin()
            .lock()
            .read_line(&mut line)
            .context("failed to read password from stdin")?;
        let password = line.trim_end_matches(['\r', '\n']).to_owned();
        huxley_auth::password::validate(&password)?;
        return Ok((password, false));
    }
    let mut bytes = [0u8; 18];
    getrandom::fill(&mut bytes).context("the operating system RNG is unavailable")?;
    Ok((URL_SAFE_NO_PAD.encode(bytes), true))
}

fn print_generated(password: Option<&str>) {
    if let Some(password) = password {
        println!();
        println!("  Generated password: {password}");
        println!("  It is shown only once. Sign in and change it from your profile.");
    }
}
