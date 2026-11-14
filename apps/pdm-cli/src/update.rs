//! `pdm-cli update`: check the private Gitea for the newest PDM release and,
//! with `apply`, download it and replace this CLI in place.
//!
//! Exit codes (`check` and `apply` alike):
//!   0  up to date, an update was printed/applied, or no releases exist yet
//!   1  network/parse/asset error (including a refused or empty download)
//!   3  the Gitea rejected the request: PDM_UPDATE_TOKEN missing or wrong
//!
//! `check` is script-friendly: it prints exactly one line, `update available:
//! <tag> …` or `up to date: <tag> …` (plus `no releases yet`), and `--json`
//! emits `{"local","latest","available","url","server"}`.

use std::io::Write;
use std::path::Path;
use std::process::ExitCode;

use clap::Subcommand;

#[derive(Subcommand)]
pub enum UpdateCmd {
    /// Check Gitea for a newer PDM release.
    Check {
        /// Machine-readable output: {"local","latest","available","url","server"}.
        #[arg(long)]
        json: bool,
    },
    /// Download the newest release and replace this pdm-cli.
    Apply {
        /// Replace without asking.
        #[arg(long, short = 'y')]
        yes: bool,
    },
}

/// This CLI's version as it compares: the PDM build tag when this binary was
/// built for a deployment (`PDM_BUILD_TAG`, e.g. V.01-34), else the crate
/// version (0.1.0 for a from-source build).
fn local_version() -> String {
    std::env::var("PDM_BUILD_TAG")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_string())
}

/// The running server's build tag, best effort: only attempted when the CLI
/// knows its deployment (`--database-url` / PDM_DATABASE_URL set). The API
/// base is `PDM_API_URL`, defaulting to the local stack.
async fn server_build(db_url: Option<&str>) -> Option<String> {
    let _url = db_url?;
    let base = std::env::var("PDM_API_URL")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .map(|s| s.trim().trim_end_matches('/').to_string())
        .unwrap_or_else(|| "http://127.0.0.1:8080".into());
    let client = reqwest::Client::new();
    let resp = client
        .get(format!("{base}/version"))
        .timeout(std::time::Duration::from_secs(2))
        .send()
        .await
        .ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let v: serde_json::Value = resp.json().await.ok()?;
    v.get("build")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
        .or_else(|| {
            v.get("version")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
        })
}

async fn fetch_release() -> Result<pdm_update::Release, pdm_update::Error> {
    let base = pdm_update::base_url();
    let token = pdm_update::token();
    pdm_update::fetch_latest(&base, token.as_deref()).await
}

pub async fn check(json: bool, db_url: Option<&str>) -> ExitCode {
    let local = local_version();
    let server = server_build(db_url).await;
    let releases_page = pdm_update::releases_page_url();
    match fetch_release().await {
        Ok(rel) => {
            let available = pdm_update::is_newer(&rel.tag_name, &local);
            if json {
                let out = serde_json::json!({
                    "local": local,
                    "latest": rel.tag_name,
                    "available": available,
                    "url": rel.page_url().unwrap_or(releases_page.as_str()),
                    "server": server,
                });
                println!("{out}");
            } else if available {
                println!(
                    "update available: {} (local {}{})",
                    rel.tag_name,
                    local,
                    server
                        .as_ref()
                        .map(|s| format!(", server {s}"))
                        .unwrap_or_default()
                );
            } else {
                println!(
                    "up to date: {} (local {}{})",
                    rel.tag_name,
                    local,
                    server
                        .as_ref()
                        .map(|s| format!(", server {s}"))
                        .unwrap_or_default()
                );
            }
            ExitCode::SUCCESS
        }
        Err(pdm_update::Error::NoReleases) => {
            if json {
                println!(
                    "{}",
                    serde_json::json!({
                        "local": local,
                        "latest": null,
                        "available": false,
                        "url": releases_page,
                        "server": server,
                    })
                );
            } else {
                println!("no releases yet (local {local})");
            }
            ExitCode::SUCCESS
        }
        Err(pdm_update::Error::Auth) => {
            eprintln!(
                "update check: token missing or wrong — set PDM_UPDATE_TOKEN (the Gitea repo is private)"
            );
            ExitCode::from(3)
        }
        Err(e) => {
            eprintln!("update check failed: {e}");
            ExitCode::FAILURE
        }
    }
}

