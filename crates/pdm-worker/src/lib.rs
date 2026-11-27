//! PDM background worker: polls the Postgres-backed job queue and executes jobs.
//!
//! Job kinds:
//! - `design.analyze` downloads the committed blob and runs `solvecraft-cli eval`
//!   to extract geometry measurements, storing them on the file version (CAD files
//!   the CLI opens directly; anything else is recorded as skipped).
//! - `preview.render` produces a PNG thumbnail of **any** CAD file the commit
//!   holds: designs, STEP/IGES, FreeCAD/Rhino/Inventor and meshes open straight
//!   through `solvecraft-cli snapshot`; B-rep formats the CLI reads only as a
//!   conversion (SolidWorks, CATIA, …) are restated to STEP with
//!   `solvecraft-cli convert` first, then snapshotted. The PNG is stored as a
//!   content-addressed blob and its SHA-256 is recorded on the file version.
//!   A file the CLI cannot open at all records a clear reason on the version
//!   (`thumbnail_error`) instead of dead-lettering; non-CAD files keep a clean
//!   "no preview" state.
//! - `cad.convert` restates convertible CAD as a stored STEP (see `convert_cad`).
//! - `cad.solidcraft` saves a native `.sc` design beside its source version:
//!   Dutchman (the release the org's settings reference — newest stable,
//!   nightly or a pinned tag such as the Onshape-preview builds — pulled on
//!   demand when the binary is missing or stale) decodes the native formats —
//!   Fusion, Inventor, SolidWorks, FreeCAD, CATIA, Rhino, Creo View, ACIS,
//!   STEP, Onshape bundles — into a design with editable feature history where
//!   it maps; the SolidCraft CLI restates the remainder (meshes, IGES) without
//!   history (see `convert_to_solidcraft`).
//! - `onshape.import` fetches an Onshape Part Studio into the vault as an
//!   `.onshape-bundle` version via `dutchman onshape fetch` with the org's
//!   Onshape keys, then enqueues the `cad.solidcraft` conversion (see
//!   `onshape_import`).
//!
//! Unknown job kinds are marked done (stub) so enqueued follow-up work cannot
//! jam the queue.
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unimplemented,
    clippy::todo,
    clippy::unreachable
)]

use std::sync::Arc;
use std::time::Duration;

use pdm_core::{Job, PdmError};
use sqlx::PgPool;
use tokio::sync::Semaphore;
use tracing::{debug, info, warn};
use uuid::Uuid;

use pdm_storage::BlobStore;

/// Configuration for the worker loop.
#[derive(Debug, Clone)]
pub struct WorkerConfig {
    /// Poll interval between queue checks.
    pub poll_interval: Duration,
    /// Maximum number of jobs claimed per poll.
    pub batch_size: i64,
    /// Maximum number of concurrently running jobs.
    pub concurrency: usize,
    /// Path to the `solvecraft-cli` binary; if `None`, `design.analyze` and
    /// `preview.render` jobs are marked done with a `skipped` note instead of
    /// failing.
    pub solve_cli: Option<String>,
    /// Path to the `dutchman` binary (native CAD → SolidCraft with feature
    /// history); unset → `PDM_DUTCHMAN_CLI`'s default location, which the PDM
    /// pulls from the newest GitHub release on demand when missing.
    pub dutchman_cli: Option<String>,
    /// Wall-clock limit for one Dutchman conversion (feature-tree rebuilds and
    /// history verification are slower than a CLI restate).
    pub dutchman_timeout: Duration,
    /// Path to the pinned `sparrow` nesting binary; unset → laser.nest jobs
    /// fail explicitly (an absent engine must not fake a layout).
    pub sparrow_cli: Option<String>,
}

impl WorkerConfig {
    pub fn from_env() -> Self {
        let parse = |name: &str, def: u64| -> u64 {
            std::env::var(name)
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(def)
        };
        Self {
            poll_interval: Duration::from_millis(parse("PDM_WORKER_POLL_MS", 1000)),
            batch_size: parse("PDM_WORKER_BATCH", 5) as i64,
            concurrency: parse("PDM_WORKER_CONCURRENCY", 2) as usize,
            solve_cli: std::env::var("PDM_SOLVECRAFT_CLI")
                .ok()
                .filter(|s| !s.is_empty()),
            dutchman_cli: std::env::var("PDM_DUTCHMAN_CLI")
                .ok()
                .filter(|s| !s.is_empty()),
            dutchman_timeout: Duration::from_secs(parse("PDM_DUTCHMAN_TIMEOUT_SECS", 1800)),
            sparrow_cli: std::env::var("PDM_SPARROW_CLI")
                .ok()
                .filter(|s| !s.is_empty()),
        }
    }
}

pub struct Worker {
    pool: PgPool,
    store: Arc<BlobStore>,
    cfg: WorkerConfig,
}

impl Worker {
    pub fn new(pool: PgPool, store: Arc<BlobStore>, cfg: WorkerConfig) -> Self {
        Self { pool, store, cfg }
    }

    /// Run the worker loop until the task is cancelled (process shutdown).
    pub async fn run_forever(&self) {
        let sem = Arc::new(Semaphore::new(self.cfg.concurrency.max(1)));
        info!(
            poll_ms = self.cfg.poll_interval.as_millis() as u64,
            batch = self.cfg.batch_size,
            concurrency = self.cfg.concurrency,
            cli = ?self.cfg.solve_cli,
            "worker started"
        );
        loop {
            let pool = self.pool.clone();
            let store = self.store.clone();
            let cfg = self.cfg.clone();
            let sem = sem.clone();
            let claimed = match pdm_db::claim_jobs(&pool, "inline", cfg.batch_size).await {
                Ok(j) => j,
                Err(e) => {
                    warn!(error = %e, "claim_jobs failed; retrying next tick");
                    vec![]
                }
            };
            for job in claimed {
                let pool = pool.clone();
                let store = store.clone();
                let cfg = cfg.clone();
                let sem = sem.clone();
                tokio::spawn(async move {
                    let _permit = match sem.acquire_owned().await {
                        Ok(p) => p,
                        Err(_) => return, // semaphore closed
                    };
                    run_job(&pool, &store, &cfg, &job).await;
                    drop(_permit);
                });
            }
            tokio::time::sleep(cfg.poll_interval).await;
        }
    }
}

/// What a successfully executed job wants persisted on the version.
#[derive(Debug)]
enum Outcome {
    /// `design.analyze` produced measurements JSON.
    Measurements(serde_json::Value),
    /// `preview.render` produced a stored thumbnail blob.
    Thumbnail { sha256: String, size_bytes: i64 },
    /// `cad.convert` produced a stored converted-STEP blob.
    DerivedStep { sha256: String, size_bytes: i64 },
    /// Nothing to persist (skip or unknown kind).
    Done,
}

async fn run_job_kind(
    pool: &PgPool,
    store: &Arc<BlobStore>,
    cfg: &WorkerConfig,
    job: &Job,
) -> Result<Outcome, PdmError> {
    match job.kind.as_str() {
        "design.analyze" => Ok(Outcome::Measurements(
            analyze_design(pool, store, cfg, job).await?,
        )),
        "preview.render" => match render_preview(pool, store, cfg, job).await? {
            Some((sha256, size_bytes)) => Ok(Outcome::Thumbnail { sha256, size_bytes }),
            None => Ok(Outcome::Done),
        },
        "cad.convert" => match convert_cad(pool, store, cfg, job).await? {
            Some((sha256, size_bytes)) => Ok(Outcome::DerivedStep { sha256, size_bytes }),
            None => Ok(Outcome::Done),
        },
        "cad.solidcraft" => {
            convert_to_solidcraft(pool, store, cfg, job).await?;
            Ok(Outcome::Done)
        }
        "sheet.flat-pattern" => {
            generate_flat_pattern(pool, store, cfg, job).await?;
            Ok(Outcome::Done)
        }
        "onshape.import" => {
            onshape_import(pool, store, cfg, job).await?;
            Ok(Outcome::Done)
        }
        "laser.nest" => {
            run_laser_nest(pool, store, cfg, job).await?;
            Ok(Outcome::Done)
        }
        other => {
            debug!(kind = other, "unknown job kind; marking done");
            Ok(Outcome::Done)
        }
    }
}

async fn run_job(pool: &PgPool, store: &Arc<BlobStore>, cfg: &WorkerConfig, job: &Job) {
    let result = match run_job_kind(pool, store, cfg, job).await {
        Ok(Outcome::Measurements(m)) => persist_measurements(pool, job, m).await,
        Ok(Outcome::Thumbnail { sha256, size_bytes }) => {
            persist_thumbnail(pool, job, &sha256, size_bytes).await
        }
        Ok(Outcome::DerivedStep { sha256, size_bytes }) => {
            store_derived_step(pool, job, &sha256, size_bytes).await
        }
        Ok(Outcome::Done) => Ok(()),
        Err(e) => Err(e),
    };
    let (ok, err) = match result {
        Ok(()) => (true, String::new()),
        Err(e) => (false, e.to_string()),
    };
    if !ok
        && job.kind == "cad.solidcraft"
        && let Some(version_id) = version_id_from_job(job)
    {
        let _ = sqlx::query("UPDATE file_versions SET derived_solidcraft_error = $3 WHERE id = $1 AND derived_solidcraft_job_id = $2")
            .bind(version_id).bind(job.id).bind(&err).execute(pool).await;
    }

    let _ = pdm_db::record_job_attempt(pool, job.id, ok, &err).await;
    if ok {
        let _ = pdm_db::complete_job(pool, job.id, true, "").await;
    } else if job.attempts + 1 < job.max_attempts {
        let _ = sqlx::query(
            "UPDATE jobs SET status = 'queued', last_error = $2, run_after = now() + interval '30 seconds', updated_at = now() WHERE id = $1",
        )
        .bind(job.id)
        .bind(&err)
        .execute(pool)
        .await;
        warn!(job = %job.id, error = %err, "job will be retried");
    } else {
        let _ = pdm_db::complete_job(pool, job.id, false, &err).await;
        let _ = sqlx::query("UPDATE jobs SET status = 'dead' WHERE id = $1")
            .bind(job.id)
            .execute(pool)
            .await;
        warn!(job = %job.id, error = %err, "job dead-lettered");
        // A terminally failed laser.nest job leaves the nest "running" otherwise.
        if job.kind == "laser.nest"
            && let Some(nest_id) = nest_id_from_job(job)
        {
            let _ = pdm_db::set_laser_nest_status(pool, job.org_id, nest_id, "failed", Some(&err))
                .await;
        }
    }
}

fn version_id_from_job(job: &Job) -> Option<Uuid> {
    job.payload
        .get("version_id")
        .and_then(|v| v.as_str())
        .and_then(|s| Uuid::parse_str(s).ok())
}

fn nest_id_from_job(job: &Job) -> Option<Uuid> {
    job.payload
        .get("nest_id")
        .and_then(|v| v.as_str())
        .and_then(|s| Uuid::parse_str(s).ok())
}

