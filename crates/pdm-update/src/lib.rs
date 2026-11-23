//! PDM update checks: the latest release on the private Gitea, version comparison,
//! release-asset selection and the "managed by a launcher" convention, shared by
//! `pdm-cli update` and the server's boot-time check.
//!
//! The repo is private, so every API call carries a bearer token from
//! `PDM_UPDATE_TOKEN` (401/404 answers surface as [`Error::Auth`]); a missing or
//! wrong token is a clear error, never a crash. Release pages and asset URLs are
//! only accepted under the repository's `releases/` (and `releases_download/`)
//! paths, and bodies are capped so a hostile answer cannot eat the disk.
//!
//! A launcher can manage a copy of the CLI: `SOLIDCRAFT_MANAGED_BY=<name>`, or an
//! `installed.json` next to the executable records the manager. Checks still run;
//! `pdm-cli update apply` then points at the launcher instead of replacing itself.

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use futures::StreamExt;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// The Gitea the PDM releases live on (override with `PDM_UPDATE_BASE_URL`).
pub const DEFAULT_BASE: &str = "https://gitea.example.com";
/// The private repository.
pub const REPO: &str = "owner/solidcraft-pdm";
/// Bearer token for the private repo API.
pub const TOKEN_ENV: &str = "PDM_UPDATE_TOKEN";
/// Alternate Gitea base URL (tests, mirrors).
pub const BASE_ENV: &str = "PDM_UPDATE_BASE_URL";
/// Set to `0` to opt a server out of the boot-time release check.
pub const CHECK_ENV: &str = "PDM_UPDATE_CHECK";
/// Set by a launcher that installed and updates this copy; the value is its name.
pub const MANAGED_ENV: &str = "SOLIDCRAFT_MANAGED_BY";
/// Boot checks at most once every 24 h (marker file next to the data dir).
pub const CHECK_INTERVAL: Duration = Duration::from_secs(24 * 3600);
/// A release JSON comes from the network: cap it like the desktop app does.
const MAX_RELEASE_BYTES: u64 = 4 * 1024 * 1024;
/// A release asset (the new pdm-cli binary in a tarball) may be this big.
pub const MAX_DOWNLOAD_BYTES: u64 = 200 * 1024 * 1024;
/// The request the update check makes never waits longer than this.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
/// More assets than this are ignored (the release JSON is hostile input).
const MAX_ASSETS: usize = 400;
/// The longest manager name shown (the env var / record file are untrusted text).
const MAX_MANAGER_NAME: usize = 64;
/// A launcher record is a few KB; room for a hundred entries.
const MAX_RECORD_FILE: u64 = 1 << 20;

// ── Errors ───────────────────────────────────────────────────────────────

/// Why a release check or download failed.
#[derive(Error, Debug, PartialEq)]
pub enum Error {
    /// 401/403: the private repo rejected the request — token missing or wrong.
    #[error("Gitea rejected the request: token missing or wrong (set PDM_UPDATE_TOKEN)")]
    Auth,
    /// No releases have been published yet (`releases/latest` answers 404).
    #[error("no releases yet")]
    NoReleases,
    /// Offline, timed out, a non-2xx answer, a body past its cap, ...
    #[error("release check failed: {0}")]
    Network(String),
    /// The answer was not the expected release JSON.
    #[error("release check failed: {0}")]
    Parse(String),
}

// ── Release shape (Gitea's GitHub-compatible API) ────────────────────────

/// One downloadable file of a release.
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct Asset {
    #[serde(default)]
    pub name: String,
    /// Gitea: `https://<host>/<owner>/<repo>/releases/download/<tag>/<name>`.
    #[serde(default)]
    pub browser_download_url: Option<String>,
    /// Gitea API form: `…/api/v1/repos/…/releases/<id>/assets/<name>`.
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub size: Option<u64>,
}

impl Asset {
    /// Where to download from: the browser URL first, the API URL as fallback.
    pub fn download_url(&self) -> Option<&str> {
        self.browser_download_url
            .as_deref()
            .filter(|s| !s.is_empty())
            .or_else(|| self.url.as_deref().filter(|s| !s.is_empty()))
    }
}

/// The latest release, as Gitea's `releases/latest` answers it.
#[derive(Debug, Clone, Deserialize, PartialEq, Default)]
pub struct Release {
    #[serde(default)]
    pub tag_name: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub html_url: Option<String>,
    #[serde(default)]
    pub published_at: Option<String>,
    /// When the release (or its assets) was last touched: the change marker for
    /// rolling tags like `nightly`.
    #[serde(default)]
    pub updated_at: Option<String>,
    /// GitHub's pre-release flag (the PDM's Gitea releases never set it).
    #[serde(default)]
    pub prerelease: bool,
    #[serde(default)]
    pub assets: Vec<Asset>,
}

impl Release {
    /// The release page people read notes on.
    pub fn page_url(&self) -> Option<&str> {
        self.html_url.as_deref().filter(|s| !s.is_empty())
    }
}

/// Parse a release JSON body (hostile: every field optional, non-JSON tolerated).
pub fn parse_release(body: &[u8]) -> Result<Release, Error> {
    let rel: Release = serde_json::from_slice(body)
        .map_err(|e| Error::Parse(format!("not a release JSON: {e}")))?;
    if rel.tag_name.is_empty() {
        return Err(Error::Parse("release JSON has no tag_name".into()));
    }
    if rel.assets.len() > MAX_ASSETS {
        let mut rel = rel;
        rel.assets.truncate(MAX_ASSETS);
        Ok(rel)
    } else {
        Ok(rel)
    }
}

/// The URL a release check asks.
pub fn latest_api_url(base: &str) -> String {
    format!("{base}/api/v1/repos/{}/releases/latest", repo())
}

/// The release history page (what the web "Update" row links to).
pub fn releases_page_url() -> String {
    let full = base_url();
    format!("{full}/{}/releases", repo())
}

/// Repository path on the configured Gitea server.
pub fn repo() -> String {
    std::env::var("PDM_UPDATE_REPO")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .map(|s| s.trim().trim_matches('/').to_string())
        .unwrap_or_else(|| REPO.to_string())
}

/// The Gitea base URL in effect: `PDM_UPDATE_BASE_URL` or the public instance.
pub fn base_url() -> String {
    std::env::var(BASE_ENV)
        .ok()
        .filter(|s| !s.trim().is_empty())
        .map(|s| s.trim().trim_end_matches('/').to_string())
        .unwrap_or_else(|| DEFAULT_BASE.to_string())
}

/// The token in effect, if any.
pub fn token() -> Option<String> {
    std::env::var(TOKEN_ENV)
        .ok()
        .filter(|s| !s.trim().is_empty())
}

/// Whether the boot-time check is disabled (`PDM_UPDATE_CHECK=0`).
pub fn check_disabled() -> bool {
    std::env::var(CHECK_ENV).is_ok_and(|v| v == "0")
}

/// Fetch the latest release. `base` without trailing slash; `token` = bearer value.
pub async fn fetch_latest(base: &str, token: Option<&str>) -> Result<Release, Error> {
    let client = reqwest::Client::new();
    let url = latest_api_url(base.trim_end_matches('/'));
    let mut req = client.get(&url).timeout(REQUEST_TIMEOUT);
    if let Some(t) = token.filter(|t| !t.is_empty()) {
        req = req.bearer_auth(t);
    }
    let resp = req
        .send()
        .await
        .map_err(|e| Error::Network(e.to_string()))?;
    let status = resp.status();
    if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
        return Err(Error::Auth);
    }
    if status == reqwest::StatusCode::NOT_FOUND {
        return Err(Error::NoReleases);
    }
    if !status.is_success() {
        return Err(Error::Network(format!("HTTP {status} from {url}")));
    }
    let body = read_capped(resp, MAX_RELEASE_BYTES).await?;
    parse_release(&body)
}

/// Download a release asset, refusing to keep more than `cap` bytes.
pub async fn download(url: &str, cap: u64) -> Result<Vec<u8>, Error> {
    let client = reqwest::Client::new();
    let resp = client
        .get(url)
        .timeout(Duration::from_secs(120))
        .send()
        .await
        .map_err(|e| Error::Network(e.to_string()))?;
    let status = resp.status();
    if !status.is_success() {
        return Err(Error::Network(format!("HTTP {status} from {url}")));
    }
    read_capped(resp, cap).await
}

/// Read a response body up to `cap` bytes; one more byte is a capped-body error.
async fn read_capped(resp: reqwest::Response, cap: u64) -> Result<Vec<u8>, Error> {
    let mut bytes = Vec::new();
    let mut stream = resp
        .bytes_stream()
        .map(|c| c.map_err(|e| Error::Network(e.to_string())));
    while let Some(chunk) = stream.next().await {
        bytes.extend_from_slice(&chunk?);
        if bytes.len() as u64 > cap {
            return Err(Error::Parse(format!("body larger than the {cap}-byte cap")));
        }
    }
    Ok(bytes)
}

// ── URL allow-list ───────────────────────────────────────────────────────

/// A release page or asset URL: the Gitea base, this repository, then a
/// `releases/` (page, `download/` and API asset links) or legacy
/// `releases_download/` path. Anything else is not an allowed download.
pub fn release_url_allowed(url: &str, base: &str) -> bool {
    let b = base.trim_end_matches('/');
    let prefixes = [
        format!("{b}/{}/releases/", repo()),
        format!("{b}/{}/releases_download/", repo()),
        format!("{b}/api/v1/repos/{}/releases/", repo()),
    ];
    prefixes.iter().any(|p| url.starts_with(p.as_str()))
}

// ── versions ─────────────────────────────────────────────────────────────

/// What a version compares as. PDM spells versions two ways: crate semver
/// (`0.1.0`, optionally `v`-prefixed, optionally a `-pre` suffix) and
/// build tags (`V.01-34` = release series 01, build 34). Both map onto one
/// total order: `(major, minor, patch, is_release)`, where a build tag
/// `V.MM-NN` is `(MM, 0, NN, release)` and a semver `X.Y[.Z]` is
/// `(X, Y, Z, …)` with pre-releases below their release.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct VersionKey {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
    /// Releases (1) sort above their pre-releases (0).
    pub is_release: u8,
}

