//! PDM CLI: operational commands for the PDM platform.
//!
//!   pdm-cli migrate
//!   pdm-cli create-org --name "Acme"
//!   pdm-cli create-user --org acme --email a@acme.com --role editor [--password s3kr3t]
//!   pdm-cli set-password --org acme --email a@acme.com --password s3kr3t
//!   pdm-cli create-token --org acme --user a@acme.com
//!   pdm-cli update check [--json]
//!   pdm-cli update apply [--yes]
//!
//! All commands read the connection string from --database-url or PDM_DATABASE_URL.
//! `update` talks to the Gitea instead (PDM_UPDATE_TOKEN; see the module docs
//! in `update.rs` for its exit codes).
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unimplemented,
    clippy::todo,
    clippy::unreachable
)]

mod update;

use std::process::ExitCode;

use clap::{Parser, Subcommand};
use sqlx::postgres::PgPoolOptions;
use tracing::{info, warn};

#[derive(Parser)]
#[command(name = "pdm-cli", about = "Foundry PDM operational CLI")]
struct Cli {
    /// PostgreSQL connection string (default from PDM_DATABASE_URL)
    #[arg(long, global = true)]
    database_url: Option<String>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run database migrations.
    Migrate,
    /// Create an organization.
    CreateOrg {
        #[arg(long)]
        name: String,
    },
    /// Create a user in an organization.
    CreateUser {
        #[arg(long)]
        org: String,
        #[arg(long)]
        email: String,
        #[arg(long, default_value = "")]
        display_name: String,
        /// 'viewer' | 'operator' | 'editor' | 'admin'
        #[arg(long, default_value = "editor")]
        role: String,
        /// Initial password (Argon2id-hashed before storage)
        #[arg(long)]
        password: Option<String>,
    },
    /// Set (or reset) a user's password.
    SetPassword {
        #[arg(long)]
        org: String,
        #[arg(long)]
        email: String,
        #[arg(long)]
        password: String,
    },
    /// Create an API token for a user.
    CreateToken {
        #[arg(long)]
        org: String,
        #[arg(long)]
        user: String,
        #[arg(long, default_value = "cli-token")]
        name: String,
    },
    /// Check for / apply the newest PDM release from Gitea.
    Update {
        #[command(subcommand)]
        cmd: update::UpdateCmd,
    },
}

fn database_url(cli: &Cli) -> String {
    cli.database_url
        .clone()
        .or_else(|| std::env::var("PDM_DATABASE_URL").ok())
        .unwrap_or_else(|| {
            warn!("no --database-url or PDM_DATABASE_URL; using default localhost URL");
            "postgresql://pdm:pdm@localhost:5432/pdm".to_string()
        })
}

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();

    let filter =
        tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into());
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .init();

    match &cli.command {
        Command::Update { cmd } => {
            let db_url = cli
                .database_url
                .clone()
                .or_else(|| std::env::var("PDM_DATABASE_URL").ok());
            match cmd {
                update::UpdateCmd::Check { json } => {
                    return update::check(*json, db_url.as_deref()).await;
                }
                update::UpdateCmd::Apply { yes } => return update::apply(*yes).await,
            }
        }
        _ => {
            let url = database_url(&cli);
            let pool = match PgPoolOptions::new().max_connections(5).connect(&url).await {
                Ok(pool) => pool,
                Err(e) => {
                    eprintln!("error: {e}");
                    return ExitCode::FAILURE;
                }
            };
            match run_db_command(&cli.command, &pool).await {
                Ok(()) => return ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("error: {e}");
                    return ExitCode::FAILURE;
                }
            }
        }
    }
}

async fn run_db_command(command: &Command, pool: &sqlx::PgPool) -> Result<(), pdm_core::PdmError> {
    match command {
        Command::Migrate => {
            pdm_db::migrate(pool).await?;
        }

        Command::CreateOrg { name } => {
            let o = pdm_db::create_org(pool, name).await?;
            info!(org_id = %o.id, name = %o.name, "organization created");
            println!("org_id: {}", o.id);
        }

        Command::CreateUser {
            org,
            email,
            display_name,
            role,
            password,
        } => {
            let org_row = pdm_db::get_org_by_name(pool, org)
                .await?
                .ok_or_else(|| pdm_core::PdmError::NotFound(format!("org '{org}'")))?;
            let hash = match password {
                Some(pw) => Some(pdm_db::hash_password(pw)?),
                None => None,
            };
            let u =
                pdm_db::create_user(pool, org_row.id, email, display_name, role, hash.as_deref())
                    .await?;
            info!(user_id = %u.id, email = %u.email, role = %role, "user created");
            println!("user_id: {}", u.id);
            if password.is_some() {
                println!("password: set (not shown)");
            }
        }

        Command::SetPassword {
            org,
            email,
            password,
        } => {
            let org_row = pdm_db::get_org_by_name(pool, org)
                .await?
                .ok_or_else(|| pdm_core::PdmError::NotFound(format!("org '{org}'")))?;
            let user_row = pdm_db::get_user_by_email(pool, org_row.id, email)
                .await?
                .ok_or_else(|| pdm_core::PdmError::NotFound(format!("user '{email}'")))?;
            let hash = pdm_db::hash_password(password)?;
            pdm_db::set_user_password(pool, user_row.id, &hash).await?;
            info!(email = %email, "password updated");
            println!("password set for {email}");
        }

        Command::CreateToken { org, user, name } => {
            let org_row = pdm_db::get_org_by_name(pool, org)
                .await?
                .ok_or_else(|| pdm_core::PdmError::NotFound(format!("org '{org}'")))?;
            let user_row = pdm_db::get_user_by_email(pool, org_row.id, user)
                .await?
                .ok_or_else(|| pdm_core::PdmError::NotFound(format!("user '{user}'")))?;

            let token_value = format!("pdm_{}", uuid::Uuid::new_v4());
            let hash = {
                use sha2::{Digest, Sha256};
                let mut h = Sha256::new();
                h.update(token_value.as_bytes());
                hex::encode(h.finalize())
            };

            let _tok = pdm_db::create_token(
                pool,
                org_row.id,
                Some(user_row.id),
                name,
                &hash,
                "full",
                None,
            )
            .await?;
            info!(email = %user, "token created");
            println!("token: {token_value}");
        }

        &Command::Update { .. } => {
            // Handled in main before the DB pool opens.
        }
    }

    Ok(())
}