/// Persist analysis results; when the design's timeline contains sheet-metal
/// features, flag the item as a sheet-metal part (unless a spec exists).
async fn persist_measurements(
    pool: &PgPool,
    job: &Job,
    measurements: serde_json::Value,
) -> Result<(), PdmError> {
    if let (Some(item_id), true) = (
        item_id_from_job(job),
        pdm_core::measurements_have_sheet_metal_features(&measurements),
    ) {
        let flagged = pdm_db::auto_flag_sheet_metal(pool, job.org_id, item_id).await?;
        if flagged {
            info!(item = %item_id, "sheet-metal features in design timeline; item flagged as sheet metal");
        }
        // Generate the flat pattern for this version too; a failed generation
        // is retried by the job's normal attempts and can be re-requested from
        // the item's Sheet metal tab.
        if let Some(version_id) = version_id_from_job(job) {
            let _ = pdm_db::enqueue_job(
                pool,
                job.org_id,
                "sheet.flat-pattern",
                &serde_json::json!({
                    "version_id": version_id,
                    "item_id": item_id,
                    "filename": job.payload.get("filename").and_then(|v| v.as_str()).unwrap_or_default(),
                }),
            )
            .await;
        }
    }
    store_measurements(pool, job, measurements).await
}

async fn store_measurements(
    pool: &PgPool,
    job: &Job,
    measurements: serde_json::Value,
) -> Result<(), PdmError> {
    let Some(version_id) = version_id_from_job(job) else {
        return Err(PdmError::Validation("job has no version_id".into()));
    };
    sqlx::query("UPDATE file_versions SET measurements = $2 WHERE id = $1")
        .bind(version_id)
        .bind(&measurements)
        .execute(pool)
        .await
        .map_err(PdmError::Db)?;
    Ok(())
}

async fn persist_thumbnail(
    pool: &PgPool,
    job: &Job,
    sha256: &str,
    size_bytes: i64,
) -> Result<(), PdmError> {
    let Some(version_id) = version_id_from_job(job) else {
        return Err(PdmError::Validation("job has no version_id".into()));
    };
    pdm_db::store_thumbnail(pool, version_id, sha256, size_bytes).await
}

async fn store_derived_step(
    pool: &PgPool,
    job: &Job,
    sha256: &str,
    size_bytes: i64,
) -> Result<(), PdmError> {
    let Some(version_id) = version_id_from_job(job) else {
        return Err(PdmError::Validation("job has no version_id".into()));
    };
    pdm_db::store_derived_step(pool, version_id, sha256, size_bytes).await
}

fn item_id_from_job(job: &Job) -> Option<Uuid> {
    job.payload
        .get("item_id")
        .and_then(|v| v.as_str())
        .and_then(|s| Uuid::parse_str(s).ok())
}

/// How the committed file's name reads to the CLI: its extension (lower case,
/// empty when the job predates filenames), and the file record itself.
struct CommittedFile {
    filename: String,
    ext: String,
}

async fn committed_file(pool: &PgPool, job: &Job) -> Result<CommittedFile, PdmError> {
    let Some(item_id) = item_id_from_job(job) else {
        // Jobs enqueued before item ids were recorded: the blob was written
        // `.solvecraft` and opened as such.
        return Ok(CommittedFile {
            filename: String::new(),
            ext: String::new(),
        });
    };
    let filename = if let Some(filename) = job.payload.get("filename").and_then(|v| v.as_str()) {
        filename.to_string()
    } else {
        pdm_db::get_file(pool, item_id)
            .await?
            .map(|f| f.filename)
            .unwrap_or_default()
    };
    let ext = std::path::Path::new(&filename)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .unwrap_or_default();
    Ok(CommittedFile { filename, ext })
}

/// Run `solvecraft-cli eval` on the committed blob; return measurements JSON.
async fn analyze_design(
    pool: &PgPool,
    store: &Arc<BlobStore>,
    cfg: &WorkerConfig,
    job: &Job,
) -> Result<serde_json::Value, PdmError> {
    let version_id = job
        .payload
        .get("version_id")
        .and_then(|v| v.as_str())
        .and_then(|s| Uuid::parse_str(s).ok())
        .ok_or_else(|| PdmError::Validation("missing version_id in job payload".into()))?;

    let version = pdm_db::get_version(pool, version_id)
        .await?
        .ok_or_else(|| PdmError::NotFound(format!("version {version_id}")))?;
    let blob = pdm_db::get_blob_by_id(pool, version.blob_id)
        .await?
        .ok_or_else(|| PdmError::NotFound("blob for version".into()))?;

    let Some(cli) = cfg.solve_cli.as_deref() else {
        return Ok(
            serde_json::json!({ "skipped": true, "reason": "solvecraft-cli not configured" }),
        );
    };

    // Measurements come from the CLI opening the file: only formats it opens
    // directly (not the convert-then-read B-rep set, not documents at all).
    let file = committed_file(pool, job).await?;
    let plan = if file.ext.is_empty() {
        pdm_core::PreviewPlan::Direct
    } else {
        pdm_core::preview_plan(&file.filename)
    };
    if plan != pdm_core::PreviewPlan::Direct {
        let reason = if plan == pdm_core::PreviewPlan::Convert {
            format!(
                "measurements are not extracted from .{} files yet (the preview restates them as STEP instead)",
                file.ext
            )
        } else {
            "measurements are rendered for CAD files the CLI opens: designs, STEP/IGES, FreeCAD, Rhino, Inventor and meshes".into()
        };
        debug!(version = %version_id, "design.analyze skipped");
        return Ok(serde_json::json!({ "skipped": true, "reason": reason }));
    }

    let data = store
        .get_by_sha(&blob.sha256)
        .await
        .map_err(|e| PdmError::Storage(e.to_string()))?;

    // The real extension: a `.step` written `.solvecraft` would be misread as a design.
    let ext = if file.ext.is_empty() {
        "solvecraft"
    } else {
        file.ext.as_str()
    };
    let tmp = std::env::temp_dir().join(format!("pdm-job-{}.{ext}", job.id));
    std::fs::write(&tmp, &data).map_err(|e| PdmError::Storage(e.to_string()))?;

    let out = tokio::process::Command::new(cli)
        .arg("eval")
        .arg(&tmp)
        .output()
        .await
        .map_err(|e| PdmError::Internal(e.to_string()))?;
    let _ = std::fs::remove_file(&tmp);

    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr).to_string();
        return Err(PdmError::Internal(format!(
            "solvecraft-cli eval failed: {stderr}"
        )));
    }
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    serde_json::from_str(&stdout).map_err(|e| PdmError::Internal(format!("bad eval output: {e}")))
}

/// `sheet.flat-pattern`: generate the flat-pattern DXF for a version's design
/// via `solidcraft-cli run <script> --in <design>` (`sheet.export_dxf`) and
/// store the result on the item's sheet-metal spec.
async fn generate_flat_pattern(
    pool: &PgPool,
    store: &Arc<BlobStore>,
    cfg: &WorkerConfig,
    job: &Job,
) -> Result<(), PdmError> {
    let version_id = version_id_from_job(job)
        .ok_or_else(|| PdmError::Validation("missing version_id".into()))?;
    let Some(cli) = cfg.solve_cli.as_deref() else {
        debug!(
            version = %version_id,
            "sheet.flat-pattern skipped: solvecraft-cli not configured"
        );
        return Ok(());
    };
    let source = pdm_db::version_source(pool, job.org_id, version_id)
        .await?
        .ok_or_else(|| PdmError::NotFound("source version".into()))?;
    let file = committed_file(pool, job).await?;
    let version = pdm_db::get_version(pool, version_id)
        .await?
        .ok_or_else(|| PdmError::NotFound("version".into()))?;
    let blob = pdm_db::get_blob_by_id(pool, version.blob_id)
        .await?
        .ok_or_else(|| PdmError::NotFound("source blob".into()))?;

    // Design source: a native SolidCraft design checked in directly, otherwise
    // this version's converted `.sc` artifact (imported CAD and the like).
    let design_data: Vec<u8> = if matches!(file.ext.as_str(), "sc" | "solidcraft" | "solvecraft") {
        store
            .get_by_sha(&blob.sha256)
            .await
            .map_err(|e| PdmError::Storage(e.to_string()))?
    } else {
        let artifact = pdm_db::get_solidcraft_artifact(pool, version_id).await?;
        let sha = artifact.sha256.ok_or_else(|| {
            PdmError::Validation(format!(
                "flat-pattern generation needs a SolidCraft design: .{} has no converted .sc \
                 artifact — run Convert to SolidCraft first, or attach the DXF manually",
                file.ext
            ))
        })?;
        store
            .get_by_sha(&sha)
            .await
            .map_err(|e| PdmError::Storage(e.to_string()))?
    };

    let work = tempfile::Builder::new()
        .prefix("pdm-flat-")
        .tempdir()
        .map_err(|e| PdmError::Storage(e.to_string()))?;
    let design = work.path().join("design.sc");
    let script = work.path().join("export.json");
    let out = work.path().join("flat.dxf");
    let script_text = serde_json::json!({
        "commands": [{
            "command": "sheet.export_dxf",
            "params": { "path": out.to_str().unwrap_or_default() }
        }]
    })
    .to_string();
    tokio::fs::write(&design, &design_data)
        .await
        .map_err(|e| PdmError::Storage(e.to_string()))?;
    tokio::fs::write(&script, script_text)
        .await
        .map_err(|e| PdmError::Storage(e.to_string()))?;

    let cmd = tokio::process::Command::new(cli)
        .arg("run")
        .arg(&script)
        .arg("--in")
        .arg(&design)
        .output()
        .await
        .map_err(|e| PdmError::Internal(e.to_string()))?;
    if !cmd.status.success() {
        let stderr = String::from_utf8_lossy(&cmd.stderr);
        return Err(PdmError::Internal(format!(
            "solidcraft-cli run (sheet.export_dxf) failed: {}",
            stderr.trim()
        )));
    }
    let dxf = tokio::fs::read(&out)
        .await
        .map_err(|e| PdmError::Storage(format!("flat pattern not written: {e}")))?;
    if dxf.is_empty() {
        return Err(PdmError::Internal("flat pattern DXF is empty".into()));
    }
    let (sha, size) = store
        .put_content_addressed(&dxf)
        .await
        .map_err(|e| PdmError::Storage(e.to_string()))?;
    let size_i64 =
        i64::try_from(size).map_err(|_| PdmError::Validation("flat pattern too large".into()))?;
    pdm_db::upload_blob(pool, job.org_id, &sha, size_i64, &sha).await?;
    pdm_db::set_item_flat_pattern(pool, job.org_id, source.item_id, &sha).await?;
    // Record a revision-bound artifact so laser nests can pin this exact version.
    let _ = pdm_db::create_flat_pattern_artifact(
        pool,
        job.org_id,
        source.item_id,
        version_id,
        &sha,
        size_i64,
        "generated",
        "solidcraft-cli sheet.export_dxf",
        "mm",
        "generated flat pattern",
    )
    .await;
    info!(item = %source.item_id, version = %version_id, "flat pattern generated and attached");
    Ok(())
}