/// Parse a version string (crate semver or build tag); `None` when it spells
/// neither (`""`, `"unknown"`, `"latest"`, …).
pub fn parse_version(s: &str) -> Option<VersionKey> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    let lower = s.to_ascii_lowercase();
    // Build tag: `V.MM-NN` (or `v.01-34`, `v01-34` …): optional v, optional dot,
    // series number, '-', build number.
    for cand in [
        lower.as_str(),
        lower.strip_prefix('v').unwrap_or(lower.as_str()),
    ] {
        let num = cand.strip_prefix('.').unwrap_or(cand);
        if let Ok((series, build)) = split_build_tag(num) {
            return Some(VersionKey {
                major: series,
                minor: 0,
                patch: build,
                is_release: 1,
            });
        }
    }
    // A leading `v` belongs to the tag, not the semver.
    let lower = lower.strip_prefix('v').unwrap_or(lower.as_str());
    // Semver: `X.Y`, `X.Y.Z`, with an optional `-pre` suffix (pre-releases
    // sort below their release).
    let (core, is_release) = match lower.split_once('-') {
        Some((core, _)) => (core, 0u8),
        None => (lower, 1),
    };
    let mut nums = core.split('.');
    let major: u64 = nums.next()?.parse().ok()?;
    let minor: u64 = nums.next()?.parse().ok()?;
    let patch: u64 = match nums.next() {
        Some(p) => p.parse().ok()?,
        None => 0,
    };
    if nums.next().is_some() {
        return None; // `0.1.0.2` is not our shape
    }
    Some(VersionKey {
        major,
        minor,
        patch,
        is_release,
    })
}

fn split_build_tag(num: &str) -> Result<(u64, u64), ()> {
    let (series, build) = num.split_once('-').ok_or(())?;
    let series: u64 = series.parse().map_err(|_| ())?;
    let build: u64 = build.parse().map_err(|_| ())?;
    Ok((series, build))
}

/// The total order over two version strings; `None` when either does not parse.
pub fn cmp_versions(a: &str, b: &str) -> Option<std::cmp::Ordering> {
    let (a, b) = (parse_version(a)?, parse_version(b)?);
    Some(a.cmp(&b))
}

/// `latest` (a release tag) is newer than `current` (this build's tag).
pub fn is_newer(latest: &str, current: &str) -> bool {
    matches!(
        cmp_versions(latest, current),
        Some(std::cmp::Ordering::Greater)
    )
}

// ── asset selection ──────────────────────────────────────────────────────

/// This machine's asset tokens: the OS name and the architecture, the ways
/// release asset names spell them.
pub fn asset_tokens(os: &str, arch: &str) -> (&'static [&'static str], &'static [&'static str]) {
    let os = match os {
        "linux" => &["linux"][..],
        "windows" => &["windows"][..],
        "macos" | "darwin" => &["macos", "darwin"][..],
        _ => &[][..], // an unknown platform matches no asset
    };
    let arch = match arch {
        "x86_64" | "amd64" => &["x86_64", "amd64"][..],
        "aarch64" | "arm64" => &["aarch64", "arm64"][..],
        _ => &[][..],
    };
    (os, arch)
}

/// The index of the release asset that fits this OS/arch: its name (any case)
/// contains an OS token and an arch token. With several matches, only a name
/// containing `pdm-cli` wins; two `pdm-cli` candidates (or none) mean
/// ambiguous, so `None`.
pub fn pick_asset(rel: &Release, os: &str, arch: &str) -> Option<usize> {
    let (os_tokens, arch_tokens) = asset_tokens(os, arch);
    let mut matches: Vec<usize> = rel
        .assets
        .iter()
        .enumerate()
        .filter(|(_, a)| {
            let n = a.name.to_ascii_lowercase();
            os_tokens.iter().any(|t| n.contains(t)) && arch_tokens.iter().any(|t| n.contains(t))
        })
        .map(|(i, _)| i)
        .collect();
    if matches.len() > 1 {
        matches.retain(|i| rel.assets[*i].name.to_ascii_lowercase().contains("pdm-cli"));
    }
    if matches.len() == 1 {
        Some(matches.pop()?)
    } else {
        None
    }
}

// ── managed by a launcher ────────────────────────────────────────────────

/// What manages this copy, if anything: the variable's value, else the
/// `installed.json` record next to the executable (an object with a `manager`
/// or `managed_by` string). Read-only: nothing is written. `read` returns a
/// file's bytes, so tests can describe any machine.
pub fn managed_by(
    env: &dyn Fn(&str) -> Option<String>,
    exe_dir: Option<&Path>,
    read: &dyn Fn(&Path) -> Option<Vec<u8>>,
) -> Option<String> {
    if let Some(name) = env(MANAGED_ENV) {
        let name: String = name
            .chars()
            .filter(|c| !c.is_control())
            .take(MAX_MANAGER_NAME)
            .collect();
        if !name.trim().is_empty() {
            return Some(name.trim().to_string());
        }
    }
    let dir = exe_dir?;
    let bytes = read(&dir.join("installed.json"))?;
    let Ok(json) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
        return None;
    };
    json.get("manager")
        .or_else(|| json.get("managed_by"))
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// This machine's manager, looked up once: the env var, else the
/// `installed.json` beside the running executable.
pub fn this_managed_by(exe: Option<&Path>) -> Option<String> {
    let env = |k: &str| std::env::var(k).ok();
    let read = |p: &Path| {
        use std::io::Read;
        let mut bytes = Vec::new();
        std::fs::File::open(p)
            .ok()?
            .take(MAX_RECORD_FILE)
            .read_to_end(&mut bytes)
            .ok()?;
        Some(bytes)
    };
    managed_by(&env, exe.and_then(Path::parent), &read)
}

// ── tarballs ─────────────────────────────────────────────────────────────

/// Extract a `.tar.gz` into `dest` and return the `pdm-cli` binary inside it
/// (any directory depth, `pdm-cli` or `pdm-cli.exe`). Errors when the archive
/// is not a tarball or holds no such binary.
pub fn extract_cli(bytes: &[u8], dest: &Path) -> Result<std::path::PathBuf, Error> {
    use flate2::read::GzDecoder;
    std::fs::create_dir_all(dest).map_err(|e| Error::Parse(format!("create {dest:?}: {e}")))?;
    let gz = GzDecoder::new(bytes);
    let mut archive = tar::Archive::new(gz);
    let mut found: Option<std::path::PathBuf> = None;
    for entry in archive
        .entries()
        .map_err(|e| Error::Parse(format!("bad tar.gz: {e}")))?
    {
        let mut entry = entry.map_err(|e| Error::Parse(format!("bad tar.gz entry: {e}")))?;
        let name = entry
            .path()
            .map_err(|e| Error::Parse(format!("bad tar.gz entry path: {e}")))?
            .file_name()
            .map(|n| n.to_os_string())
            .and_then(|n| n.into_string().ok());
        let is_cli = entry.header().entry_type().is_file()
            && matches!(name.as_deref(), Some("pdm-cli") | Some("pdm-cli.exe"));
        if is_cli {
            let out = dest.join(name.clone().unwrap_or_else(|| "pdm-cli".into()));
            entry
                .unpack(&out)
                .map_err(|e| Error::Parse(format!("extract: {e}")))?;
            found = Some(out);
            break;
        }
    }
    found.ok_or_else(|| Error::Parse("the .tar.gz contains no pdm-cli binary".into()))
}

// ── 24 h marker ──────────────────────────────────────────────────────────

/// What a completed release check remembered, in the marker file next to the
/// data dir (one JSON line: when, what it found).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct Marker {
    pub ts: u64,
    pub latest: Option<String>,
    pub available: bool,
}