pub async fn apply(yes: bool) -> ExitCode {
    let local = local_version();
    let base = pdm_update::base_url();
    let rel = match fetch_release().await {
        Ok(rel) => rel,
        Err(pdm_update::Error::NoReleases) => {
            println!("no releases yet; nothing to apply");
            return ExitCode::SUCCESS;
        }
        Err(pdm_update::Error::Auth) => {
            eprintln!(
                "update: token missing or wrong — set PDM_UPDATE_TOKEN (the Gitea repo is private)"
            );
            return ExitCode::from(3);
        }
        Err(e) => {
            eprintln!("update failed: {e}");
            return ExitCode::FAILURE;
        }
    };
    let tag = rel.tag_name.clone();
    let os = std::env::consts::OS;
    let arch = std::env::consts::ARCH;
    let Some(idx) = pdm_update::pick_asset(&rel, os, arch) else {
        let names = rel
            .assets
            .iter()
            .map(|a| a.name.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        eprintln!("update: no asset for {os}/{arch} in {tag} (assets: {names})");
        return ExitCode::FAILURE;
    };
    let asset = &rel.assets[idx];
    let Some(url) = asset.download_url() else {
        eprintln!("update: asset '{0}' has no download URL", asset.name);
        return ExitCode::FAILURE;
    };
    if !pdm_update::release_url_allowed(url, &base) {
        eprintln!("update: asset URL not under the repo's release paths: {url}");
        return ExitCode::FAILURE;
    }
    let exe = std::env::current_exe().ok();

    // A launcher-managed copy never replaces itself: point at the launcher.
    if let Some(mgr) = pdm_update::this_managed_by(exe.as_deref()) {
        println!("update this copy in {mgr} (the launcher manages its CLI)");
        return ExitCode::SUCCESS;
    }

    // Ask first, then spend the download.
    if !yes {
        let label = exe
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "this pdm-cli".into());
        print!("Replace {label} with {tag}? [y/N] ");
        let _ = std::io::stdout().flush();
        let mut line = String::new();
        if std::io::stdin().read_line(&mut line).is_err() || !matches!(line.trim(), "y" | "Y") {
            println!("kept the current version ({local})");
            return ExitCode::SUCCESS;
        }
    }

    eprintln!("downloading {url} …");
    let bytes = match pdm_update::download(url, pdm_update::MAX_DOWNLOAD_BYTES).await {
        Ok(b) => b,
        Err(e) => {
            eprintln!("update failed: {e}");
            return ExitCode::FAILURE;
        }
    };
    if bytes.is_empty() {
        eprintln!("update failed: the downloaded asset is empty");
        return ExitCode::FAILURE;
    }

    let dir = std::env::temp_dir().join(format!("pdm-update-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let candidate = if asset.name.to_ascii_lowercase().ends_with(".tar.gz") {
        match pdm_update::extract_cli(&bytes, &dir) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("update failed: {e}");
                return ExitCode::FAILURE;
            }
        }
    } else if asset
        .name
        .to_ascii_lowercase()
        .strip_suffix(".tar.gz")
        .unwrap_or(&asset.name)
        .to_ascii_lowercase()
        .starts_with("pdm-cli")
    {
        let p = dir.join("pdm-cli");
        if let Err(e) = std::fs::write(&p, &bytes) {
            eprintln!("update failed: write {p:?}: {e}");
            return ExitCode::FAILURE;
        }
        p
    } else {
        eprintln!(
            "update: unexpected release asset '{0}' (want a pdm-cli .tar.gz or bare binary)",
            asset.name
        );
        return ExitCode::FAILURE;
    };

    let result = match exe {
        Some(exe)
            if exe
                .metadata()
                .map(|m| m.file_type().is_file())
                .unwrap_or(false) =>
        {
            replace_exe(&exe, &candidate, &tag)
        }
        _ => {
            println!(
                "downloaded the new {tag} pdm-cli to {0}; move it over the current executable \
                 (this copy is not a plain file, e.g. it runs through a script)",
                candidate.display()
            );
            Ok(())
        }
    };
    let _ = std::fs::remove_dir_all(&dir);
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("update failed: {e}");
            ExitCode::FAILURE
        }
    }
}

/// Atomically replace `exe`: the old file is kept beside it as
/// `pdm-cli.old-<timestamp>`, the new one is renamed into place. Both renames
/// are same-directory, so a crash between them can't leave two executables
/// missing.
fn replace_exe(exe: &Path, candidate: &Path, tag: &str) -> Result<(), String> {
    let dir = exe.parent().ok_or("the executable has no directory")?;
    let ts = format!(
        "{}-{}",
        chrono::Utc::now().format("%Y%m%dT%H%M%S"),
        std::process::id()
    );
    let bak = dir.join(format!("pdm-cli.old-{ts}"));
    let new = dir.join(format!("pdm-cli.new-{ts}"));

    std::fs::copy(candidate, &new).map_err(|e| format!("copy the new binary into {dir:?}: {e}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&new, std::fs::Permissions::from_mode(0o755));
    }
    std::fs::rename(exe, &bak).map_err(|e| format!("back up {exe:?} to {bak:?}: {e}"))?;
    if let Err(e) = std::fs::rename(&new, exe) {
        // Best effort: put the old executable back.
        let _ = std::fs::rename(&bak, exe);
        let _ = std::fs::remove_file(&new);
        return Err(format!("move the new binary into {exe:?}: {e}"));
    }
    println!(
        "updated {0} to {tag} (backup: {1})",
        exe.display(),
        bak.display()
    );
    Ok(())
}