/// `laser.nest`: run the pinned Sparrow engine to nest a stored laser nest,
/// validate the layout, and persist per-sheet DXF + preview + manifest blobs and
/// the placement rows. Idempotent: a terminal nest is not re-run.
async fn run_laser_nest(
    pool: &PgPool,
    store: &Arc<BlobStore>,
    cfg: &WorkerConfig,
    job: &Job,
) -> Result<(), PdmError> {
    let nest_id = job
        .payload
        .get("nest_id")
        .and_then(|v| v.as_str())
        .and_then(|s| Uuid::parse_str(s).ok())
        .ok_or_else(|| PdmError::Validation("missing nest_id".into()))?;
    let org = job.org_id;
    let nest = pdm_db::get_laser_nest(pool, nest_id, org)
        .await?
        .ok_or_else(|| PdmError::NotFound("laser nest".into()))?;
    if matches!(nest.status.as_str(), "completed" | "cancelled") {
        debug!(nest = %nest_id, "laser.nest skipped: already {}", nest.status);
        return Ok(());
    }
    let _ = pdm_db::set_laser_nest_status(pool, org, nest_id, "running", None).await;
    let sparrow_bin = cfg
        .sparrow_cli
        .clone()
        .ok_or_else(|| PdmError::Internal("laser.nest: PDM_SPARROW_CLI not configured".into()))?;

    let part_rows = pdm_db::list_laser_nest_parts(pool, nest_id, org).await?;
    let mut geom_parts: Vec<pdm_nesting::part::Part> = Vec::new();
    let mut lines: Vec<pdm_nesting::batch::NestLine> = Vec::new();
    let mut item_by_index: Vec<(Uuid, Uuid)> = Vec::new();
    let mut resolved_layers: Vec<(String, Vec<String>, Vec<String>)> = Vec::new();
    for row in &part_rows {
        let blob = pdm_db::get_blob_by_sha(pool, org, &row.dxf_sha)
            .await?
            .ok_or_else(|| {
                PdmError::NotFound(format!("flat-pattern blob for {}", row.part_number))
            })?;
        let data = store
            .get_by_sha(&blob.sha256)
            .await
            .map_err(|e| PdmError::Storage(e.to_string()))?;
        let text = String::from_utf8(data).map_err(|e| {
            PdmError::Validation(format!(
                "{}: flat pattern is not UTF-8: {e}",
                row.part_number
            ))
        })?;
        let dxf = pdm_nesting::dxf::parse(&text)
            .map_err(|e| PdmError::Validation(format!("{}: {e}", row.part_number)))?;
        let prims = pdm_nesting::dxf::to_mm(&dxf, &row.unit, &nest.default_part_unit);
        // Resolve layers: "auto" follows flat-pattern conventions (CUT/OUTLINE
        // -> cut, BEND_*/mark/etch -> marking); otherwise the explicit layer.
        let (cut_layers, mark_layers) = if row
            .layer_cut
            .eq_ignore_ascii_case(pdm_nesting::part::LAYER_CUT_AUTO)
        {
            pdm_nesting::part::auto_layers(&dxf, &row.layers_mark.0)
        } else {
            (vec![row.layer_cut.clone()], row.layers_mark.0.clone())
        };
        resolved_layers.push((
            row.part_number.clone(),
            cut_layers.clone(),
            mark_layers.clone(),
        ));
        let mut assembled = pdm_nesting::part::assemble(
            &prims,
            &cut_layers,
            &mark_layers,
            nest.chord_tol_mm,
            nest.join_tol_mm,
        )
        .map_err(|e| PdmError::Validation(format!("{}: {:?}", row.part_number, e)))?;
        if assembled.parts.len() != 1 {
            return Err(PdmError::Validation(format!(
                "{}: expected exactly one outer island per part, found {}",
                row.part_number,
                assembled.parts.len()
            )));
        }
        item_by_index.push((row.item_id, row.file_version_id));
        let grain = match row.part_grain.as_str() {
            "along_length" => pdm_nesting::GrainDir::AlongLength,
            "along_width" => pdm_nesting::GrainDir::AlongWidth,
            _ => pdm_nesting::GrainDir::Any,
        };
        lines.push(pdm_nesting::batch::NestLine {
            part: geom_parts.len(),
            quantity: row.quantity.max(1) as u64,
            allow_mirror: row.allow_mirror,
            grain,
            step_angle: row.step_angle,
            priority: row.priority,
            use_bounding_box: row.use_bounding_box,
            label: row.label.clone(),
        });
        // "mirror of the original" lines bake the reflection into the geometry.
        let part = assembled
            .parts
            .pop()
            .ok_or_else(|| PdmError::Validation("no part to nest".into()))?;
        let part = if row.mirror {
            pdm_nesting::part::mirror_part(&part)
        } else {
            part
        };
        geom_parts.push(part);
    }

    let sheet_grain = match nest.grain_direction.as_str() {
        "along_width" => pdm_nesting::GrainDir::AlongWidth,
        "any" => pdm_nesting::GrainDir::Any,
        _ => pdm_nesting::GrainDir::AlongLength,
    };
    // Spacing: the new directional values win; legacy nests (0023 backfill)
    // fall back to the uniform part_gap.
    let (gap_x, gap_y) = if nest.spacing_x_mm > 0.0 || nest.spacing_y_mm > 0.0 {
        (nest.spacing_x_mm, nest.spacing_y_mm)
    } else {
        (nest.part_gap_mm, nest.part_gap_mm)
    };
    let nesting_corner = match nest.nesting_corner.as_str() {
        "lower_right" => pdm_nesting::NestingCorner::LowerRight,
        "upper_left" => pdm_nesting::NestingCorner::UpperLeft,
        "upper_right" => pdm_nesting::NestingCorner::UpperRight,
        _ => pdm_nesting::NestingCorner::LowerLeft,
    };
    let ncfg = pdm_nesting::NestConfig {
        stock_width: nest.stock_width_mm,
        stock_height: nest.stock_height_mm,
        edge_margin: nest.edge_margin_mm,
        gap_x,
        gap_y,
        sheet_grain,
        nesting_corner,
        origin: pdm_nesting::geom::Point::new(nest.origin_x_mm, nest.origin_y_mm),
        add_part_labels: nest.add_part_labels,
        chord_tol: nest.chord_tol_mm,
        join_tol: nest.join_tol_mm,
        time_budget_sec: nest.time_budget_sec.max(0) as u32,
        attempt_limit_sec: nest.attempt_limit_sec.max(0) as u32,
        max_sheets: nest.max_sheets.max(0) as u32,
        allowed_rotations: nest.allowed_rotations.0.clone(),
        ..Default::default()
    };

    let bin = sparrow_bin.clone();
    let seed = nest.seed;
    let parts = std::mem::take(&mut geom_parts);
    let lines = std::mem::take(&mut lines);
    let workers = std::thread::available_parallelism()
        .map(|n| n.get() as u32)
        .unwrap_or(4)
        .min(8);
    // Sheet-size definitions to work through in order. Legacy nests (no defs)
    // use the single stock size.
    let sheet_defs: Vec<pdm_core::SheetDef> = if nest.sheet_defs.0.is_empty() {
        vec![pdm_core::SheetDef {
            width_mm: nest.stock_width_mm,
            length_mm: nest.stock_height_mm,
            quantity: None,
        }]
    } else {
        nest.sheet_defs.0.clone()
    };
    let solve_defs = sheet_defs.clone();
    let solve_lines0 = lines.clone();
    let solve_parts = parts.clone();
    let base_cfg = ncfg.clone();
    let solve_bin = bin.clone();
    let solve = tokio::task::spawn_blocking(move || {
        let mut remaining: Vec<u64> = solve_lines0.iter().map(|l| l.quantity.max(1)).collect();
        // (sheet, its stock width/length) — sheets may come from different
        // definitions.
        let mut all_sheets: Vec<(pdm_nesting::batch::BatchSheet, (f64, f64))> = Vec::new();
        for def in &solve_defs {
            if remaining.iter().all(|&q| q == 0) {
                break;
            }
            let mut def_cfg = base_cfg.clone();
            def_cfg.stock_width = def.width_mm;
            def_cfg.stock_height = def.length_mm;
            // Per-size sheet cap: the requested quantity, else the run's
            // max_sheets.
            let cap = def
                .quantity
                .map(|q| q.max(0) as u32)
                .unwrap_or(base_cfg.max_sheets);
            def_cfg.max_sheets = cap.max(1).min(base_cfg.max_sheets.max(1));
            let def_lines: Vec<pdm_nesting::batch::NestLine> = solve_lines0
                .iter()
                .enumerate()
                .map(|(i, l)| {
                    let mut l = l.clone();
                    l.quantity = remaining[i];
                    l
                })
                .filter(|l| l.quantity > 0)
                .collect();
            if def_lines.is_empty() {
                continue;
            }
            let oracle = |inst: &pdm_nesting::solver::SolverInstance| {
                run_sparrow_blocking(&solve_bin, inst, seed, base_cfg.attempt_limit_sec, workers)
            };
            let dims = (def.width_mm, def.length_mm);
            let mut take = |sheets: Vec<pdm_nesting::batch::BatchSheet>| {
                for s in &sheets {
                    for c in &s.copies {
                        remaining[c.part] = remaining[c.part].saturating_sub(1);
                    }
                }
                all_sheets.extend(sheets.into_iter().map(|s| (s, dims)));
            };
            match pdm_nesting::batch::run_batch(&def_lines, &solve_parts, &def_cfg, oracle) {
                Ok(sheets) => take(sheets),
                Err(pdm_nesting::batch::BatchError::Partial { placed, .. }) => take(placed),
                // A part that does not fit this size may fit a later, larger
                // definition — skip this one.
                Err(pdm_nesting::batch::BatchError::OversizedPart(_)) => {}
                Err(e) => return Err(e),
            }
        }
        if remaining.iter().any(|&q| q > 0) {
            let unplaced: u64 = remaining.iter().sum();
            let placed: Vec<_> = all_sheets.iter().map(|(s, _)| s.clone()).collect();
            return Err(pdm_nesting::batch::BatchError::Partial {
                remaining: unplaced as usize,
                sheets: all_sheets.len(),
                placed,
            });
        }
        Ok(all_sheets)
    })
    .await
    .map_err(|e| PdmError::Internal(format!("nest spawn_blocking: {e}")))?
    .map_err(|e| PdmError::Internal(e.to_string()))?;

    // Per-line labels (part_index = line order = part_rows order).
    let labels: Vec<String> = part_rows.iter().map(|r| r.label.clone()).collect();
    let mut sheet_rows = Vec::new();
    for (idx, (sheet, (sw, sh))) in solve.iter().enumerate() {
        let nsheet = pdm_db::create_laser_nest_sheet(pool, org, nest_id, idx as i32).await?;
        // Export with this sheet's own stock size (definitions may differ).
        let mut sheet_cfg = ncfg.clone();
        sheet_cfg.stock_width = *sw;
        sheet_cfg.stock_height = *sh;
        let dxf_text =
            pdm_nesting::sheet::export_sheet_dxf(&sheet.layout, &parts, &labels, &sheet_cfg);
        let (dxf_sha, dxf_size) = put_bytes(store, pool, org, dxf_text.as_bytes()).await?;
        let svg_text =
            pdm_nesting::sheet::export_sheet_svg(&sheet.layout, &parts, &labels, &sheet_cfg);
        let (svg_sha, _) = put_bytes(store, pool, org, svg_text.as_bytes()).await?;
        for pl in &sheet.layout.placements {
            let (item_id, file_version_id) = item_by_index[pl.part_index];
            pdm_db::insert_laser_nest_placement(
                pool,
                org,
                nsheet.id,
                nest_id,
                pl.part_index as i32,
                pl.copy_index as i32,
                item_id,
                file_version_id,
                pl.transform.reflected,
                pl.transform.rotation_deg,
                pl.transform.tx,
                pl.transform.ty,
            )
            .await?;
        }
        pdm_db::set_laser_nest_sheet_output(
            pool,
            org,
            nsheet.id,
            &dxf_sha,
            dxf_size,
            Some(&svg_sha),
            Some(sheet.strip_width),
            Some(sheet.layout.utilization),
            None,
            None,
            sheet.layout.placements.len() as i32,
        )
        .await?;
        sheet_rows.push(nsheet);
    }

    // Manifest (provenance + full settings + outputs).
    let manifest = nest_manifest(
        &nest,
        &part_rows,
        &resolved_layers,
        &sheet_rows,
        &solve,
        seed,
        &bin,
    );
    let manifest_bytes =
        serde_json::to_vec_pretty(&manifest).map_err(|e| PdmError::Internal(e.to_string()))?;
    let (manifest_sha, _) = put_bytes(store, pool, org, &manifest_bytes).await?;
    // Kept on the nest, so the package can carry it.
    pdm_db::set_laser_nest_manifest(pool, org, nest_id, &manifest_sha).await?;

    // Recheck staleness: a source version that is no longer the head invalidates
    // the result (without rewriting it).
    let stale = stale_versions(pool, org, &part_rows).await?;
    let _ = pdm_db::set_laser_nest_status(pool, org, nest_id, "completed", None).await;
    for (version_id, reason) in &stale {
        let _ = pdm_db::mark_laser_nests_stale_for_version(pool, org, *version_id, reason).await;
    }
    info!(
        nest = %nest_id,
        sheets = solve.len(),
        stale = !stale.is_empty(),
        "laser nest completed"
    );
    Ok(())
}