/// The marker's mtime in the file's contents, or `None` (absent/unreadable).
pub fn read_marker(path: &Path) -> Option<Marker> {
    let bytes = std::fs::read(path).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// Remember a check.
pub fn write_marker(path: &Path, marker: &Marker) -> std::io::Result<()> {
    std::fs::write(
        path,
        serde_json::to_vec(marker).unwrap_or_else(|_| b"{}".to_vec()),
    )
}

/// Whether a check is due: no marker, or the last one is `CHECK_INTERVAL` old.
pub fn check_due(marker: Option<&Marker>) -> bool {
    let Some(m) = marker else { return true };
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    now.saturating_sub(m.ts) >= CHECK_INTERVAL.as_secs()
}

/// The server's boot-time check: at most once every 24 h, in the background,
/// silent on failure. Refreshes [`UpdateState`] and, when a newer release
/// exists, logs it.
pub async fn boot_check(
    state: Arc<tokio::sync::RwLock<UpdateState>>,
    marker: Option<&Path>,
    current: &str,
) {
    if let Some(path) = marker
        && let Some(m) = read_marker(path).filter(|m| !check_due(Some(m)))
    {
        if m.latest.is_some() {
            let mut s = state.write().await;
            s.latest = m.latest.clone();
            s.update_available =
                m.available && is_newer(m.latest.as_deref().unwrap_or_default(), current);
        }
        tracing::debug!("release check skipped: checked <24h ago (marker)");
        return;
    }
    let Some(tok) = token() else {
        tracing::debug!("release check skipped: PDM_UPDATE_TOKEN not set");
        return;
    };
    let base = base_url();
    match fetch_latest(&base, Some(&tok)).await {
        Ok(rel) => {
            let available = is_newer(&rel.tag_name, current);
            {
                let mut s = state.write().await;
                s.latest = Some(rel.tag_name.clone());
                s.update_available = available;
            }
            if let Some(path) = marker {
                let m = Marker {
                    ts: SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .map(|d| d.as_secs())
                        .unwrap_or(0),
                    latest: Some(rel.tag_name.clone()),
                    available,
                };
                if let Err(e) = write_marker(path, &m) {
                    tracing::debug!(%e, "could not write the update-check marker");
                }
            }
            if available {
                tracing::info!(
                    latest = %rel.tag_name,
                    running = %current,
                    "PDM update available: {0} (running {1}); `pdm-cli update check`",
                    rel.tag_name, current
                );
            }
        }
        Err(Error::Auth) => {
            tracing::warn!("PDM release check: token missing or wrong (set PDM_UPDATE_TOKEN)");
        }
        Err(e) => {
            tracing::debug!(%e, "PDM release check failed (boot continues)");
        }
    }
}

/// GitHub repo that publishes SolidCraft (and `solidcraft-cli` inside the linux tarball).
pub const CAD_CLI_GITHUB: &str = "tangency-labs/solidcraft";
/// Set to `0` to skip pulling the latest SolidCraft CLI at boot.
pub const CAD_CLI_UPDATE_ENV: &str = "PDM_CLI_UPDATE";
/// Where to install the CLI if `PDM_SOLVECRAFT_CLI` is unset.
pub const CAD_CLI_DEFAULT_PATH: &str = "/usr/local/bin/solidcraft-cli";

pub fn cad_cli_update_disabled() -> bool {
    std::env::var(CAD_CLI_UPDATE_ENV).is_ok_and(|v| v == "0")
}

/// Latest SolidCraft release on GitHub (public; no token).
pub async fn fetch_cad_cli_latest() -> Result<Release, Error> {
    let url = format!("https://api.github.com/repos/{CAD_CLI_GITHUB}/releases/latest");
    let client = reqwest::Client::new();
    let resp = client
        .get(&url)
        .header("User-Agent", "foundry-pdm")
        .timeout(REQUEST_TIMEOUT)
        .send()
        .await
        .map_err(|e| Error::Network(e.to_string()))?;
    let status = resp.status();
    if status == reqwest::StatusCode::NOT_FOUND {
        return Err(Error::NoReleases);
    }
    if !status.is_success() {
        return Err(Error::Network(format!("HTTP {status} from {url}")));
    }
    let body = read_capped(resp, MAX_RELEASE_BYTES).await?;
    parse_release(&body)
}

/// Linux tarball for this CPU that contains `bin/solidcraft-cli`.
pub fn cad_cli_linux_tarball(rel: &Release) -> Option<&Asset> {
    let arch = match std::env::consts::ARCH {
        "aarch64" | "arm64" => "aarch64",
        "x86_64" | "amd64" => "x86_64",
        _ => return None,
    };
    rel.assets.iter().find(|a| {
        let n = a.name.to_ascii_lowercase();
        n.starts_with("solidcraft-")
            && n.contains("linux")
            && n.contains(arch)
            && n.ends_with(".tar.gz")
            && !n.contains("appimage")
    })
}

/// Pull `solidcraft-cli` (and `solvecraft-cli` if present) from a GitHub tarball.
pub fn extract_cad_cli(bytes: &[u8], dest: &Path) -> Result<std::path::PathBuf, Error> {
    use flate2::read::GzDecoder;
    std::fs::create_dir_all(dest).map_err(|e| Error::Parse(format!("create {dest:?}: {e}")))?;
    let gz = GzDecoder::new(bytes);
    let mut archive = tar::Archive::new(gz);
    let mut found: Option<std::path::PathBuf> = None;
    for entry in archive
        .entries()
        .map_err(|e| Error::Parse(format!("bad tar.gz: {e}")))?
    {
        let mut entry = entry.map_err(|e| Error::Parse(format!("bad tar.gz entry: {e}")))?;
        let name = entry
            .path()
            .map_err(|e| Error::Parse(format!("bad tar.gz entry path: {e}")))?
            .file_name()
            .map(|n| n.to_os_string())
            .and_then(|n| n.into_string().ok());
        let is_cli = entry.header().entry_type().is_file()
            && matches!(
                name.as_deref(),
                Some("solidcraft-cli")
                    | Some("solvecraft-cli")
                    | Some("solidcraft-cli.exe")
                    | Some("solvecraft-cli.exe")
            );
        if !is_cli {
            let _ = std::io::copy(&mut entry, &mut std::io::sink());
            continue;
        }
        let fname = name.unwrap_or_else(|| "solidcraft-cli".into());
        let out = dest.join(&fname);
        entry
            .unpack(&out)
            .map_err(|e| Error::Parse(format!("extract: {e}")))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&out, std::fs::Permissions::from_mode(0o755));
        }
        if fname.starts_with("solidcraft-cli") || found.is_none() {
            found = Some(out);
        }
    }
    found.ok_or_else(|| Error::Parse("the .tar.gz contains no solidcraft-cli binary".into()))
}

fn install_file(from: &Path, to: &Path) -> Result<(), Error> {
    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| Error::Parse(format!("mkdir {parent:?}: {e}")))?;
    }
    let tmp = to.with_extension("new");
    std::fs::copy(from, &tmp).map_err(|e| Error::Parse(format!("copy {from:?}: {e}")))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755));
    }
    std::fs::rename(&tmp, to).map_err(|e| Error::Parse(format!("install {to:?}: {e}")))?;
    Ok(())
}

