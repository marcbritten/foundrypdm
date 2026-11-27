//! PDM server binary: boots the API and (optionally) the worker in one process.
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unimplemented,
    clippy::todo,
    clippy::unreachable
)]

use std::sync::Arc;

use clap::Parser;
use sqlx::postgres::PgPoolOptions;
use tracing::{info, warn};

/// Foundry PDM server
#[derive(Parser)]
struct Args {
    /// Listen address (default 0.0.0.0:8080)
    #[arg(long, default_value = "0.0.0.0:8080")]
    listen: String,

    /// PostgreSQL connection string (default from PDM_DATABASE_URL env)
    #[arg(long)]
    database_url: Option<String>,
}

type MainError = Box<dyn std::error::Error + Send + Sync>;

#[tokio::main]
async fn main() -> Result<(), MainError> {
    let args = Args::parse();

    let filter =
        tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into());
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .init();

    let database_url = args
        .database_url
        .clone()
        .or_else(|| std::env::var("PDM_DATABASE_URL").ok())
        .unwrap_or_else(|| {
            warn!("PDM_DATABASE_URL not set; using localhost default");
            "postgresql://pdm:pdm@localhost:5432/pdm".to_string()
        });

    let pool = PgPoolOptions::new()
        .max_connections(20)
        .connect(&database_url)
        .await
        .map_err(|e| format!("failed to connect to PostgreSQL: {e}"))?;
    info!(url = %database_url.replace("pdm@", "****@"), "connected to database");

    pdm_db::migrate(&pool).await?;
    info!("migrations applied");

    let backend = pdm_storage::Backend::from_env()
        .map_err(|e| format!("storage backend config failed: {e}"))?;
    let store = Arc::new(
        pdm_storage::BlobStore::create(&backend)
            .await
            .map_err(|e| format!("storage init failed: {e}"))?,
    );
    info!(backend = %backend.bucket_name(), "object storage ready");

    // 2 GiB: a native CAD file with its history (Inventor, SolidWorks) easily exceeds the old
    // 256 MB cap; blobs are content-addressed and stored on the vault's disk.
    let max_upload: u64 = std::env::var("PDM_MAX_UPLOAD_BYTES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(2 * 1024 * 1024 * 1024);
    let lease_hours: i64 = std::env::var("PDM_LOCK_LEASE_HOURS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(24);

    // The key that signs the SolidCraft web editor's short-lived design links: random, made
    // once and kept in the database (shared by every API process on it), so nothing to set.
    let design_link_key = pdm_db::server_secret(&pool, "design_link", 32).await?;
    let design_links = pdm_api::DesignLinks::new(&design_link_key)?;

    let state = pdm_api::AppState {
        pool: pool.clone(),
        store: store.clone(),
        max_upload,
        lease_hours,
        started_at: std::time::Instant::now(),
        update: std::sync::Arc::new(tokio::sync::RwLock::new(pdm_update::UpdateState::default())),
        design_links,
    };
    let app = pdm_api::build_router(state.clone());

    // Background release check: at most once every 24 h (marker file next to
    // the local data dir when storage is local), silent on failure, opt out
    // with PDM_UPDATE_CHECK=0. Needs PDM_UPDATE_TOKEN for the private repo.
    if !pdm_update::check_disabled() {
        let update = state.update.clone();
        let marker = std::env::var("PDM_STORAGE_LOCAL")
            .ok()
            .map(|d| std::path::Path::new(&d).join(".pdm-update-check"));
        let build = pdm_api::version_info();
        let current = build["build"]
            .as_str()
            .map(str::to_string)
            .or_else(|| build["version"].as_str().map(str::to_string))
            .unwrap_or_default();
        tokio::spawn(async move {
            pdm_update::boot_check(update, marker.as_deref(), &current).await;
        });
    }
    if !pdm_update::cad_cli_update_disabled() {
        let dest = std::env::var("PDM_SOLVECRAFT_CLI")
            .unwrap_or_else(|_| pdm_update::CAD_CLI_DEFAULT_PATH.to_string());
        tokio::spawn(async move {
            pdm_update::loop_cad_cli_update(std::path::PathBuf::from(dest)).await;
        });
    }
    // Keep Dutchman (native CAD → SolidCraft with a feature tree) on the release
    // the org's settings reference: check at boot, then on an interval
    // (PDM_DUTCHMAN_UPDATE=0 opts out; PDM_DUTCHMAN_CLI names the binary,
    // default /usr/local/bin/dutchman; PDM_DUTCHMAN_CHANNEL overrides the org
    // choice for this host). Conversion and Onshape-import jobs also pull the
    // referenced release on demand.
    if !pdm_update::dutchman_update_disabled() {
        let dest = std::path::PathBuf::from(pdm_update::dutchman_cli_path());
        let pool = pool.clone();
        let env_channel = std::env::var("PDM_DUTCHMAN_CHANNEL")
            .ok()
            .filter(|s| !s.trim().is_empty());
        tokio::spawn(async move {
            loop {
                let channel = env_channel
                    .clone()
                    .or(pdm_db::first_org_dutchman_channel(&pool).await);
                let channel = match channel {
                    Some(c) if !c.trim().is_empty() => c,
                    _ => "latest".to_string(),
                };
                pdm_update::boot_dutchman_update(&dest, &channel).await;
                tokio::time::sleep(pdm_update::dutchman_interval()).await;
            }
        });
    }
    // Keep the bundled SolidCraft web build ("Edit in SolidCraft web") on the
    // newest GitHub release, the way the CLI is kept current. Only where the
    // portal actually ships it (the image's PDM_PORTAL_DIR); PDM_WEB_UPDATE=0
    // opts out.
    if !pdm_update::sc_web_update_disabled()
        && let Some(dest) = pdm_update::solidcraft_web_dir()
    {
        tokio::spawn(async move {
            pdm_update::loop_solidcraft_web_update(dest).await;
        });
    }

    // Optional in-process worker (disable with PDM_WORKER=0 to run a dedicated
    // worker container instead).
    let worker_enabled: bool = std::env::var("PDM_WORKER")
        .map(|v| v != "0")
        .unwrap_or(true);
    if worker_enabled {
        let worker = pdm_worker::Worker::new(
            pool.clone(),
            store.clone(),
            pdm_worker::WorkerConfig::from_env(),
        );
        tokio::spawn(async move {
            worker.run_forever().await;
        });
        info!("worker started in-process");
    }

    let build = pdm_api::version_info();
    info!(
        version = build["version"].as_str().unwrap_or("?"),
        commit = build["commit"].as_str().unwrap_or("unknown"),
        built_at = build["built_at"].as_str().unwrap_or("unknown"),
        "PDM server build"
    );
    let listener = tokio::net::TcpListener::bind(&args.listen)
        .await
        .map_err(|e| format!("bind {0} failed: {e}", args.listen))?;
    info!(listen = %args.listen, "PDM server listening");
    axum::serve(listener, app)
        .await
        .map_err(|e| format!("server error: {e}"))?;
    Ok(())
}