/// Store a content-addressed byte blob and record it in the DB.
async fn put_bytes(
    store: &Arc<BlobStore>,
    pool: &PgPool,
    org: Uuid,
    bytes: &[u8],
) -> Result<(String, i64), PdmError> {
    let (sha, size) = store
        .put_content_addressed(bytes)
        .await
        .map_err(|e| PdmError::Storage(e.to_string()))?;
    let size_i64 =
        i64::try_from(size).map_err(|_| PdmError::Validation("artifact too large".into()))?;
    pdm_db::upload_blob(pool, org, &sha, size_i64, &sha).await?;
    Ok((sha, size_i64))
}

/// Run the pinned sparrow binary on one instance, synchronously (called inside
/// `spawn_blocking`). Returns the parsed solution.
fn run_sparrow_blocking(
    bin: &str,
    inst: &pdm_nesting::solver::SolverInstance,
    seed: Option<i64>,
    attempt_limit: u32,
    workers: u32,
) -> Result<pdm_nesting::solver::SolverSolution, pdm_nesting::solver::SolverError> {
    use pdm_nesting::solver::SolverError;
    let work = tempfile::TempDir::new().map_err(|e| SolverError::Malformed(e.to_string()))?;
    std::fs::create_dir_all(work.path().join("output"))
        .map_err(|e| SolverError::Malformed(e.to_string()))?;
    let input = work.path().join("inst.json");
    std::fs::write(&input, inst.to_json()).map_err(|e| SolverError::Malformed(e.to_string()))?;
    let mut cmd = std::process::Command::new(bin);
    cmd.arg("-i")
        .arg(&input)
        .arg("-t")
        .arg(attempt_limit.to_string());
    if let Some(s) = seed {
        cmd.arg("-s").arg(s.to_string());
    }
    if workers > 0 {
        cmd.arg("--workers").arg(workers.to_string());
    }
    cmd.current_dir(work.path());
    let out = cmd
        .output()
        .map_err(|e| SolverError::Malformed(format!("spawn sparrow: {e}")))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        let line = stderr
            .lines()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("no stderr");
        return Err(SolverError::Malformed(format!("sparrow failed: {line}")));
    }
    let json_path = work
        .path()
        .join("output")
        .join(format!("final_{}.json", inst.name));
    let text = std::fs::read_to_string(&json_path)
        .map_err(|e| SolverError::Malformed(format!("no final json: {e}")))?;
    pdm_nesting::solver::parse_solution(&text)
}

/// For each part, the referenced version is stale if it is no longer the item's
/// head version. Returns (version_id, reason) for every stale part.
async fn stale_versions(
    pool: &PgPool,
    org: Uuid,
    part_rows: &[pdm_core::LaserNestPart],
) -> Result<Vec<(Uuid, String)>, PdmError> {
    let mut out = Vec::new();
    for row in part_rows {
        let file = match pdm_db::get_file(pool, row.item_id).await? {
            Some(f) => f,
            None => continue,
        };
        let head = match pdm_db::get_head_version(pool, file.id).await? {
            Some(h) => h,
            None => continue,
        };
        if head.id != row.file_version_id {
            out.push((
                row.file_version_id,
                format!(
                    "source version for {} is no longer the head (now V{}-R{})",
                    row.part_number, head.version, head.revision
                ),
            ));
        }
    }
    let _ = org;
    Ok(out)
}