/// Download the latest GitHub SolidCraft CLI and install it at `dest` (and a
/// `solvecraft-cli` alias beside it). Returns the release tag.
pub async fn apply_cad_cli(dest: &Path) -> Result<String, Error> {
    let rel = fetch_cad_cli_latest().await?;
    let asset = cad_cli_linux_tarball(&rel).ok_or_else(|| {
        Error::Parse("no linux tar.gz for this architecture in the SolidCraft release".into())
    })?;
    let url = asset
        .download_url()
        .ok_or_else(|| Error::Parse("release asset has no download URL".into()))?;
    if !url.starts_with("https://github.com/tangency-labs/solidcraft/releases/download/") {
        return Err(Error::Parse(
            "refusing download URL outside GitHub SolidCraft releases".into(),
        ));
    }
    let bytes = download(url, MAX_DOWNLOAD_BYTES).await?;
    let dir = std::env::temp_dir().join(format!("pdm-cad-cli-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let extracted = extract_cad_cli(&bytes, &dir)?;
    install_file(&extracted, dest)?;
    let alias = dest.parent().unwrap_or(dest).join("solvecraft-cli");
    let alias_src = dir.join("solvecraft-cli");
    if alias_src.is_file() {
        let _ = install_file(&alias_src, &alias);
    }
    let _ = std::fs::remove_dir_all(&dir);
    Ok(rel.tag_name)
}

/// If GitHub has a newer SolidCraft CLI than `current` (e.g. `solidcraft-cli 0.27.1`), install it.
pub async fn boot_cad_cli_update(current: &str, dest: &Path) {
    if cad_cli_update_disabled() {
        return;
    }
    match fetch_cad_cli_latest().await {
        Ok(rel) => {
            let tag = rel.tag_name.trim_start_matches('v');
            let local = current
                .rsplit_once(' ')
                .map(|(_, v)| v)
                .unwrap_or(current)
                .trim()
                .trim_start_matches('v');
            if !is_newer(tag, local) {
                tracing::info!("SolidCraft CLI is current ({local})");
                return;
            }
            tracing::info!("SolidCraft CLI {local} → {tag}; downloading from GitHub");
            match apply_cad_cli(dest).await {
                Ok(installed) => tracing::info!("SolidCraft CLI updated to {installed}"),
                Err(e) => tracing::warn!("SolidCraft CLI update failed: {e}"),
            }
        }
        Err(e) => tracing::debug!(%e, "SolidCraft CLI release check failed (boot continues)"),
    }
}

/// Hours between GitHub CLI checks (default 6). `PDM_CLI_UPDATE_HOURS`.
pub fn cad_cli_interval() -> Duration {
    let hours = std::env::var("PDM_CLI_UPDATE_HOURS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .filter(|h| *h > 0)
        .unwrap_or(6);
    Duration::from_secs(hours.saturating_mul(3600))
}

/// Keep the CLI current without a reboot: check now, then on an interval.
pub async fn loop_cad_cli_update(dest: std::path::PathBuf) {
    loop {
        let current = tokio::process::Command::new(&dest)
            .arg("--version")
            .output()
            .await
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .unwrap_or_default();
        boot_cad_cli_update(&current, &dest).await;
        tokio::time::sleep(cad_cli_interval()).await;
    }
}

// ── Dutchman (dutchman-cad/dutchman): native CAD → SolidCraft with a feature
//    tree. The PDM converts with the newest published release: the image bakes
//    one in at build, the server keeps it current on an interval, and a
//    conversion job pulls it on demand when the binary is missing. ────────────

/// GitHub repo that publishes Dutchman Convert releases.
pub const DUTCHMAN_GITHUB: &str = "dutchman-cad/dutchman";
/// Set to `0` to skip pulling the latest Dutchman release (boot loop, on-demand
/// installs, and the worker's fallback all opt out).
pub const DUTCHMAN_UPDATE_ENV: &str = "PDM_DUTCHMAN_UPDATE";
/// The configured Dutchman CLI path.
pub const DUTCHMAN_CLI_ENV: &str = "PDM_DUTCHMAN_CLI";
/// Where to install dutchman if `PDM_DUTCHMAN_CLI` is unset.
pub const DUTCHMAN_DEFAULT_PATH: &str = "/usr/local/bin/dutchman";
/// The Linux release tarball carries the full native decoder: cap it above the CLI's.
const DUTCHMAN_MAX_DOWNLOAD_BYTES: u64 = 512 * 1024 * 1024;

pub fn dutchman_update_disabled() -> bool {
    std::env::var(DUTCHMAN_UPDATE_ENV).is_ok_and(|v| v == "0")
}

/// The Dutchman binary path in effect: `PDM_DUTCHMAN_CLI` or the default.
pub fn dutchman_cli_path() -> String {
    std::env::var(DUTCHMAN_CLI_ENV)
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| DUTCHMAN_DEFAULT_PATH.to_string())
}

/// Hours between Dutchman release checks (default 1, tighter than the CLI's):
/// the PDM converts with the newest published release. `PDM_DUTCHMAN_UPDATE_HOURS`.
pub fn dutchman_interval() -> Duration {
    let hours = std::env::var("PDM_DUTCHMAN_UPDATE_HOURS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .filter(|h| *h > 0)
        .unwrap_or(1);
    Duration::from_secs(hours.saturating_mul(3600))
}

/// Latest Dutchman release on GitHub (public; no token).
pub async fn fetch_dutchman_latest() -> Result<Release, Error> {
    fetch_dutchman_release("latest").await
}

/// A Dutchman release by tag: `latest` (newest stable) or an exact tag name
/// (`nightly`, `v0.2.9`, `onshape-preview-2`, …).
pub async fn fetch_dutchman_release(tag: &str) -> Result<Release, Error> {
    let url = if tag == "latest" {
        format!("https://api.github.com/repos/{DUTCHMAN_GITHUB}/releases/latest")
    } else {
        format!(
            "https://api.github.com/repos/{DUTCHMAN_GITHUB}/releases/tags/{}",
            urlencoding(tag)
        )
    };
    let client = reqwest::Client::new();
    let resp = client
        .get(&url)
        .header("User-Agent", "foundry-pdm")
        .timeout(REQUEST_TIMEOUT)
        .send()
        .await
        .map_err(|e| Error::Network(e.to_string()))?;
    let status = resp.status();
    if status == reqwest::StatusCode::NOT_FOUND {
        return Err(Error::NoReleases);
    }
    if !status.is_success() {
        return Err(Error::Network(format!("HTTP {status} from {url}")));
    }
    let body = read_capped(resp, MAX_RELEASE_BYTES).await?;
    parse_release(&body)
}

/// Minimal percent-encoding for release tags (no query parsing involved).
fn urlencoding(tag: &str) -> String {
    let mut out = String::new();
    for b in tag.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// This machine's Dutchman Linux tarball: the versioned
/// `dutchman-<version>-<arch>-unknown-linux-gnu.tar.gz` asset first (it has a
/// `.sha256` sidecar), the unversioned `dutchman-<arch>-unknown-linux-gnu.tar.gz`
/// name the README links as fallback.
pub fn dutchman_linux_tarball(rel: &Release) -> Option<&Asset> {
    let arch = match std::env::consts::ARCH {
        "aarch64" | "arm64" => "aarch64",
        "x86_64" | "amd64" => "x86_64",
        _ => return None,
    };
    let versioned = {
        let tag = rel.tag_name.trim_start_matches('v');
        (!tag.is_empty())
            .then(|| {
                rel.assets
                    .iter()
                    .find(|a| a.name == format!("dutchman-{tag}-{arch}-unknown-linux-gnu.tar.gz"))
            })
            .flatten()
    };
    versioned.or_else(|| {
        rel.assets
            .iter()
            .find(|a| a.name == format!("dutchman-{arch}-unknown-linux-gnu.tar.gz"))
    })
}

/// The version token of the installed dutchman (`dutchman --version`), `None`
/// when the binary is missing or reports no recognizable version.
pub async fn dutchman_version_of(dest: &Path) -> Option<String> {
    let out = tokio::time::timeout(
        Duration::from_secs(5),
        tokio::process::Command::new(dest).arg("--version").output(),
    )
    .await
    .ok()
    .and_then(|out| out.ok())?;
    let text = String::from_utf8_lossy(&out.stdout);
    text.lines().rev().find_map(|line| {
        line.split_whitespace()
            .find(|tok| parse_version(tok).is_some())
            .map(str::to_string)
    })
}

/// The expected SHA-256 of a release tarball from its `.sha256` sidecar: the
/// first 64-hex-digit field of the first line.
pub fn dutchman_sidecar_sha(bytes: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(bytes);
    let line = text.lines().next()?;
    line.split_whitespace()
        .next()
        .filter(|h| h.len() == 64 && h.bytes().all(|b| b.is_ascii_hexdigit()))
        .map(|h| h.to_ascii_lowercase())
}

/// Extract the `dutchman` binary from a release tarball (the top-level
/// `dutchman-<version>-<target>/` folder) into `dest`.
pub fn extract_dutchman(bytes: &[u8], dest: &Path) -> Result<std::path::PathBuf, Error> {
    use flate2::read::GzDecoder;
    std::fs::create_dir_all(dest).map_err(|e| Error::Parse(format!("create {dest:?}: {e}")))?;
    let gz = GzDecoder::new(bytes);
    let mut archive = tar::Archive::new(gz);
    let mut found: Option<std::path::PathBuf> = None;
    for entry in archive
        .entries()
        .map_err(|e| Error::Parse(format!("bad tar.gz: {e}")))?
    {
        let mut entry = entry.map_err(|e| Error::Parse(format!("bad tar.gz entry: {e}")))?;
        let name = entry
            .path()
            .map_err(|e| Error::Parse(format!("bad tar.gz entry path: {e}")))?
            .file_name()
            .map(|n| n.to_os_string())
            .and_then(|n| n.into_string().ok());
        if entry.header().entry_type().is_file()
            && matches!(name.as_deref(), Some("dutchman" | "dutchman.exe"))
        {
            let out = dest.join(name.clone().unwrap_or_else(|| "dutchman".into()));
            entry
                .unpack(&out)
                .map_err(|e| Error::Parse(format!("extract: {e}")))?;
            found = Some(out);
            break;
        }
    }
    found.ok_or_else(|| Error::Parse("the .tar.gz contains no dutchman binary".into()))
}

/// What an installed Dutchman binary came from, recorded in the info file
/// beside it. `marker` distinguishes builds of a channel: the tag for stable
/// and pinned releases, the release's `updated_at` for `nightly` (the tag never
/// changes, the build does).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DutchmanInfo {
    pub channel: String,
    pub tag: String,
    pub marker: String,
}

/// The info file recording which release the binary at `dest` came from.
pub fn dutchman_info_path(dest: &Path) -> std::path::PathBuf {
    let name = dest
        .file_name()
        .map(|n| n.to_os_string())
        .and_then(|n| n.into_string().ok())
        .unwrap_or_else(|| "dutchman".into());
    dest.with_file_name(format!("{name}.info"))
}

pub fn read_dutchman_info(dest: &Path) -> Option<DutchmanInfo> {
    let bytes = std::fs::read(dutchman_info_path(dest)).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// The marker that identifies the current build of a release for a channel:
/// the tag for `latest`/pinned channels, the release's `updated_at` for
/// `nightly` (whose tag is constant while its contents change daily).
fn dutchman_marker(channel: &str, rel: &Release) -> String {
    if channel == "nightly" {
        rel.updated_at
            .clone()
            .unwrap_or_else(|| rel.published_at.clone().unwrap_or_default())
    } else {
        rel.tag_name.clone()
    }
}

/// Download a Dutchman release (verified against its `.sha256` sidecar) and
/// install its `dutchman` binary at `dest`, recording where it came from.
/// `tag` is the release to install; `channel` the settings channel it satisfies.
pub async fn apply_dutchman_tag(dest: &Path, tag: &str, channel: &str) -> Result<Release, Error> {
    static DUTCHMAN_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    let _guard = DUTCHMAN_LOCK.lock().await;
    let rel = fetch_dutchman_release(tag).await?;
    let asset = dutchman_linux_tarball(&rel).ok_or_else(|| {
        Error::Parse("no linux tar.gz for this architecture in the Dutchman release".into())
    })?;
    let url = asset
        .download_url()
        .ok_or_else(|| Error::Parse("release asset has no download URL".into()))?
        .to_string();
    if !url.starts_with("https://github.com/dutchman-cad/dutchman/releases/download/") {
        return Err(Error::Parse(
            "refusing download URL outside GitHub Dutchman releases".into(),
        ));
    }
    let sidecar = download(&format!("{url}.sha256"), 16 * 1024).await?;
    let want = dutchman_sidecar_sha(&sidecar)
        .ok_or_else(|| Error::Parse(".sha256 sidecar has no usable hash".into()))?;
    let bytes = download(&url, DUTCHMAN_MAX_DOWNLOAD_BYTES).await?;
    let got = sha256_hex(&bytes);
    if !got.eq_ignore_ascii_case(&want) {
        return Err(Error::Parse(format!(
            "dutchman tarball sha256 mismatch (sidecar {want}, got {got})"
        )));
    }
    let dir = std::env::temp_dir().join(format!("pdm-dutchman-{}", std::process::id()));
    let extracted = extract_dutchman(&bytes, &dir)?;
    install_file(&extracted, dest)?;
    let _ = std::fs::remove_dir_all(&dir);
    let info = DutchmanInfo {
        channel: channel.to_string(),
        tag: rel.tag_name.clone(),
        marker: dutchman_marker(channel, &rel),
    };
    if let Err(e) = std::fs::write(
        dutchman_info_path(dest),
        serde_json::to_vec(&info).unwrap_or_else(|_| b"{}".to_vec()),
    ) {
        tracing::debug!(%e, "could not write the Dutchman info file");
    }
    Ok(rel)
}

/// Download the newest stable Dutchman release and install it at `dest`.
pub async fn apply_dutchman(dest: &Path) -> Result<String, Error> {
    let rel = apply_dutchman_tag(dest, "latest", "latest").await?;
    Ok(rel.tag_name)
}

/// Whether the installed binary satisfies `channel`: the info file's channel and
/// marker match the newest release of that channel.
async fn dutchman_satisfied(dest: &Path, channel: &str) -> Result<bool, Error> {
    let info = match read_dutchman_info(dest) {
        Some(i) => i,
        None => return Ok(false),
    };
    if info.channel != channel {
        return Ok(false);
    }
    let rel = if channel == "latest" {
        fetch_dutchman_latest().await?
    } else {
        fetch_dutchman_release(channel).await?
    };
    Ok(info.marker == dutchman_marker(channel, &rel))
}

/// Make sure the dutchman binary at `dest` satisfies `channel`, pulling the
/// referenced release when it does not (the worker's on-demand install). Returns
/// the release tag when it downloaded, `None` when satisfied or updates are
/// disabled.
pub async fn ensure_dutchman_for(dest: &Path, channel: &str) -> Result<Option<String>, Error> {
    if dutchman_update_disabled() {
        return Ok(None);
    }
    if dest.exists() && dutchman_satisfied(dest, channel).await.unwrap_or(false) {
        return Ok(None);
    }
    let tag = if channel == "latest" {
        "latest".to_string()
    } else {
        channel.to_string()
    };
    let rel = apply_dutchman_tag(dest, &tag, channel).await?;
    tracing::info!(channel, tag = %rel.tag_name, "installed the referenced Dutchman release");
    Ok(Some(rel.tag_name))
}

/// Install or update the Dutchman binary at `dest` to satisfy `channel` (the
/// boot/interval check). Silent when satisfied; a missing binary installs the
/// referenced release.
pub async fn boot_dutchman_update(dest: &Path, channel: &str) {
    if dutchman_update_disabled() {
        return;
    }
    match ensure_dutchman_for(dest, channel).await {
        Ok(Some(tag)) => tracing::info!(channel, "Dutchman updated to {tag}"),
        Ok(None) => tracing::debug!(channel, "Dutchman is current for the channel"),
        Err(e) => tracing::warn!(channel, "Dutchman update failed: {e}"),
    }
}

/// The System-panel channel menu: `latest`, `nightly`, the onshape-preview
/// builds, and the recent stable tags.
pub async fn dutchman_channels() -> Result<Vec<(String, String)>, Error> {
    let client = reqwest::Client::new();
    let url = format!("https://api.github.com/repos/{DUTCHMAN_GITHUB}/releases?per_page=20");
    let resp = client
        .get(&url)
        .header("User-Agent", "foundry-pdm")
        .timeout(REQUEST_TIMEOUT)
        .send()
        .await
        .map_err(|e| Error::Network(e.to_string()))?;
    if !resp.status().is_success() {
        return Err(Error::Network(format!("HTTP {} from {url}", resp.status())));
    }
    let body = read_capped(resp, MAX_RELEASE_BYTES).await?;
    let releases: Vec<Release> =
        serde_json::from_slice(&body).map_err(|e| Error::Parse(format!("release list: {e}")))?;
    let mut options: Vec<(String, String)> = Vec::new();
    let mut stable: Vec<&Release> = Vec::new();
    for rel in &releases {
        if rel.prerelease && rel.tag_name == "nightly" {
            let date = rel
                .name
                .as_deref()
                .and_then(|n| n.rsplit(' ').next())
                .unwrap_or("");
            options.push((
                "nightly".into(),
                format!("Nightly {date} (includes unreleased work)"),
            ));
        } else if rel.prerelease && rel.tag_name.starts_with("onshape-preview") {
            options.push((
                rel.tag_name.clone(),
                rel.name.clone().unwrap_or_else(|| rel.tag_name.clone()),
            ));
        } else if !rel.prerelease {
            stable.push(rel);
        }
    }
    if let Some(latest) = stable.first() {
        options.insert(
            0,
            (
                "latest".into(),
                format!(
                    "Latest stable ({})",
                    latest.tag_name.trim_start_matches('v')
                ),
            ),
        );
    }
    // Recent stable tags for pinning (newest first), after the channels above.
    for rel in stable.iter().skip(1).take(5) {
        options.push((
            rel.tag_name.clone(),
            format!("Pinned {}", rel.tag_name.trim_start_matches('v')),
        ));
    }
    Ok(options)
}

// ── SolidCraft web build (the "Edit in SolidCraft web" app bundled in the
//    portal image, installed under <portal>/editors/solidcraft/) ────────────

/// Set to `0` to skip the periodic SolidCraft web build update.
pub const SC_WEB_UPDATE_ENV: &str = "PDM_WEB_UPDATE";

pub fn sc_web_update_disabled() -> bool {
    std::env::var(SC_WEB_UPDATE_ENV).is_ok_and(|v| v == "0")
}

/// The directory the SolidCraft web build is served from, when the server runs
/// with a portal directory (`PDM_PORTAL_DIR/editors/solidcraft`, as the image
/// does). `None` in source-tree dev runs, where there is nothing to update in
/// place.
pub fn solidcraft_web_dir() -> Option<std::path::PathBuf> {
    std::env::var("PDM_PORTAL_DIR")
        .ok()
        .map(|p| {
            std::path::PathBuf::from(p)
                .join("editors")
                .join("solidcraft")
        })
        .filter(|p| p.is_dir())
}

/// Hours between SolidCraft web build checks (default 6). `PDM_WEB_UPDATE_HOURS`.
pub fn sc_web_interval() -> Duration {
    let hours = std::env::var("PDM_WEB_UPDATE_HOURS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .filter(|h| *h > 0)
        .unwrap_or(6);
    Duration::from_secs(hours.saturating_mul(3600))
}

/// The tag of the installed web build (`0.30.1`), read from the build's
/// `build.json` (`"version": "0.30.1-<hash>"`); `None` when the directory
/// holds no verified build.
pub fn installed_solidcraft_web_version(dir: &Path) -> Option<String> {
    let bytes = std::fs::read(dir.join("build.json")).ok()?;
    let value: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    let full = value
        .get("version")
        .and_then(serde_json::Value::as_str)?
        .trim();
    let tag = full.split('-').next()?.trim();
    if tag.is_empty() || parse_version(tag).is_none() {
        None
    } else {
        Some(tag.to_string())
    }
}

/// The release asset that carries the web build: `solidcraft-web-<version>.zip`.
pub fn sc_web_zip_asset(rel: &Release) -> Option<&Asset> {
    rel.assets.iter().find(|a| {
        let n = a.name.to_ascii_lowercase();
        n.starts_with("solidcraft-web-") && n.ends_with(".zip")
    })
}

/// The hex SHA-256 of `file` in a GitHub `SHA256SUMS.txt` (`<sha256>  <file>`,
/// `*` allowed for binary mode).
pub fn sha256_sums_entry(sums: &[u8], file: &str) -> Option<String> {
    for line in sums.split(|&b| b == b'\n') {
        let line = std::str::from_utf8(line).ok()?;
        let mut fields = line.split_whitespace();
        let digest = fields.next()?.trim_start_matches('*');
        let name = fields.next()?;
        if name == file
            && digest.len() == 64
            && digest
                .bytes()
                .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
        {
            return Some(digest.to_string());
        }
    }
    None
}

/// Unpack the web release zip under `work`, refusing entries outside the
/// `solidcraft-web-<tag>/` root, and returning that root once it is verified
/// to carry a web build (`index.html` + `build.json`).
pub fn extract_web_zip(bytes: &[u8], tag: &str, work: &Path) -> Result<std::path::PathBuf, Error> {
    use zip::ZipArchive;
    const MAX_UNCOMPRESSED: u64 = 512 * 1024 * 1024;
    let top = format!("solidcraft-web-{tag}");
    let mut archive = ZipArchive::new(std::io::Cursor::new(bytes))
        .map_err(|e| Error::Parse(format!("bad zip: {e}")))?;
    let mut total: u64 = 0;
    let mut saw_index = false;
    let mut saw_build = false;
    for i in 0..archive.len() {
        let mut file = archive
            .by_index(i)
            .map_err(|e| Error::Parse(format!("bad zip entry: {e}")))?;
        let name = file.name().to_string();
        if file.is_symlink() {
            return Err(Error::Parse(format!("zip entry is a symlink: {name}")));
        }
        let rel = name
            .strip_prefix(&top)
            .ok_or_else(|| Error::Parse(format!("zip entry outside {top}/: {name}")))?
            .trim_start_matches('/');
        if rel.split('/').any(|c| c == "..") {
            return Err(Error::Parse(format!("zip entry with .. segment: {name}")));
        }
        if file.is_dir() {
            let dir = work.join(&top).join(rel);
            std::fs::create_dir_all(&dir)
                .map_err(|e| Error::Parse(format!("mkdir {dir:?}: {e}")))?;
            continue;
        }
        if rel.is_empty() {
            continue;
        }
        let out = work.join(&top).join(rel);
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| Error::Parse(format!("mkdir {parent:?}: {e}")))?;
        }
        let mut body = Vec::new();
        std::io::copy(&mut file, &mut body)
            .map_err(|e| Error::Parse(format!("read zip entry {name}: {e}")))?;
        total += body.len() as u64;
        if total > MAX_UNCOMPRESSED {
            return Err(Error::Parse(format!(
                "web build unpacks to more than the {MAX_UNCOMPRESSED}-byte cap"
            )));
        }
        // Write to a sibling name first, then rename: a half-written asset is
        // never visible under its final name.
        let tmp = out.with_file_name(format!(
            ".{}~",
            out.file_name().unwrap_or_default().to_string_lossy()
        ));
        std::fs::write(&tmp, &body).map_err(|e| Error::Parse(format!("write {out:?}: {e}")))?;
        std::fs::rename(&tmp, &out).map_err(|e| Error::Parse(format!("rename to {out:?}: {e}")))?;
        saw_index |= rel == "index.html";
        saw_build |= rel == "build.json";
    }
    if !saw_index || !saw_build {
        return Err(Error::Parse(
            "zip holds no web build (index.html and build.json required)".into(),
        ));
    }
    Ok(work.join(top))
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(bytes))
}

fn copy_web_directory(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        let target = to.join(entry.file_name());
        if kind.is_dir() {
            copy_web_directory(&entry.path(), &target)?;
        } else if kind.is_file() {
            std::fs::copy(entry.path(), target)?;
        } else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "web build contains a symlink or special file",
            ));
        }
    }
    Ok(())
}

