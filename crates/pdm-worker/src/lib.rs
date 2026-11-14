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
        other => {
            debug!(kind = other, "unknown job kind; marking done");
            Ok(Outcome::Done)
        }
    }
}

async fn run_job(pool: &PgPool, store: &Arc<BlobStore>, cfg: &WorkerConfig, job: &Job) {
    let result = match run_job_kind(pool, store, cfg, job).await {
        Ok(Outcome::Measurements(m)) => store_measurements(pool, job, m).await,
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
    }
}

fn version_id_from_job(job: &Job) -> Option<Uuid> {
    job.payload
        .get("version_id")
        .and_then(|v| v.as_str())
        .and_then(|s| Uuid::parse_str(s).ok())
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

/// A CLI failure. `Fatal`: the CLI ran and refused the file (or wrote nothing) —
/// the version records the reason and the preview stops there. `Transient`:
/// the CLI could not be run at all — the job's normal retry applies.
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
            if style != "viewport" && message.contains("product image exceeds its triangle/appearance/polyline admission cap") =>
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
async fn convert_to_solidcraft(
    pool: &PgPool,
    store: &Arc<BlobStore>,
    cfg: &WorkerConfig,
    job: &Job,
) -> Result<(), PdmError> {
    let version_id = version_id_from_job(job)
        .ok_or_else(|| PdmError::Validation("missing version_id".into()))?;
    let cli = cfg
        .solve_cli
        .as_deref()
        .ok_or_else(|| PdmError::BadRequest("SolidCraft CLI is not configured".into()))?;
    let source = pdm_db::version_source(pool, job.org_id, version_id)
        .await?
        .ok_or_else(|| PdmError::NotFound("source version".into()))?;
    let file = committed_file(pool, job).await?;
    if !pdm_core::can_convert_to_solidcraft(&file.filename) {
        return Err(PdmError::Validation(
            "file type is not supported for SolidCraft conversion".into(),
        ));
    }
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
    let result = tokio::time::timeout(
        Duration::from_secs(120),
        tokio::process::Command::new(cli)
            .kill_on_drop(true)
            .arg("run")
            .arg(&input)
            .arg("--save")
            .arg(&output)
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

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::postgres::PgPoolOptions;

    #[cfg(unix)]
    #[tokio::test]
    async fn product_admission_failure_retries_coarse_viewport() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("fake-cli");
        std::fs::write(&script, r##"#!/bin/sh
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
"##).unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        let png = cli_snapshot(script.to_str().unwrap(), &dir.path().join("model.step"),
            &dir.path().join("preview.png"), "auto", "product").await.unwrap();
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
        let version =
            match pdm_db::create_version(&pool, file.id, blob.id, user.id, "worker test", None, "")
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
}