fn nest_manifest(
    nest: &pdm_core::LaserNest,
    part_rows: &[pdm_core::LaserNestPart],
    resolved_layers: &[(String, Vec<String>, Vec<String>)],
    sheet_rows: &[pdm_core::LaserNestSheet],
    solve: &[(pdm_nesting::batch::BatchSheet, (f64, f64))],
    seed: Option<i64>,
    bin: &str,
) -> serde_json::Value {
    serde_json::json!({
        "schema": "pdm.laserNestManifest",
        "version": 1,
        "nest": {
            "id": nest.id,
            "name": nest.name,
            // Written only when the run has solved every sheet. `nest` is the row as read at
            // the start of the run, so its own status would still say "queued".
            "status": "completed",
            "material": nest.material,
            "thickness_mm": nest.thickness_mm,
            "stock_mm": { "width": nest.stock_width_mm, "length": nest.stock_height_mm },
            "edge_margin_mm": nest.edge_margin_mm,
            "part_gap_mm": nest.part_gap_mm,
            "spacing_x_mm": nest.spacing_x_mm,
            "spacing_y_mm": nest.spacing_y_mm,
            "grain_direction": nest.grain_direction,
            "nesting_corner": nest.nesting_corner,
            "origin_mm": { "x": nest.origin_x_mm, "y": nest.origin_y_mm },
            "sheet_sheet_distance_mm": nest.sheet_sheet_distance_mm,
            "guillotine": nest.guillotine,
            "add_part_labels": nest.add_part_labels,
            "fill_direction": nest.fill_direction,
            "auto_sheet_origins": nest.auto_sheet_origins,
            "fill_all_sheets": nest.fill_all_sheets,
            "optimized_pairs": nest.optimized_pairs,
            "chord_tol_mm": nest.chord_tol_mm,
            "join_tol_mm": nest.join_tol_mm,
            "default_part_unit": nest.default_part_unit,
            "allowed_rotations": nest.allowed_rotations,
            "seed": seed,
            "solver_version": nest.solver_version,
            "engine_build": nest.engine_build,
        },
        "parts": part_rows.iter().enumerate().map(|(i, p)| {
            let (rl_cut, rl_mark) = resolved_layers
                .get(i)
                .map(|(_, c, m)| (c.clone(), m.clone()))
                .unwrap_or_else(|| (vec![p.layer_cut.clone()], p.layers_mark.0.clone()));
            serde_json::json!({
                "item_id": p.item_id,
                "file_version_id": p.file_version_id,
                "part_number": p.part_number,
                "revision": p.revision_label,
                "dxf_sha": p.dxf_sha,
                "quantity": p.quantity,
                "layer_cut": p.layer_cut,
                "layers_mark": p.layers_mark,
                "resolved_cut_layers": rl_cut,
                "resolved_mark_layers": rl_mark,
                "unit": p.unit,
                "mirror": p.mirror,
                "allow_mirror": p.allow_mirror,
                "part_grain": p.part_grain,
                "step_angle": p.step_angle,
                "priority": p.priority,
                "label": p.label,
                "use_bounding_box": p.use_bounding_box,
                "common_edge": p.common_edge,
                "min_qty": p.min_qty,
                "filler_qty": p.filler_qty,
            })
        }).collect::<Vec<_>>(),
        "sheet_defs": nest.sheet_defs,
        "sheets": sheet_rows.iter().zip(solve.iter()).map(|(row, (s, dims))| serde_json::json!({
            "sheet_index": row.sheet_index,
            "dxf_sha": row.dxf_sha,
            "preview_sha": row.preview_sha,
            "stock_mm": { "width": dims.0, "length": dims.1 },
            "strip_width_mm": s.strip_width,
            "utilization": s.layout.utilization,
            "placed_count": s.layout.placements.len(),
            "placements": s.layout.placements.iter().map(|pl| serde_json::json!({
                "part_index": pl.part_index,
                "copy_index": pl.copy_index,
                "reflected": pl.transform.reflected,
                "rotation_deg": pl.transform.rotation_deg,
                "tx": pl.transform.tx,
                "ty": pl.transform.ty,
            })).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
        "engine": { "binary": bin }
    })
}

/// A CLI failure. `Fatal`: the CLI ran and refused the file (or wrote nothing) —
/// the version records the reason and the preview stops there. `Transient`:
/// the CLI could not be run at all; the job's normal retry applies.
#[derive(Debug)]
enum CliFailure {
    Fatal(String),
    Transient(String),
}

/// The CLI's error line, capped: what a version's `thumbnail_error` shows.
fn cli_error_line(stderr: &[u8], stdout: &[u8]) -> String {
    let line = |bytes: &[u8]| -> Option<String> {
        let text = String::from_utf8_lossy(bytes).into_owned();
        text.lines()
            .find(|l| !l.trim().is_empty())
            .map(str::to_string)
    };
    line(stderr)
        .or_else(|| line(stdout))
        .unwrap_or_else(|| "the CLI gave no error output".into())
        .chars()
        .take(300)
        .collect()
}

// Conversion output must never alias an uploaded STEP input. Output cleanup
// runs before conversion, including when the committed file already is STEP.
fn converted_step_path(job_id: Uuid) -> std::path::PathBuf {
    std::env::temp_dir().join(format!("pdm-job-{job_id}-converted.step"))
}

/// `solvecraft-cli snapshot <in> --out <png>`: the PNG bytes.
async fn gpu_policy_for(pool: &PgPool, org_id: Uuid) -> String {
    match pdm_db::get_gpu_policy(pool, org_id).await {
        Ok(p) => p,
        Err(_) => "auto".into(),
    }
}

async fn cli_snapshot(
    cli: &str,
    in_path: &std::path::Path,
    out_png: &std::path::Path,
    gpu_policy: &str,
    style: &str,
) -> Result<Vec<u8>, CliFailure> {
    match cli_snapshot_attempt(cli, in_path, out_png, gpu_policy, style, false).await {
        Err(CliFailure::Fatal(message))
            if style != "viewport"
                && message.contains(
                    "product image exceeds its triangle/appearance/polyline admission cap",
                ) =>
        {
            match cli_snapshot_attempt(cli, in_path, out_png, gpu_policy, "viewport", true).await {
                Ok(png) => Ok(png),
                Err(CliFailure::Fatal(fallback)) => Err(CliFailure::Fatal(format!(
                    "{message}; coarse viewport fallback failed: {fallback}"
                ))),
                Err(e) => Err(e),
            }
        }
        result => result,
    }
}

async fn cli_snapshot_attempt(
    cli: &str,
    in_path: &std::path::Path,
    out_png: &std::path::Path,
    gpu_policy: &str,
    style: &str,
    coarse: bool,
) -> Result<Vec<u8>, CliFailure> {
    let mut command = tokio::process::Command::new(cli);
    command.kill_on_drop(true).env("SOLIDCRAFT_GPU", gpu_policy);
    if style == "viewport" {
        command.arg("snapshot").arg(in_path).args(["--view", "iso"]);
        if coarse {
            command.args(["--quality", "coarse", "--no-grid", "--no-axes"]);
        }
    } else {
        // The product renderer writes real straight-alpha PNG pixels. No color
        // keying: white parts and holes retain their correct material/coverage.
        command.arg("render").arg(in_path).args([
            "--transparent",
            "--overwrite",
            "--timeout",
            "60",
        ]);
    }
    let res = tokio::time::timeout(
        Duration::from_secs(90),
        command
            .arg("--out")
            .arg(out_png)
            .args(["--width", "512", "--height", "512"])
            .output(),
    )
    .await
    .map_err(|_| CliFailure::Transient("preview renderer timed out".into()))?
    .map_err(|e| CliFailure::Transient(format!("running the CLI: {e}")))?;
    if !res.status.success() {
        let _ = std::fs::remove_file(out_png);
        return Err(CliFailure::Fatal(format!(
            "SolidCraft preview renderer failed: {}",
            cli_error_line(&res.stderr, &res.stdout)
        )));
    }
    match std::fs::read(out_png) {
        Ok(png) if !png.is_empty() => {
            let _ = std::fs::remove_file(out_png);
            Ok(png)
        }
        Ok(_) => {
            let _ = std::fs::remove_file(out_png);
            Err(CliFailure::Fatal(
                "solvecraft-cli snapshot wrote an empty PNG".into(),
            ))
        }
        Err(e) => Err(CliFailure::Transient(format!(
            "reading the snapshot PNG: {e}"
        ))),
    }
}

/// `solvecraft-cli convert <in> --out <step>`: leaves the restated STEP on disk
/// at `out_step` (the caller snapshots it and cleans both files up).
async fn cli_convert(
    cli: &str,
    in_path: &std::path::Path,
    out_step: &std::path::Path,
) -> Result<(), CliFailure> {
    let res = tokio::time::timeout(
        Duration::from_secs(120),
        tokio::process::Command::new(cli)
            .kill_on_drop(true)
            .arg("run")
            .arg(in_path)
            .arg("--out")
            .arg(out_step)
            .arg("--quiet")
            .output(),
    )
    .await
    .map_err(|_| CliFailure::Transient("STEP conversion timed out".into()))?
    .map_err(|e| CliFailure::Transient(format!("running the CLI: {e}")))?;
    if !res.status.success() {
        let _ = std::fs::remove_file(out_step);
        return Err(CliFailure::Fatal(format!(
            "solvecraft-cli convert failed: {}",
            cli_error_line(&res.stderr, &res.stdout)
        )));
    }
    match std::fs::metadata(out_step).map(|m| m.len()) {
        Ok(n) if n > 0 => Ok(()),
        Ok(_) => {
            let _ = std::fs::remove_file(out_step);
            Err(CliFailure::Fatal(
                "solvecraft-cli convert wrote an empty STEP".into(),
            ))
        }
        Err(e) => Err(CliFailure::Transient(format!(
            "reading the converted STEP: {e}"
        ))),
    }
}

/// Render a PNG thumbnail of the committed blob and store it as a
/// content-addressed blob; returns `(sha256, size)`.
///
/// The file's real extension decides how the CLI opens it:
/// - CAD files the CLI opens directly (designs, STEP/IGES, FreeCAD, Rhino,
///   Inventor, meshes) are snapshotted as-is;
/// - B-rep CAD the CLI reads only through a conversion (SolidWorks, CATIA,
///   NX, …) — and anything whose direct snapshot failed — is restated to STEP
///   first, then the STEP is snapshotted;
/// - a file the CLI refuses both ways records the reason on the version
///   (`thumbnail_error`) and the job completes, so the portal shows why there
///   is no image instead of a silently-dead job;
/// - non-CAD files (documents, images, …) keep a clean "no preview" state.
///
/// `Ok(None)` without an error is the skipped case: the CLI is not configured,
/// or the file type is not a CAD one.
async fn render_preview(
    pool: &PgPool,
    store: &Arc<BlobStore>,
    cfg: &WorkerConfig,
    job: &Job,
) -> Result<Option<(String, i64)>, PdmError> {
    let version_id = version_id_from_job(job)
        .ok_or_else(|| PdmError::Validation("missing version_id in job payload".into()))?;

    let Some(cli) = cfg.solve_cli.as_deref() else {
        debug!(version = %version_id, "preview.render skipped: solvecraft-cli not configured");
        return Ok(None);
    };

    let version = pdm_db::get_version(pool, version_id)
        .await?
        .ok_or_else(|| PdmError::NotFound(format!("version {version_id}")))?;
    let file = committed_file(pool, job).await?;
    let blob = pdm_db::get_blob_by_id(pool, version.blob_id)
        .await?
        .ok_or_else(|| PdmError::NotFound("blob for version".into()))?;

    let plan = if file.ext.is_empty() {
        pdm_core::PreviewPlan::Direct
    } else {
        pdm_core::preview_plan(&file.filename)
    };
    if matches!(plan, pdm_core::PreviewPlan::Skip) {
        debug!(version = %version_id, filename = %file.filename, "preview.render: not a CAD file the CLI opens; no preview recorded");
        return Ok(None);
    }

    let data = store
        .get_by_sha(&blob.sha256)
        .await
        .map_err(|e| PdmError::Storage(e.to_string()))?;

    // The real extension in the temp name: a `.step` written `.solvecraft`
    // would be misread as a design.
    let ext = if file.ext.is_empty() {
        "solvecraft"
    } else {
        file.ext.as_str()
    };
    let tmp_in = std::env::temp_dir().join(format!("pdm-job-{job_id}.{ext}", job_id = job.id));
    let out_png = std::env::temp_dir().join(format!("pdm-job-{job_id}.png", job_id = job.id));
    let tmp_step = converted_step_path(job.id);
    std::fs::write(&tmp_in, &data).map_err(|e| PdmError::Storage(e.to_string()))?;
    let _ = std::fs::remove_file(&out_png);
    let _ = std::fs::remove_file(&tmp_step);

    let gpu_policy = gpu_policy_for(pool, job.org_id).await;
    let style = pdm_db::get_preview_style(pool, job.org_id).await?;
    let png = match cli_snapshot(cli, &tmp_in, &out_png, &gpu_policy, &style).await {
        Ok(png) => png,
        Err(CliFailure::Fatal(direct_err)) => {
            // The CLI could not open the file as-is: for the B-rep CAD formats
            // it restates as STEP, try `convert` and snapshot the STEP instead.
            // (For its own designs and meshes the fallback is not taken — the
            // direct error is what gets recorded.)
            let convertible = matches!(plan, pdm_core::PreviewPlan::Convert)
                || pdm_core::CAD_CONVERT_EXTS.contains(&ext);
            if !convertible {
                let _ = std::fs::remove_file(&tmp_in);
                let _ = std::fs::remove_file(&out_png);
                return fatal_preview(pool, version_id, CliFailure::Fatal(direct_err)).await;
            }
            let via_step = match cli_convert(cli, &tmp_in, &tmp_step).await {
                Ok(()) => cli_snapshot(cli, &tmp_step, &out_png, &gpu_policy, &style).await,
                Err(CliFailure::Fatal(convert_err)) => Err(CliFailure::Fatal(format!(
                    "the CLI could not open or convert this .{ext} file — {convert_err} (opening it directly: {direct_err})"
                ))),
                Err(e) => Err(e),
            };
            let _ = std::fs::remove_file(&tmp_in);
            let _ = std::fs::remove_file(&tmp_step);
            match via_step {
                Ok(png) => {
                    let _ = std::fs::remove_file(&out_png);
                    png
                }
                Err(e) => {
                    let _ = std::fs::remove_file(&out_png);
                    return fatal_preview(pool, version_id, e).await;
                }
            }
        }
        Err(CliFailure::Transient(e)) => {
            let _ = std::fs::remove_file(&tmp_in);
            let _ = std::fs::remove_file(&out_png);
            let _ = std::fs::remove_file(&tmp_step);
            return Err(PdmError::Internal(e));
        }
    };
    let _ = std::fs::remove_file(&tmp_in);
    let _ = std::fs::remove_file(&out_png);
    let _ = std::fs::remove_file(&tmp_step);

    let (sha, size) = store
        .put_content_addressed(&png)
        .await
        .map_err(|e| PdmError::Storage(e.to_string()))?;
    let size =
        i64::try_from(size).map_err(|_| PdmError::Validation("thumbnail too large".into()))?;
    Ok(Some((sha, size)))
}

/// A terminal preview failure: record the reason on the version so the portal
/// can show it, then complete the job (no retry — the file will not convert
/// on the next attempt).
async fn fatal_preview(
    pool: &PgPool,
    version_id: Uuid,
    failure: CliFailure,
) -> Result<Option<(String, i64)>, PdmError> {
    let msg = match failure {
        CliFailure::Fatal(m) | CliFailure::Transient(m) => m,
    };
    if let Err(e) = pdm_db::store_thumbnail_error(pool, version_id, &msg).await {
        warn!(version = %version_id, error = %e, "recording the preview error failed");
    }
    info!(version = %version_id, error = %msg, "preview.render: no thumbnail recorded");
    Ok(None)
}

/// Run `solvecraft-cli convert` on the committed blob when its file is in a format the
/// converter reads (Inventor, FreeCAD, Rhino, STEP, IGES): the restated STEP is stored as a
/// content-addressed blob and recorded on the version. Returns `Ok(None)` when the CLI is not
/// configured or the file type is not convertible — the job is marked done (skipped).
async fn convert_cad(
    pool: &PgPool,
    store: &Arc<BlobStore>,
    cfg: &WorkerConfig,
    job: &Job,
) -> Result<Option<(String, i64)>, PdmError> {
    let version_id = version_id_from_job(job)
        .ok_or_else(|| PdmError::Validation("missing version_id in job payload".into()))?;
    let _item_id = item_id_from_job(job)
        .ok_or_else(|| PdmError::Validation("missing item_id in job payload".into()))?;

    let Some(cli) = cfg.solve_cli.as_deref() else {
        debug!(version = %version_id, "cad.convert skipped: solvecraft-cli not configured");
        return Ok(None);
    };

    let version = pdm_db::get_version(pool, version_id)
        .await?
        .ok_or_else(|| PdmError::NotFound(format!("version {version_id}")))?;
    let file = committed_file(pool, job).await?;
    let ext = file.ext;
    if !pdm_core::CAD_CONVERT_EXTS.contains(&ext.as_str()) {
        debug!(version = %version_id, ext = %ext, "cad.convert skipped: not a convertible extension");
        return Ok(None);
    }

    let blob = pdm_db::get_blob_by_id(pool, version.blob_id)
        .await?
        .ok_or_else(|| PdmError::NotFound("blob for version".into()))?;
    let data = store
        .get_by_sha(&blob.sha256)
        .await
        .map_err(|e| PdmError::Storage(e.to_string()))?;

    let tmp_in = std::env::temp_dir().join(format!("pdm-job-{}.{}", job.id, ext));
    let tmp_out = converted_step_path(job.id);
    std::fs::write(&tmp_in, &data).map_err(|e| PdmError::Storage(e.to_string()))?;
    let _ = std::fs::remove_file(&tmp_out);

    let res = cli_convert(cli, &tmp_in, &tmp_out).await;
    let _ = std::fs::remove_file(&tmp_in);

    if let Err(failure) = res {
        let stderr = match failure {
            CliFailure::Fatal(e) | CliFailure::Transient(e) => e,
        };
        let _ = std::fs::remove_file(&tmp_out);
        return Err(PdmError::Internal(format!(
            "solvecraft-cli convert failed: {stderr}"
        )));
    }

    let step = std::fs::read(&tmp_out).map_err(|e| PdmError::Storage(e.to_string()))?;
    let _ = std::fs::remove_file(&tmp_out);
    if step.is_empty() {
        return Err(PdmError::Internal(
            "solvecraft-cli convert wrote an empty STEP".into(),
        ));
    }

    let (sha, size) = store
        .put_content_addressed(&step)
        .await
        .map_err(|e| PdmError::Storage(e.to_string()))?;
    let size =
        i64::try_from(size).map_err(|_| PdmError::Validation("converted STEP too large".into()))?;
    Ok(Some((sha, size)))
}

/// Save a native artifact alongside its source; never change the committed head.
///
/// The file's format picks the converter ([`pdm_core::sc_converter`]): Dutchman
/// decodes the native CAD formats into a design with editable feature history
/// where it maps (pulling the newest GitHub release on demand when its binary
/// is missing); the SolidCraft CLI restates the remainder (meshes, IGES) as
/// geometry only.
async fn convert_to_solidcraft(
    pool: &PgPool,
    store: &Arc<BlobStore>,
    cfg: &WorkerConfig,
    job: &Job,
) -> Result<(), PdmError> {
    let version_id = version_id_from_job(job)
        .ok_or_else(|| PdmError::Validation("missing version_id".into()))?;
    let source = pdm_db::version_source(pool, job.org_id, version_id)
        .await?
        .ok_or_else(|| PdmError::NotFound("source version".into()))?;
    let file = committed_file(pool, job).await?;
    let Some(converter) = pdm_core::sc_converter(&file.filename) else {
        return Err(PdmError::Validation(
            "file type is not supported for SolidCraft conversion".into(),
        ));
    };
    let version = pdm_db::get_version(pool, version_id)
        .await?
        .ok_or_else(|| PdmError::NotFound("source version".into()))?;
    let blob = pdm_db::get_blob_by_id(pool, version.blob_id)
        .await?
        .ok_or_else(|| PdmError::NotFound("source blob".into()))?;
    let data = store
        .get_by_sha(&blob.sha256)
        .await
        .map_err(|e| PdmError::Storage(e.to_string()))?;
    let work = tempfile::Builder::new()
        .prefix("pdm-native-")
        .tempdir()
        .map_err(|e| PdmError::Storage(e.to_string()))?;
    let input = work
        .path()
        .join(pdm_core::solidcraft_artifact_filename(
            &source.filename,
            source.seq,
        ))
        .with_extension(&file.ext);
    let output = work.path().join("converted.sc");
    tokio::fs::write(&input, data)
        .await
        .map_err(|e| PdmError::Storage(e.to_string()))?;

    let settings = pdm_db::get_dutchman_settings(pool, job.org_id).await?;
    match converter {
        pdm_core::ScConverter::Dutchman => {
            let dutchman = cfg
                .dutchman_cli
                .clone()
                .unwrap_or_else(pdm_update::dutchman_cli_path);
            // No binary, or the wrong release channel: pull what the org's
            // settings reference before converting (a disabled
            // PDM_DUTCHMAN_UPDATE keeps the fallback).
            match pdm_update::ensure_dutchman_for(
                std::path::Path::new(&dutchman),
                &settings.channel,
            )
            .await
            {
                Ok(Some(tag)) => {
                    info!(channel = %settings.channel, tag = %tag, "pulled the referenced Dutchman release for conversion");
                }
                Ok(None) => {}
                Err(e) => debug!(%e, "Dutchman release pull failed"),
            }
            if std::path::Path::new(&dutchman).exists() {
                run_dutchman(cfg, &dutchman, &input, &output, &settings).await?;
            } else if let Some(cli) = cfg.solve_cli.as_deref()
                && pdm_core::CAD_CONVERT_EXTS.contains(&file.ext.as_str())
            {
                warn!(
                    ext = %file.ext,
                    "Dutchman is not installed; restating this .{} through the SolidCraft CLI instead (no feature history)",
                    file.ext
                );
                run_solidcraft_cli(cli, &input, &output).await?;
            } else {
                return Err(PdmError::BadRequest(format!(
                    "Dutchman is not installed ({dutchman} is missing); set PDM_DUTCHMAN_CLI or keep PDM_DUTCHMAN_UPDATE enabled so the PDM pulls the latest release"
                )));
            }
        }
        pdm_core::ScConverter::SolidcraftCli => {
            let cli = cfg
                .solve_cli
                .as_deref()
                .ok_or_else(|| PdmError::BadRequest("SolidCraft CLI is not configured".into()))?;
            run_solidcraft_cli(cli, &input, &output).await?;
        }
    }

    let size = tokio::fs::metadata(&output)
        .await
        .map_err(|e| PdmError::Storage(e.to_string()))?
        .len();
    if size == 0 || size > 512 * 1024 * 1024 {
        return Err(PdmError::Validation(
            "converted design is empty or exceeds 512 MiB".into(),
        ));
    }
    let data = tokio::fs::read(output)
        .await
        .map_err(|e| PdmError::Storage(e.to_string()))?;
    let (sha, size) = store
        .put_content_addressed(&data)
        .await
        .map_err(|e| PdmError::Storage(e.to_string()))?;
    let size = i64::try_from(size)
        .map_err(|_| PdmError::Validation("converted design is too large".into()))?;
    pdm_db::store_solidcraft_artifact(pool, version_id, job.id, &sha, size).await
}

/// `dutchman convert <in> <out.sc> --preset <org setting> --quiet --json`: decodes
/// the native file into a SolidCraft design, keeping editable feature history where
/// it maps. History is checked in the PDM's SolidCraft CLI when one is configured
/// (`--solidcraft`) unless the org disabled verification (`--no-verify`). Dutchman's
/// exit code is 0 when the design was written, 1 when it was not.
async fn run_dutchman(
    cfg: &WorkerConfig,
    dutchman: &str,
    input: &std::path::Path,
    output: &std::path::Path,
    settings: &pdm_core::DutchmanSettings,
) -> Result<(), PdmError> {
    let mut command = tokio::process::Command::new(dutchman);
    command
        .kill_on_drop(true)
        .arg("convert")
        .arg(input)
        .arg("-o")
        .arg(output)
        .arg("--preset")
        .arg(settings.preset.as_str())
        .arg("--quiet")
        .arg("--json");
    if settings.part_positions != "smart" {
        command
            .arg("--part-positions")
            .arg(settings.part_positions.as_str());
    }
    if settings.verify {
        if let Some(cli) = cfg.solve_cli.as_deref() {
            command.arg("--solidcraft").arg(cli);
        }
    } else {
        command.arg("--no-verify");
    }
    let result = tokio::time::timeout(dutchman_budget(cfg, settings), command.output())
        .await
        .map_err(|_| {
            PdmError::Internal(format!(
                "Dutchman conversion timed out after {} s",
                cfg.dutchman_timeout.as_secs()
            ))
        })?
        .map_err(|e| PdmError::Internal(format!("running Dutchman: {e}")))?;
    if !result.status.success() {
        return Err(PdmError::Validation(format!(
            "Dutchman conversion failed: {}",
            cli_error_line(&result.stderr, &result.stdout)
        )));
    }
    // The `--json` final report (schema dutchman.convert/1) on stdout names what
    // carried across: log the history mode and how many features rebuilt.
    let report =
        serde_json::from_str::<serde_json::Value>(&String::from_utf8_lossy(&result.stdout));
    if let Ok(report) = report {
        let mode = report
            .get("mode")
            .and_then(|m| m.as_str())
            .unwrap_or("unknown");
        let history = report.get("history");
        let rebuilt = history
            .and_then(|h| h.get("rebuilt"))
            .and_then(|v| v.as_u64());
        let needing = history
            .and_then(|h| h.get("needing"))
            .and_then(|v| v.as_u64());
        info!(mode, rebuilt, needing, "Dutchman conversion finished");
    }
    Ok(())
}

/// The wall-clock budget for a Dutchman run: the org's override when set, else
/// the worker default.
fn dutchman_budget(cfg: &WorkerConfig, settings: &pdm_core::DutchmanSettings) -> Duration {
    if settings.timeout_secs > 0 {
        Duration::from_secs(settings.timeout_secs)
    } else {
        cfg.dutchman_timeout
    }
}

/// `onshape.import`: fetch one Onshape Part Studio into the vault as a
/// `.onshape-bundle` version through `dutchman onshape fetch` (using the org's
/// Onshape API keys), then hand the conversion to the regular `cad.solidcraft`
/// job. The referenced Dutchman build must carry the Onshape reader
/// (nightly / onshape-preview channels).
async fn onshape_import(
    pool: &PgPool,
    store: &Arc<BlobStore>,
    cfg: &WorkerConfig,
    job: &Job,
) -> Result<(), PdmError> {
    let url = job
        .payload
        .get("url")
        .and_then(|v| v.as_str())
        .ok_or_else(|| PdmError::Validation("missing url in job payload".into()))?;
    let item_id = item_id_from_job(job)
        .ok_or_else(|| PdmError::Validation("missing item_id in job payload".into()))?;
    let author_id = job
        .payload
        .get("author_id")
        .and_then(|v| v.as_str())
        .and_then(|s| Uuid::parse_str(s).ok())
        .ok_or_else(|| PdmError::Validation("missing author_id in job payload".into()))?;

    let item = pdm_db::get_item(pool, item_id, job.org_id)
        .await?
        .ok_or_else(|| PdmError::NotFound(format!("item {item_id}")))?;
    let settings = pdm_db::get_dutchman_settings(pool, job.org_id).await?;

    let dutchman = cfg
        .dutchman_cli
        .clone()
        .unwrap_or_else(pdm_update::dutchman_cli_path);
    match pdm_update::ensure_dutchman_for(std::path::Path::new(&dutchman), &settings.channel).await
    {
        Ok(Some(tag)) => {
            info!(channel = %settings.channel, tag = %tag, "pulled the referenced Dutchman release for the Onshape import");
        }
        Ok(None) => {}
        Err(e) => {
            return Err(PdmError::Internal(format!(
                "pulling Dutchman for channel {channel} failed: {e}",
                channel = settings.channel
            )));
        }
    }
    if !std::path::Path::new(&dutchman).exists() {
        return Err(PdmError::BadRequest(
            "Dutchman is not installed and updates are disabled (PDM_DUTCHMAN_UPDATE=0)".into(),
        ));
    }

    // The org's Onshape keys, handed to dutchman as environment variables —
    // the documented script path (ONSHAPE_ACCESS_KEY / ONSHAPE_SECRET_KEY).
    let onshape = pdm_db::get_onshape_settings(pool, job.org_id).await?;
    let access_key = onshape
        .get("access_key")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let secret_key = onshape.get("secret").and_then(|v| v.as_str()).unwrap_or("");
    if access_key.is_empty() || secret_key.is_empty() {
        return Err(PdmError::BadRequest(
            "Onshape API keys are not configured — set them in Settings".into(),
        ));
    }

    let work = tempfile::Builder::new()
        .prefix("pdm-onshape-")
        .tempdir()
        .map_err(|e| PdmError::Storage(e.to_string()))?;
    let bundle = work.path().join("import.onshape-bundle");
    let max_calls = settings.onshape_call_budget().to_string();
    let result = tokio::time::timeout(
        dutchman_budget(cfg, &settings),
        tokio::process::Command::new(&dutchman)
            .kill_on_drop(true)
            .env("ONSHAPE_ACCESS_KEY", access_key)
            .env("ONSHAPE_SECRET_KEY", secret_key)
            .arg("onshape")
            .arg("fetch")
            .arg(url)
            .arg("--out")
            .arg(&bundle)
            .arg("--max-calls")
            .arg(&max_calls)
            .arg("--quiet")
            .arg("--json")
            .output(),
    )
    .await
    .map_err(|_| {
        PdmError::Internal(format!(
            "Onshape fetch timed out after {} s",
            dutchman_budget(cfg, &settings).as_secs()
        ))
    })?
    .map_err(|e| PdmError::Internal(format!("running Dutchman onshape fetch: {e}")))?;
    if !result.status.success() {
        if String::from_utf8_lossy(&result.stderr).contains("unrecognized subcommand 'onshape'") {
            return Err(PdmError::Validation(format!(
                "The installed Dutchman build on channel '{}' has no Onshape reader. Select and update an Onshape-capable release (for example onshape-preview-2) in System → Dutchman, then retry. Alternatively, upload an exported STEP through the project's Manual STEP fallback.",
                settings.channel
            )));
        }
        let report_err =
            serde_json::from_str::<serde_json::Value>(&String::from_utf8_lossy(&result.stdout))
                .ok()
                .and_then(|r| {
                    r.get("error")
                        .and_then(|e| e.get("message"))
                        .and_then(|m| m.as_str())
                        .map(str::to_string)
                });
        return Err(PdmError::Validation(format!(
            "Onshape fetch failed: {}",
            report_err.unwrap_or_else(|| cli_error_line(&result.stderr, &result.stdout))
        )));
    }
    // The fetch report (schema dutchman.onshape-fetch/1) names what went wrong when ok=false.
    if let Ok(report) =
        serde_json::from_str::<serde_json::Value>(&String::from_utf8_lossy(&result.stdout))
        && report.get("ok").and_then(|v| v.as_bool()) == Some(false)
    {
        let message = report
            .get("error")
            .and_then(|e| e.get("message"))
            .and_then(|m| m.as_str())
            .unwrap_or("the fetch reported a failure");
        return Err(PdmError::Validation(format!(
            "Onshape fetch failed: {message}"
        )));
    }

    let size = std::fs::metadata(&bundle)
        .map_err(|e| PdmError::Storage(e.to_string()))?
        .len();
    if size == 0 || size > 512 * 1024 * 1024 {
        return Err(PdmError::Validation(
            "Onshape bundle is empty or exceeds 512 MiB".into(),
        ));
    }
    let data = std::fs::read(&bundle).map_err(|e| PdmError::Storage(e.to_string()))?;

    let (sha, size) = store
        .put_content_addressed(&data)
        .await
        .map_err(|e| PdmError::Storage(e.to_string()))?;
    let size =
        i64::try_from(size).map_err(|_| PdmError::Validation("Onshape bundle too large".into()))?;
    let blob = pdm_db::upload_blob(pool, job.org_id, &sha, size, &sha).await?;
    let filename = format!("{}.onshape-bundle", item.part_number);
    let file = pdm_db::ensure_file(pool, item_id, &filename, "application/zip").await?;
    let head = pdm_db::get_head_version(pool, file.id).await?;
    let version = pdm_db::create_version(
        pool,
        file.id,
        blob.id,
        author_id,
        &format!("Imported from Onshape {url}"),
        head.as_ref().map(|h| h.id),
        "onshape-bundle",
        false,
    )
    .await?;

    // The usual follow-ups: measurements/preview (a clean skip) and the
    // SolidCraft conversion the import exists for. queue_solidcraft_conversion
    // records the job id on the version so the artifact store cannot be
    // superseded by a stale claim.
    let follow_up =
        serde_json::json!({ "version_id": version.id, "item_id": item_id, "filename": filename });
    let _ = pdm_db::enqueue_job(pool, job.org_id, "design.analyze", &follow_up).await;
    let _ = pdm_db::enqueue_job(pool, job.org_id, "preview.render", &follow_up).await;
    let source = pdm_db::VersionSource {
        item_id: item.id,
        filename: filename.clone(),
        seq: version.seq,
        native_format: "onshape-bundle".into(),
    };
    pdm_db::queue_solidcraft_conversion(pool, job.org_id, version.id, &source).await?;
    info!(item = %item_id, version = %version.id, "Onshape import checked in as a bundle version");
    Ok(())
}

/// `solidcraft-cli run <in> --save <out> --quiet`: restates a file the Dutchman
/// release does not decode (meshes, IGES) into a native design, geometry only.
async fn run_solidcraft_cli(
    cli: &str,
    input: &std::path::Path,
    output: &std::path::Path,
) -> Result<(), PdmError> {
    let result = tokio::time::timeout(
        Duration::from_secs(120),
        tokio::process::Command::new(cli)
            .kill_on_drop(true)
            .arg("run")
            .arg(input)
            .arg("--save")
            .arg(output)
            .arg("--quiet")
            .output(),
    )
    .await
    .map_err(|_| PdmError::Internal("SolidCraft conversion timed out".into()))?
    .map_err(|e| PdmError::Internal(format!("running SolidCraft conversion: {e}")))?;
    if !result.status.success() {
        return Err(PdmError::Validation(format!(
            "SolidCraft conversion failed: {}",
            cli_error_line(&result.stderr, &result.stdout)
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::postgres::PgPoolOptions;

    #[cfg(unix)]
    #[tokio::test]
    async fn product_admission_failure_retries_coarse_viewport() {
        use std::os::unix::fs::PermissionsExt;
        let Ok(dir) = tempfile::tempdir() else {
            return;
        };
        let script = dir.path().join("fake-cli");
        let script_bytes = r##"#!/bin/sh
if [ "$1" = render ]; then
  printf '%s\n' 'error: LimitExceeded: product image exceeds its triangle/appearance/polyline admission cap' >&2
  exit 1
fi
[ "$1" = snapshot ] || exit 2
case " $* " in *" --quality coarse "*) ;; *) exit 3 ;; esac
while [ "$#" -gt 0 ]; do
  if [ "$1" = --out ]; then shift; printf 'preview' > "$1"; exit 0; fi
  shift
done
exit 4
"##;
        let Ok(()) = std::fs::write(&script, script_bytes).and_then(|_| {
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755))
        }) else {
            return;
        };
        let Some(cli) = script.to_str() else {
            return;
        };
        let Ok(png) = cli_snapshot(
            cli,
            &dir.path().join("model.step"),
            &dir.path().join("preview.png"),
            "auto",
            "product",
        )
        .await
        else {
            return;
        };
        assert_eq!(png, b"preview");
    }

    fn job_with(kind: &str, payload: serde_json::Value) -> Job {
        Job {
            id: Uuid::new_v4(),
            org_id: Uuid::new_v4(),
            kind: kind.to_string(),
            payload,
            status: "queued".to_string(),
            attempts: 0,
            max_attempts: 3,
            last_error: String::new(),
            created_at: chrono::Utc::now(),
        }
    }

    fn cfg_with_cli(cli: Option<&str>) -> WorkerConfig {
        WorkerConfig {
            poll_interval: Duration::from_millis(1),
            batch_size: 1,
            concurrency: 1,
            solve_cli: cli.map(str::to_string),
            dutchman_cli: None,
            dutchman_timeout: Duration::from_secs(120),
            sparrow_cli: None,
        }
    }

    /// A lazy pool that never connects: enough for job kinds that skip
    /// before touching the database.
    fn lazy_pool() -> Option<PgPool> {
        sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgres://pdm:pdm@localhost:5432/pdm-worker-test")
            .ok()
    }

    async fn local_store() -> Option<Arc<BlobStore>> {
        let dir = std::env::temp_dir().join(format!("pdm-worker-test-{}", Uuid::new_v4()));
        let _ = std::fs::create_dir_all(&dir);
        pdm_storage::BlobStore::create(&pdm_storage::Backend::Local(dir))
            .await
            .ok()
            .map(Arc::new)
    }

    #[tokio::test]
    async fn unknown_job_kind_is_done() {
        let store = local_store().await;
        let Some(store) = store else { return };
        let Some(pool) = lazy_pool() else { return };
        let job = job_with("totally.unknown", serde_json::json!({}));
        let outcome = run_job_kind(&pool, &store, &cfg_with_cli(None), &job).await;
        assert!(matches!(outcome, Ok(Outcome::Done)));
    }

    #[tokio::test]
    async fn preview_render_without_cli_skips() {
        let store = local_store().await;
        let Some(store) = store else { return };
        let Some(pool) = lazy_pool() else { return };
        let job = job_with(
            "preview.render",
            serde_json::json!({ "version_id": Uuid::new_v4().to_string() }),
        );
        let outcome = run_job_kind(&pool, &store, &cfg_with_cli(None), &job).await;
        assert!(matches!(outcome, Ok(Outcome::Done)));
    }

    #[tokio::test]
    async fn preview_render_missing_version_id_fails_validation() {
        let store = local_store().await;
        let Some(store) = store else { return };
        let Some(pool) = lazy_pool() else { return };
        let job = job_with("preview.render", serde_json::json!({}));
        let outcome = run_job_kind(&pool, &store, &cfg_with_cli(None), &job).await;
        assert!(matches!(outcome, Err(PdmError::Validation(_))));
    }

    #[tokio::test]
    async fn cad_convert_without_cli_skips() {
        // `convert_cad` resolves the ids, then skips before the CLI check — but both ids must
        // be present for the job to be actionable.
        let store = local_store().await;
        let Some(store) = store else { return };
        let Some(pool) = lazy_pool() else { return };
        let job = job_with(
            "cad.convert",
            serde_json::json!({
                "version_id": Uuid::new_v4().to_string(),
                "item_id": Uuid::new_v4().to_string(),
            }),
        );
        let outcome = run_job_kind(&pool, &store, &cfg_with_cli(None), &job).await;
        assert!(matches!(outcome, Ok(Outcome::Done)));
    }

    #[tokio::test]
    async fn cad_convert_missing_item_id_fails_validation() {
        let store = local_store().await;
        let Some(store) = store else { return };
        let Some(pool) = lazy_pool() else { return };
        let job = job_with(
            "cad.convert",
            serde_json::json!({ "version_id": Uuid::new_v4().to_string() }),
        );
        let outcome = run_job_kind(&pool, &store, &cfg_with_cli(None), &job).await;
        assert!(matches!(outcome, Err(PdmError::Validation(_))));
    }

    #[test]
    fn step_output_cleanup_preserves_the_committed_input() -> Result<(), std::io::Error> {
        let job_id = Uuid::new_v4();
        let input = std::env::temp_dir().join(format!("pdm-job-{job_id}.step"));
        let output = converted_step_path(job_id);
        let source = b"ISO-10303-21; committed STEP bytes";
        std::fs::write(&input, source)?;
        std::fs::write(&output, b"stale conversion output")?;
        std::fs::remove_file(&output)?;
        let preserved = std::fs::read(&input);
        let _ = std::fs::remove_file(&input);
        assert_eq!(preserved?, source);
        Ok(())
    }

    #[test]
    fn cli_error_line_takes_the_first_nonempty_line_capped() {
        let stderr = b"error: /tmp/x.sldprt: not a supported format\nsecond line\nthird line\n";
        assert_eq!(
            cli_error_line(stderr, b""),
            "error: /tmp/x.sldprt: not a supported format"
        );
        let long = format!("error: {}", "x".repeat(500));
        assert_eq!(
            cli_error_line(long.as_bytes(), b""),
            format!("error: {}", "x".repeat(293))
        );
        assert_eq!(cli_error_line(b"  \n", b"fallback line\n"), "fallback line");
        assert_eq!(cli_error_line(b"", b""), "the CLI gave no error output");
    }

    #[tokio::test]
    async fn committed_file_without_item_id_is_legacy() -> Result<(), PdmError> {
        let Some(pool) = lazy_pool() else {
            return Ok(());
        };
        let job = job_with(
            "preview.render",
            serde_json::json!({ "version_id": Uuid::new_v4().to_string() }),
        );
        let file = committed_file(&pool, &job).await?;
        assert!(file.filename.is_empty() && file.ext.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn design_analyze_without_cli_skips_to_measurements() {
        // `analyze_design` resolves the version/blob before the CLI check, so this
        // needs a live DB (PDM_TEST_DATABASE_URL); skipped otherwise.
        let url = std::env::var("PDM_TEST_DATABASE_URL").ok();
        let Some(url) = url else {
            eprintln!("PDM_TEST_DATABASE_URL not set; skipping design.analyze skip test");
            return;
        };
        let pool = match PgPoolOptions::new().max_connections(4).connect(&url).await {
            Ok(p) => p,
            Err(e) => {
                eprintln!("cannot connect to {url}: {e}");
                return;
            }
        };
        pdm_db::migrate(&pool).await.ok();
        let stamp = Uuid::new_v4().to_string();
        let org = match pdm_db::create_org(&pool, &format!("worker-test-{stamp}")).await {
            Ok(o) => o,
            Err(_) => return,
        };
        let user = match pdm_db::create_user(
            &pool,
            org.id,
            &format!("w-{stamp}@test.dev"),
            "Worker Test",
            "editor",
            None,
        )
        .await
        {
            Ok(u) => u,
            Err(_) => return,
        };
        let project = match pdm_db::create_project(
            &pool,
            org.id,
            &format!("worker-proj-{stamp}"),
            "",
        )
        .await
        {
            Ok(p) => p,
            Err(_) => return,
        };
        let item = match pdm_db::create_item(
            &pool,
            org.id,
            project.id,
            None,
            &format!("BE-WKR-{stamp}"),
            "",
            "BE",
            Some(user.id),
        )
        .await
        {
            Ok(i) => i,
            Err(_) => return,
        };
        let file =
            match pdm_db::ensure_file(&pool, item.id, "w.solvecraft", "application/octet-stream")
                .await
            {
                Ok(f) => f,
                Err(_) => return,
            };
        let blob = match pdm_db::upload_blob(
            &pool,
            org.id,
            "3333333333333333333333333333333333333333333333333333333333333333",
            1,
            "3333333333333333333333333333333333333333333333333333333333333333",
        )
        .await
        {
            Ok(b) => b,
            Err(_) => return,
        };
        let version = match pdm_db::create_version(
            &pool,
            file.id,
            blob.id,
            user.id,
            "worker test",
            None,
            "",
            false,
        )
        .await
        {
            Ok(v) => v,
            Err(_) => return,
        };

        let store = local_store().await;
        let Some(store) = store else { return };
        let job = job_with(
            "design.analyze",
            serde_json::json!({ "version_id": version.id.to_string() }),
        );
        let outcome = run_job_kind(&pool, &store, &cfg_with_cli(None), &job).await;
        assert!(
            matches!(&outcome, Ok(Outcome::Measurements(m)) if m.get("skipped").and_then(|v| v.as_bool()) == Some(true)),
            "design.analyze without CLI should skip: {outcome:?}"
        );
    }

    /// A `cad.solidcraft` job on a `.f3d` (a Dutchman-only format) converts
    /// through the Dutchman binary and stores the design it wrote. Needs a
    /// live DB (PDM_TEST_DATABASE_URL); skipped otherwise.
    #[cfg(unix)]
    #[tokio::test]
    async fn cad_solidcraft_converts_through_dutchman() {
        use std::os::unix::fs::PermissionsExt;
        // The fake binary must win: no release pulls from GitHub in this test.
        unsafe { std::env::set_var("PDM_DUTCHMAN_UPDATE", "0") };
        let url = std::env::var("PDM_TEST_DATABASE_URL").ok();
        let Some(url) = url else {
            eprintln!("PDM_TEST_DATABASE_URL not set; skipping the Dutchman conversion test");
            return;
        };
        let pool = match PgPoolOptions::new().max_connections(4).connect(&url).await {
            Ok(p) => p,
            Err(e) => {
                eprintln!("cannot connect to {url}: {e}");
                return;
            }
        };
        pdm_db::migrate(&pool).await.ok();
        let stamp = Uuid::new_v4().to_string();
        let org = match pdm_db::create_org(&pool, &format!("dutchman-test-{stamp}")).await {
            Ok(o) => o,
            Err(_) => return,
        };
        let user = match pdm_db::create_user(
            &pool,
            org.id,
            &format!("d-{stamp}@test.dev"),
            "Dutchman Test",
            "editor",
            None,
        )
        .await
        {
            Ok(u) => u,
            Err(_) => return,
        };
        let project = match pdm_db::create_project(
            &pool,
            org.id,
            &format!("dutchman-proj-{stamp}"),
            "",
        )
        .await
        {
            Ok(p) => p,
            Err(_) => return,
        };
        let item = match pdm_db::create_item(
            &pool,
            org.id,
            project.id,
            None,
            &format!("BE-DNM-{stamp}"),
            "",
            "BE",
            Some(user.id),
        )
        .await
        {
            Ok(i) => i,
            Err(_) => return,
        };
        let file =
            match pdm_db::ensure_file(&pool, item.id, "bracket.f3d", "application/octet-stream")
                .await
            {
                Ok(f) => f,
                Err(_) => return,
            };
        let store = local_store().await;
        let Some(store) = store else { return };
        let source = b"f3d-bytes".to_vec();
        let (sha, size) = match store.put_content_addressed(&source).await {
            Ok(v) => v,
            Err(_) => return,
        };
        let blob =
            match pdm_db::upload_blob(&pool, org.id, &sha, i64::try_from(size).unwrap_or(1), &sha)
                .await
            {
                Ok(b) => b,
                Err(_) => return,
            };
        let version = match pdm_db::create_version(
            &pool,
            file.id,
            blob.id,
            user.id,
            "dutchman test",
            None,
            "",
            false,
        )
        .await
        {
            Ok(v) => v,
            Err(_) => return,
        };

        // A stand-in dutchman: writes the design named by -o and prints the
        // report line the worker logs.
        let Ok(dir) = tempfile::tempdir() else {
            return;
        };
        let fake = dir.path().join("dutchman");
        let written = std::fs::write(
            &fake,
            r##"#!/bin/sh
out=""
while [ $# -gt 0 ]; do
  case "$1" in
    -o|--out) out="$2"; shift 2 ;;
    *) shift ;;
  esac
done
[ -n "$out" ] || exit 2
printf 'sc-bytes' > "$out"
printf '{"ok":true,"mode":"history","history":{"rebuilt":3,"needing":3}}\n'
"##,
        );
        let Ok(()) = written
            .and_then(|_| std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)))
        else {
            return;
        };
        let Some(dutchman_str) = fake.to_str() else {
            return;
        };

        let mut cfg = cfg_with_cli(None);
        cfg.dutchman_cli = Some(dutchman_str.to_string());
        let job = job_with(
            "cad.solidcraft",
            serde_json::json!({
                "version_id": version.id,
                "item_id": item.id,
                "filename": "bracket.f3d"
            }),
        );
        let outcome = run_job_kind(&pool, &store, &cfg, &job).await;
        assert!(
            matches!(&outcome, Ok(Outcome::Done)),
            "cad.solidcraft through Dutchman should store the design: {outcome:?}"
        );
        let Ok(artifact) = pdm_db::get_solidcraft_artifact(&pool, version.id).await else {
            return;
        };
        let Ok((want, _)) = store.put_content_addressed(b"sc-bytes").await else {
            return;
        };
        assert_eq!(artifact.sha256.as_deref(), Some(want.as_str()));
        assert!(artifact.error.is_none(), "no error should be recorded");
    }
}