// Docker overlayfs cannot rename a directory inherited from an image layer.
// Copy that directory into the writable layer before replacing it. Work and
// destination share a parent filesystem, so the new build can be renamed in.
fn install_web_build(
    built: &Path,
    dest: &Path,
    work: &Path,
    mut rename: impl FnMut(&Path, &Path) -> std::io::Result<()>,
) -> Result<(), Error> {
    let old = work.join("previous-build");
    let had_old = dest.exists();
    if had_old && let Err(e) = rename(dest, &old) {
        if e.kind() != std::io::ErrorKind::CrossesDevices {
            return Err(Error::Parse(format!("swap out {dest:?}: {e}")));
        }
        copy_web_directory(dest, &old)
            .map_err(|e| Error::Parse(format!("back up {dest:?}: {e}")))?;
        if let Err(e) = std::fs::remove_dir_all(dest) {
            let restore = copy_web_directory(&old, dest);
            return Err(Error::Parse(format!(
                "remove old {dest:?}: {e}; restore: {restore:?}"
            )));
        }
    }
    if let Err(e) = rename(built, dest) {
        if had_old && let Err(restore) = rename(&old, dest) {
            // Keep the backup available even if automatic restoration fails.
            return Err(Error::Parse(format!(
                "swap in {dest:?}: {e}; restore failed: {restore}; backup at {old:?}"
            )));
        }
        return Err(Error::Parse(format!("swap in {dest:?}: {e}")));
    }
    Ok(())
}

