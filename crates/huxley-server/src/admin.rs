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
    },
    /// Recompute the audit log hash chain and report whether it is intact.
    /// Exits non-zero if any record was altered or removed.
    VerifyAudit,
}

pub async fn run(config: &Config, command: AdminCommand) -> anyhow::Result<()> {
    let db_config = config.database()?;
    match command {
        AdminCommand::Bootstrap {
            org_name,
            email,
            name,
            password_stdin,
        } => {
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
        } => {
            let db = huxley_db::connect(db_config)?;
            let (password, generated) = password(password_stdin)?;
            let ctx = RequestContext::system(Via::Cli).with_actor(Actor::System);
            let mut tx = db.begin().await?;
            let user = huxley_auth::create_user(&mut tx, &ctx, &email, &name, &password).await?;
            tx.commit().await?;
            println!("Created user {} ({})", user.email, user.id);
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