/// Download the latest GitHub SolidCraft release's web build (verified against
/// the release's `SHA256SUMS.txt`) and install it at `dest` in place. Returns
/// the installed tag.
pub async fn apply_solidcraft_web(dest: &Path) -> Result<String, Error> {
    static UPDATE_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    let _guard = UPDATE_LOCK.lock().await;
    let rel = fetch_cad_cli_latest().await?;
    let tag = rel.tag_name.trim_start_matches('v').to_string();
    if parse_version(&tag).is_none() {
        return Err(Error::Parse(format!("release tag {tag} is not a version")));
    }
    let asset = sc_web_zip_asset(&rel).ok_or_else(|| {
        Error::Parse("no solidcraft-web-*.zip asset in the SolidCraft release".into())
    })?;
    let zip_name = asset.name.clone();
    if !zip_name.eq_ignore_ascii_case(&format!("solidcraft-web-{tag}.zip")) {
        return Err(Error::Parse(format!(
            "release {tag} carries an unexpected web asset {zip_name}"
        )));
    }
    let url = asset
        .download_url()
        .ok_or_else(|| Error::Parse("release asset has no download URL".into()))?;
    if !url.starts_with("https://github.com/tangency-labs/solidcraft/releases/download/") {
        return Err(Error::Parse(
            "refusing download URL outside GitHub SolidCraft releases".into(),
        ));
    }
    let Some(base) = url.rfind('/') else {
        return Err(Error::Parse("cannot derive the SHA256SUMS.txt URL".into()));
    };
    let sums_url = format!("{}/SHA256SUMS.txt", &url[..base]);
    let sums = download(&sums_url, 256 * 1024).await?;
    let want = sha256_sums_entry(&sums, &zip_name)
        .ok_or_else(|| Error::Parse("SHA256SUMS.txt has no entry for the web zip".into()))?;
    let zip_bytes = download(url, MAX_DOWNLOAD_BYTES).await?;
    let got = sha256_hex(&zip_bytes);
    if !got.eq_ignore_ascii_case(&want) {
        return Err(Error::Parse(format!(
            "web zip sha256 mismatch (SHA256SUMS {want}, got {got})"
        )));
    }
    let parent = dest
        .parent()
        .ok_or_else(|| Error::Parse("install destination has no parent".into()))?;
    std::fs::create_dir_all(parent).map_err(|e| Error::Parse(format!("create {parent:?}: {e}")))?;
    let work = tempfile::Builder::new()
        .prefix(".pdm-scweb-")
        .tempdir_in(parent)
        .map_err(|e| Error::Parse(format!("create work directory in {parent:?}: {e}")))?;
    let built = extract_web_zip(&zip_bytes, &tag, work.path())?;
    if let Err(e) = install_web_build(&built, dest, work.path(), |from, to| {
        std::fs::rename(from, to)
    }) {
        // Preserve the backup if rollback could not be completed.
        if work.path().join("previous-build").exists() {
            let _ = work.keep();
        }
        return Err(e);
    }
    Ok(tag)
}

/// If GitHub has a newer SolidCraft web build than the installed one, install
/// it (the mirror of the CAD CLI boot update).
pub async fn boot_solidcraft_web_update(dest: &Path) {
    if sc_web_update_disabled() {
        return;
    }
    let Some(current) = installed_solidcraft_web_version(dest) else {
        tracing::debug!("SolidCraft web check skipped: no installed build at {dest:?}");
        return;
    };
    match fetch_cad_cli_latest().await {
        Ok(rel) => {
            let tag = rel.tag_name.trim_start_matches('v').to_string();
            if !is_newer(&tag, &current) {
                tracing::info!("SolidCraft web build is current ({current})");
                return;
            }
            tracing::info!("SolidCraft web build {current} → {tag}; downloading from GitHub");
            match apply_solidcraft_web(dest).await {
                Ok(installed) => tracing::info!("SolidCraft web build updated to {installed}"),
                Err(e) => tracing::warn!("SolidCraft web build update failed: {e}"),
            }
        }
        Err(e) => tracing::debug!(%e, "SolidCraft web release check failed (boot continues)"),
    }
}

/// Keep the web build current without a reboot: check now, then on an interval.
pub async fn loop_solidcraft_web_update(dest: std::path::PathBuf) {
    loop {
        boot_solidcraft_web_update(&dest).await;
        tokio::time::sleep(sc_web_interval()).await;
    }
}

/// The release-check result the server exposes on `/version` and `/system-info`.
#[derive(Debug, Clone, Default)]
pub struct UpdateState {
    /// The newest published release tag (`None` = not checked yet / no releases).
    pub latest: Option<String>,
    /// Whether that release is newer than this build.
    pub update_available: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── version compare ─────────────────────────────────────────────────

    #[test]
    fn semver_ordering() {
        assert_eq!(parse_version("0.1.0"), parse_version("v0.1.0"));
        assert_eq!(parse_version("0.1.0"), parse_version("V0.1.0"));
        assert_eq!(parse_version("0.1"), parse_version("0.1.0"));
        assert!(is_newer("0.2.0", "0.1.0"));
        assert!(is_newer("1.0.0", "0.9.9"));
        assert!(is_newer("0.1.1", "0.1.0"));
        assert!(!is_newer("0.1.0", "0.1.0"));
        assert!(!is_newer("0.0.9", "0.0.10")); // numeric, not lexicographic
        assert!(is_newer("0.0.10", "0.0.9"));
    }

    #[test]
    fn prerelease_below_release() {
        assert!(!is_newer("0.1.0-rc.1", "0.1.0"));
        assert!(is_newer("0.1.0", "0.1.0-rc.1"));
        assert!(is_newer("0.2.0-rc.1", "0.1.0"));
    }

    #[test]
    fn build_tags() {
        assert_eq!(parse_version("V.01-34"), parse_version("v.01-34"));
        assert_eq!(parse_version("V.01-034"), parse_version("V.01-34")); // leading zeros
        assert!(is_newer("V.01-35", "V.01-34"));
        assert!(is_newer("V.02-01", "V.01-99"));
        assert!(!is_newer("V.01-34", "V.01-34"));
        // The total order across the two shapes: the series number is the major.
        assert!(is_newer("V.01-34", "0.1.0")); // (1,0,34) > (0,1,0)
        assert!(!is_newer("0.1.0", "V.01-34"));
        assert!(is_newer("V.01-34", "V.01-33"));
    }

    #[test]
    fn unparseable_never_newer() {
        for v in [
            "", "   ", "unknown", "latest", "V.01", "0.1.0.2", "x.y.z", "V.-1-2",
        ] {
            assert!(parse_version(v).is_none(), "{v}");
            assert!(!is_newer(v, "0.1.0"));
            assert!(!is_newer("0.1.0", v));
        }
        assert!(cmp_versions("V.01-34", "0.1.0").is_some());
        assert!(cmp_versions("V.01-34", "nope").is_none());
    }

    // ── asset selection ─────────────────────────────────────────────────

    fn rel_with(names: &[&str]) -> Release {
        Release {
            tag_name: "V.01-35".into(),
            assets: names
                .iter()
                .map(|n| Asset {
                    name: (*n).into(),
                    browser_download_url: Some(format!("https://gitea.example.com/owner/solidcraft-pdm/releases/download/V.01-35/{n}")),
                    url: None,
                    size: None,
                })
                .collect(),
            ..Default::default()
        }
    }

    #[test]
    fn picks_the_asset_for_this_os_arch() {
        let rel = rel_with(&[
            "pdm-cli-V.01-35-linux-x86_64.tar.gz",
            "pdm-cli-V.01-35-linux-aarch64.tar.gz",
            "pdm-cli-V.01-35-windows-x86_64.zip",
            "pdm-cli-V.01-35-macos-arm64.tar.gz",
            "docker-manifest.json",
        ]);
        assert_eq!(pick_asset(&rel, "linux", "x86_64"), Some(0));
        assert_eq!(pick_asset(&rel, "linux", "aarch64"), Some(1));
        assert_eq!(pick_asset(&rel, "windows", "x86_64"), Some(2));
        assert_eq!(pick_asset(&rel, "macos", "aarch64"), Some(3));
        assert_eq!(pick_asset(&rel, "linux", "arm64"), Some(1)); // alias
        assert_eq!(pick_asset(&rel, "linux", "amd64"), Some(0)); // alias
    }

    #[test]
    fn asset_names_are_case_insensitive() {
        let rel = rel_with(&["PDM-CLI-Linux-X86_64.tar.gz"]);
        assert_eq!(pick_asset(&rel, "linux", "x86_64"), Some(0));
    }

    #[test]
    fn missing_asset_is_none() {
        let rel = rel_with(&["pdm-cli-linux-x86_64.tar.gz"]);
        assert_eq!(pick_asset(&rel, "macos", "x86_64"), None);
        assert_eq!(pick_asset(&rel, "linux", "riscv64"), None);
        assert_eq!(pick_asset(&Release::default(), "linux", "x86_64"), None);
    }

    #[test]
    fn ambiguous_assets_resolve_to_the_pdm_cli_one() {
        let rel = rel_with(&["docs-linux-x86_64.tar.gz", "pdm-cli-linux-x86_64.tar.gz"]);
        assert_eq!(pick_asset(&rel, "linux", "x86_64"), Some(1));
        // Two pdm-cli candidates: ambiguous → no pick.
        let rel = rel_with(&[
            "pdm-cli-linux-x86_64.tar.gz",
            "pdm-cli-debug-linux-x86_64.tar.gz",
        ]);
        assert_eq!(pick_asset(&rel, "linux", "x86_64"), None);
    }

    // ── URL allow-list ──────────────────────────────────────────────────

    #[test]
    fn release_urls_only_from_the_repository() {
        let base = "https://gitea.example.com";
        assert!(release_url_allowed(
            "https://gitea.example.com/owner/solidcraft-pdm/releases/download/V.01-35/x.tar.gz",
            base
        ));
        assert!(release_url_allowed(
            "https://gitea.example.com/owner/solidcraft-pdm/releases_download/V.01-35/x.tar.gz",
            base
        ));
        assert!(release_url_allowed(
            "https://gitea.example.com/api/v1/repos/owner/solidcraft-pdm/releases/7/assets/x.tar.gz",
            base
        ));
        // Look-alike repo, other path, other host.
        assert!(!release_url_allowed(
            "https://gitea.example.com/owner/solidcraft-pdm-evil/releases/download/x",
            base
        ));
        assert!(!release_url_allowed(
            "https://gitea.example.com/owner/solidcraft-pdm/wiki",
            base
        ));
        assert!(!release_url_allowed(
            "https://other.example/owner/solidcraft-pdm/releases/download/x",
            base
        ));
        // A base with a trailing slash still matches.
        assert!(release_url_allowed(
            "https://gitea.example.com/owner/solidcraft-pdm/releases/download/x",
            "https://gitea.example.com/"
        ));
    }

    // ── release JSON parsing (hostile) ─────────────────────────────────

    #[test]
    fn parses_a_canned_gitea_body() {
        let body = br#"{"id":7,"tag_name":"V.01-35","name":"35","html_url":"https://gitea.example.com/owner/solidcraft-pdm/releases/tag/V.01-35","published_at":"2026-10-30T12:00:00Z","assets":[{"id":1,"name":"pdm-cli-linux-x86_64.tar.gz","browser_download_url":"https://gitea.example.com/owner/solidcraft-pdm/releases/download/V.01-35/pdm-cli-linux-x86_64.tar.gz","size":1024}]}"#;
        let rel = parse_release(body).unwrap();
        assert_eq!(rel.tag_name, "V.01-35");
        assert_eq!(rel.assets.len(), 1);
        assert_eq!(
            rel.assets[0].download_url(),
            Some(
                "https://gitea.example.com/owner/solidcraft-pdm/releases/download/V.01-35/pdm-cli-linux-x86_64.tar.gz"
            )
        );
        assert_eq!(
            rel.page_url(),
            Some("https://gitea.example.com/owner/solidcraft-pdm/releases/tag/V.01-35")
        );
    }

    #[test]
    fn tolerant_of_missing_fields() {
        let rel = parse_release(br#"{"tag_name":"V.01-35"}"#).unwrap();
        assert!(rel.assets.is_empty());
        assert_eq!(rel.page_url(), None);
        let rel = parse_release(br#"{"tag_name":"V.01-35","assets":[{"name":"x"}]}"#).unwrap();
        assert_eq!(rel.assets[0].download_url(), None);
    }

    #[test]
    fn hostile_bodies_are_errors_not_panics() {
        assert!(matches!(parse_release(b"not json"), Err(Error::Parse(_))));
        assert!(matches!(parse_release(b"{}"), Err(Error::Parse(_))));
        assert!(matches!(
            parse_release(b"\"just a string\""),
            Err(Error::Parse(_))
        ));
        assert!(matches!(parse_release(b"null"), Err(Error::Parse(_))));
        assert!(matches!(parse_release(b"[1,2]"), Err(Error::Parse(_))));
        assert!(matches!(parse_release(b"\""), Err(Error::Parse(_))));
        // tag_name of the wrong type.
        assert!(matches!(
            parse_release(b"{\"tag_name\":5}"),
            Err(Error::Parse(_))
        ));
    }

    #[test]
    fn over_capped_asset_lists_are_truncated() {
        let assets: String = (0..MAX_ASSETS + 10)
            .map(|i| format!("{{\"name\":\"a{i}\"}}"))
            .collect::<Vec<_>>()
            .join(",");
        let body = format!("{{\"tag_name\":\"V.01-35\",\"assets\":[{assets}]}}");
        let rel = parse_release(body.as_bytes()).unwrap();
        assert_eq!(rel.assets.len(), MAX_ASSETS);
    }

    // ── managed by a launcher ──────────────────────────────────────────

    #[test]
    fn managed_detection_env_and_record() {
        // Env var wins, control chars dropped, capped.
        let env = |k: &str| (k == MANAGED_ENV).then(|| "My Launcher\0!".into());
        let read = |_p: &Path| Some(b"{}".to_vec());
        assert_eq!(managed_by(&env, None, &read), Some("My Launcher!".into()));
        // Whitespace-only env → fall through to the record.
        let env = |k: &str| (k == MANAGED_ENV).then(|| "   ".into());
        let read =
            |p: &Path| (p.file_name().is_some()).then(|| b"{\"manager\":\"Launcher X\"}".to_vec());
        assert_eq!(
            managed_by(&env, Some(Path::new("/opt/pdm")), &read),
            Some("Launcher X".into())
        );
        // managed_by key variant.
        let read = |_: &Path| Some(b"{\"managed_by\":\"Launcher Y\"}".to_vec());
        assert_eq!(
            managed_by(&env, Some(Path::new("/opt/pdm")), &read),
            Some("Launcher Y".into())
        );
        // No record, no env → self-managed.
        let env = |_: &str| None;
        let read = |_: &Path| None;
        assert_eq!(managed_by(&env, Some(Path::new("/opt/pdm")), &read), None);
        // Hostile record files.
        for body in [
            &b""[..],
            b"not json",
            b"[1]",
            b"{}",
            b"{\"manager\":42}",
            b"{\"manager\":\"  \"}",
        ] {
            let read = |_: &Path| Some(body.to_vec());
            assert_eq!(
                managed_by(&env, Some(Path::new("/opt/pdm")), &read),
                None,
                "{body:?}"
            );
        }
    }

    // ── markers ────────────────────────────────────────────────────────

    #[test]
    fn marker_round_trips_and_gates_the_check() {
        let dir = std::env::temp_dir().join(format!(
            "pdm-update-marker-{}-{:?}",
            std::process::id(),
            "t"
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("marker.json");
        assert!(check_due(read_marker(&p).as_ref())); // absent
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        write_marker(
            &p,
            &Marker {
                ts: now,
                latest: Some("V.01-35".into()),
                available: true,
            },
        )
        .unwrap();
        assert!(!check_due(read_marker(&p).as_ref())); // fresh
        let old = Marker {
            ts: now - CHECK_INTERVAL.as_secs() - 1,
            ..Default::default()
        };
        write_marker(&p, &old).unwrap();
        assert!(check_due(read_marker(&p).as_ref())); // stale
        let m = read_marker(&p).unwrap();
        assert_eq!(m.ts, old.ts);
        // Corrupt marker: treated as absent, never a panic.
        std::fs::write(&p, b"garbage").unwrap();
        assert!(read_marker(&p).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ── tarball extraction ─────────────────────────────────────────────

    fn make_tar_gz(entries: &[(&str, &[u8])]) -> Vec<u8> {
        use flate2::Compression;
        use flate2::write::GzEncoder;
        use std::io::Write;
        let mut buf: Vec<u8> = Vec::new();
        {
            let mut builder = tar::Builder::new(&mut buf);
            for (name, data) in entries {
                let mut header = tar::Header::new_gnu();
                header.set_size(data.len() as u64);
                header.set_mode(0o755);
                header.set_cksum();
                builder.append_data(&mut header, name, *data).unwrap();
            }
            builder.into_inner().unwrap();
        }
        let mut enc = GzEncoder::new(Vec::new(), Compression::fast());
        enc.write_all(&buf).unwrap();
        enc.finish().unwrap()
    }

    #[test]
    fn extracts_the_cli_from_a_targz() {
        let payload = b"\x7fELF-fake";
        let gz = make_tar_gz(&[
            ("pdm-cli-0.1.0-linux/bin/pdm-cli", payload),
            ("pdm-cli-0.1.0-linux/README.md", b"read me"),
        ]);
        let dir = std::env::temp_dir().join(format!("pdm-update-x-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let out = extract_cli(&gz, &dir).unwrap();
        assert!(out.exists());
        assert_eq!(std::fs::read(&out).unwrap(), payload);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn extract_errors_are_reported() {
        let dir = std::env::temp_dir().join(format!("pdm-update-x2-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        assert!(matches!(
            extract_cli(b"not a gzip", &dir),
            Err(Error::Parse(_))
        ));
        let gz = make_tar_gz(&[("hello.txt", b"hi")]);
        assert!(matches!(extract_cli(&gz, &dir), Err(Error::Parse(_))));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn helper_urls() {
        assert_eq!(
            latest_api_url("https://gitea.example.com"),
            "https://gitea.example.com/api/v1/repos/owner/solidcraft-pdm/releases/latest"
        );
        assert!(releases_page_url().ends_with("/owner/solidcraft-pdm/releases"));
        assert!(releases_page_url().starts_with("https://"));
    }

    // ── SolidCraft web build ────────────────────────────────────────────

    /// Build a zip in memory from `(name, bytes)` pairs (uncompressed).
    fn make_zip(entries: &[(&str, &[u8])]) -> Vec<u8> {
        use std::io::Write;
        use zip::write::ZipWriter;
        let mut buf: Vec<u8> = Vec::new();
        {
            let mut zw = ZipWriter::new(std::io::Cursor::new(&mut buf));
            for (name, data) in entries {
                let opts: zip::write::FileOptions<()> = zip::write::FileOptions::default()
                    .compression_method(zip::CompressionMethod::Stored);
                zw.start_file(name.to_string(), opts).unwrap();
                zw.write_all(data).unwrap();
            }
            zw.finish().unwrap();
        }
        buf
    }

    #[test]
    fn picks_the_web_zip_asset_only() {
        let rel = Release {
            tag_name: "v0.40.1".into(),
            assets: vec![
                Asset {
                    name: "solidcraft-0.40.1-linux-x86_64.tar.gz".into(),
                    browser_download_url: Some("https://x/a".into()),
                    url: None,
                    size: None,
                },
                Asset {
                    name: "solidcraft-web-0.40.1.zip".into(),
                    browser_download_url: Some("https://x/b".into()),
                    url: None,
                    size: None,
                },
                Asset {
                    name: "SHA256SUMS.txt".into(),
                    browser_download_url: Some("https://x/c".into()),
                    url: None,
                    size: None,
                },
            ],
            ..Default::default()
        };
        let got = sc_web_zip_asset(&rel).unwrap();
        assert_eq!(got.name, "solidcraft-web-0.40.1.zip");
        // A release with no web zip.
        let rel2 = Release {
            tag_name: "v0.40.1".into(),
            assets: vec![Asset {
                name: "solidcraft-0.40.1-linux-x86_64.tar.gz".into(),
                browser_download_url: None,
                url: None,
                size: None,
            }],
            ..Default::default()
        };
        assert!(sc_web_zip_asset(&rel2).is_none());
    }

    #[test]
    fn reads_a_sha256sums_entry() {
        let digest = "a1b2c3d4e5f60718293a4b5c6d7e8f90a1b2c3d4e5f60718293a4b5c6d7e8f90";
        assert_eq!(digest.len(), 64);
        let sums = format!(
            "{digest}  solidcraft-web-0.40.1.zip\n{digest}  solidcraft-0.40.1-linux-x86_64.tar.gz\n"
        );
        assert_eq!(
            sha256_sums_entry(sums.as_bytes(), "solidcraft-web-0.40.1.zip").as_deref(),
            Some(digest)
        );
        assert!(sha256_sums_entry(sums.as_bytes(), "missing.zip").is_none());
        assert!(sha256_sums_entry(b"", "x").is_none());
        // A 63-char digest is not a sha256.
        assert!(sha256_sums_entry(b"abc  x.zip", "x.zip").is_none());
    }

    #[test]
    fn extracts_a_verified_web_build() {
        let zip = make_zip(&[
            ("solidcraft-web-0.40.1/index.html", b"<html></html>"),
            (
                "solidcraft-web-0.40.1/build.json",
                br#"{"version":"0.40.1-abc","files":[]}"#,
            ),
            ("solidcraft-web-0.40.1/app.js", b"console.log(1)"),
            ("solidcraft-web-0.40.1/fonts/CraftUI-SC.otf", b"otf"),
        ]);
        let dir = std::env::temp_dir().join(format!("pdm-scweb-x-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let out = extract_web_zip(&zip, "0.40.1", &dir).unwrap();
        assert_eq!(out, dir.join("solidcraft-web-0.40.1"));
        assert!(out.join("index.html").is_file());
        assert_eq!(
            std::fs::read(out.join("app.js")).unwrap(),
            b"console.log(1)"
        );
        assert!(out.join("fonts/CraftUI-SC.otf").is_file());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn web_extract_refuses_out_of_root_and_missing_parts() {
        // An entry outside the solidcraft-web-<tag>/ root.
        let zip = make_zip(&[
            ("solidcraft-web-0.40.1/index.html", b"i"),
            ("other/app.js", b"x"),
        ]);
        let dir = std::env::temp_dir().join(format!("pdm-scweb-y-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        assert!(matches!(
            extract_web_zip(&zip, "0.40.1", &dir),
            Err(Error::Parse(_))
        ));
        // No build.json.
        let zip = make_zip(&[("solidcraft-web-0.40.1/index.html", b"i")]);
        assert!(matches!(
            extract_web_zip(&zip, "0.40.1", &dir),
            Err(Error::Parse(_))
        ));
        // Not a zip at all.
        assert!(matches!(
            extract_web_zip(b"not a zip", "0.40.1", &dir),
            Err(Error::Parse(_))
        ));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn web_install_handles_overlay_and_restores_on_failure()
    -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let dest = root.path().join("solidcraft");
        let work = root.path().join("work");
        let built = work.join("new");
        std::fs::create_dir_all(dest.join("assets"))?;
        std::fs::write(dest.join("assets/old.js"), b"old")?;
        std::fs::create_dir_all(&built)?;
        std::fs::write(built.join("index.html"), b"new")?;
        let mut calls = 0;
        let result = install_web_build(&built, &dest, &work, |from, to| {
            calls += 1;
            if calls == 1 {
                Err(std::io::Error::from(std::io::ErrorKind::CrossesDevices))
            } else if calls == 2 {
                Err(std::io::Error::other("simulated install failure"))
            } else {
                std::fs::rename(from, to)
            }
        });
        assert!(result.is_err());
        assert_eq!(std::fs::read(dest.join("assets/old.js"))?, b"old");
        assert_eq!(std::fs::read(built.join("index.html"))?, b"new");
        calls = 0;
        install_web_build(&built, &dest, &work, |from, to| {
            calls += 1;
            if calls == 1 {
                Err(std::io::Error::from(std::io::ErrorKind::CrossesDevices))
            } else {
                std::fs::rename(from, to)
            }
        })?;
        assert_eq!(std::fs::read(dest.join("index.html"))?, b"new");
        assert_eq!(
            std::fs::read(work.join("previous-build/assets/old.js"))?,
            b"old"
        );
        Ok(())
    }

    #[test]
    fn installed_version_comes_from_build_json() {
        let dir = std::env::temp_dir().join(format!("pdm-scweb-v-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("build.json"),
            br#"{"version":"0.30.1-1aa735e4240f3c46","files":[]}"#,
        )
        .unwrap();
        assert_eq!(
            installed_solidcraft_web_version(&dir).as_deref(),
            Some("0.30.1")
        );
        // Newer full tag sorts above the installed one.
        assert!(is_newer(
            "0.40.1",
            installed_solidcraft_web_version(&dir).unwrap().as_str()
        ));
        assert!(!is_newer(
            "0.30.1",
            installed_solidcraft_web_version(&dir).unwrap().as_str()
        ));
        // Absent or malformed build.json.
        std::fs::remove_file(dir.join("build.json")).unwrap();
        assert!(installed_solidcraft_web_version(&dir).is_none());
        std::fs::write(dir.join("build.json"), b"not json").unwrap();
        assert!(installed_solidcraft_web_version(&dir).is_none());
        std::fs::write(dir.join("build.json"), br#"{"version":"0.40.1-abc"}"#).unwrap();
        assert_eq!(
            installed_solidcraft_web_version(&dir).as_deref(),
            Some("0.40.1")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ── Dutchman ─────────────────────────────────────────────────────────

    fn dutchman_rel() -> Option<Release> {
        let arch = match std::env::consts::ARCH {
            "x86_64" | "aarch64" => std::env::consts::ARCH,
            _ => return None, // the picker only claims this machine's linux tarball
        };
        let mut rel = Release {
            tag_name: "v0.2.9".into(),
            ..Default::default()
        };
        for name in [
            format!("dutchman-0.2.9-{arch}-unknown-linux-gnu.tar.gz"),
            format!("dutchman-0.2.9-{arch}-unknown-linux-gnu.tar.gz.sha256"),
            format!("dutchman-{arch}-unknown-linux-gnu.tar.gz"),
            "dutchman-0.2.9-macos-universal.tar.gz".to_string(),
        ] {
            rel.assets.push(Asset {
                browser_download_url: Some(format!(
                    "https://github.com/dutchman-cad/dutchman/releases/download/v0.2.9/{name}"
                )),
                url: None,
                size: None,
                name,
            });
        }
        Some(rel)
    }

    #[test]
    fn dutchman_tarball_prefers_the_versioned_asset() {
        let Some(mut rel) = dutchman_rel() else {
            return;
        };
        let pick = dutchman_linux_tarball(&rel).expect("a linux tarball");
        assert!(pick.name.starts_with("dutchman-0.2.9-"), "{}", pick.name);
        // No versioned asset: the unversioned README name is the fallback.
        rel.assets
            .retain(|a| !a.name.starts_with("dutchman-0.2.9-"));
        let pick = dutchman_linux_tarball(&rel).expect("the unversioned tarball");
        assert!(
            pick.name.ends_with("-unknown-linux-gnu.tar.gz") && !pick.name.contains("0.2.9"),
            "{}",
            pick.name
        );
        // No linux assets at all.
        rel = dutchman_rel().expect("release");
        rel.assets
            .retain(|a| a.name.contains("macos") && !a.name.ends_with(".sha256"));
        assert!(dutchman_linux_tarball(&rel).is_none());
    }

    #[test]
    fn dutchman_marker_tracks_the_channel_build() {
        let mut rel = Release {
            tag_name: "v0.2.9".into(),
            ..Default::default()
        };
        assert_eq!(dutchman_marker("latest", &rel), "v0.2.9");
        assert_eq!(dutchman_marker("onshape-preview-2", &rel), "v0.2.9");
        // A rolling tag is identified by its last update, not its constant tag.
        rel.tag_name = "nightly".into();
        rel.updated_at = Some("2026-11-15T14:39:31Z".into());
        assert_eq!(dutchman_marker("nightly", &rel), "2026-11-15T14:39:31Z");
        rel.updated_at = Some("2026-11-16T08:00:00Z".into());
        assert_ne!(dutchman_marker("nightly", &rel), "2026-11-15T14:39:31Z");
    }

    #[test]
    fn dutchman_sidecar_sha_takes_the_first_hex_field() {
        assert_eq!(
            dutchman_sidecar_sha(
                b"56f34583c3a966067e816bf25fff559b77add5f56a1bf25a0d8bd0e2d7dd1b2f  dutchman-0.2.9-x86_64-unknown-linux-gnu.tar.gz\n"
            ),
            Some("56f34583c3a966067e816bf25fff559b77add5f56a1bf25a0d8bd0e2d7dd1b2f".into())
        );
        assert_eq!(dutchman_sidecar_sha(b"not-a-hash  name.tar.gz"), None);
        assert_eq!(dutchman_sidecar_sha(b"short"), None);
        assert_eq!(dutchman_sidecar_sha(b""), None);
    }

    #[test]
    fn extract_dutchman_pulls_the_binary_from_the_release_folder() {
        use flate2::write::GzEncoder;
        let dir = tempfile::tempdir().unwrap();
        let entries: [(&str, &[u8]); 3] = [
            (
                "dutchman-0.2.9-x86_64-unknown-linux-gnu/dutchman",
                b"#!/bin/sh\necho dutchman 0.2.9\n",
            ),
            (
                "dutchman-0.2.9-x86_64-unknown-linux-gnu/dutchman-gui",
                b"gui",
            ),
            (
                "dutchman-0.2.9-x86_64-unknown-linux-gnu/README.md",
                b"# dutchman",
            ),
        ];
        let mut gz = GzEncoder::new(Vec::new(), flate2::Compression::fast());
        {
            let mut builder = tar::Builder::new(&mut gz);
            for (name, body) in entries {
                let mut header = tar::Header::new_gnu();
                header.set_size(body.len() as u64);
                header.set_mode(0o755);
                header.set_cksum();
                builder.append_data(&mut header, name, body).unwrap();
            }
        }
        let bytes = gz.finish().unwrap();
        let bin = extract_dutchman(&bytes, dir.path()).unwrap();
        assert!(bin.file_name().unwrap() == "dutchman");
        assert!(
            std::fs::read_to_string(&bin)
                .unwrap()
                .starts_with("#!/bin/sh")
        );
        // A tarball without the binary is an error, not a panic.
        let mut gz = GzEncoder::new(Vec::new(), flate2::Compression::fast());
        {
            let mut builder = tar::Builder::new(&mut gz);
            let mut header = tar::Header::new_gnu();
            header.set_size(3);
            header.set_mode(0o755);
            header.set_cksum();
            builder
                .append_data(
                    &mut header,
                    "dutchman-0.2.9-x86_64-unknown-linux-gnu/dutchman-gui",
                    &b"gui"[..],
                )
                .unwrap();
        }
        let bytes = gz.finish().unwrap();
        assert!(extract_dutchman(&bytes, dir.path()).is_err());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn dutchman_version_parsed_from_cli_output() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("dutchman");
        std::fs::write(
            &script,
            "#!/bin/sh\n[ \"$1\" = --version ] && echo 'dutchman 0.2.9'\n",
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(dutchman_version_of(&script).await.as_deref(), Some("0.2.9"));
        assert_eq!(dutchman_version_of(&dir.path().join("missing")).await, None);
    }
}
