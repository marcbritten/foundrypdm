//! Live check of the Dutchman release pipeline: newest GitHub release, asset
//! selection for this machine, the `.sha256` sidecar, extraction and install.
//! Gated on `PDM_DUTCHMAN_NET_TEST=1` so the default run stays offline.

#[tokio::test]
async fn apply_dutchman_installs_the_newest_release() {
    if std::env::var("PDM_DUTCHMAN_NET_TEST").is_err() {
        eprintln!("PDM_DUTCHMAN_NET_TEST not set; skipping the live Dutchman release test");
        return;
    }
    let dir = std::env::temp_dir().join(format!("pdm-dutchman-live-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let dest = dir.join("dutchman");
    let tag = pdm_update::apply_dutchman(&dest)
        .await
        .expect("apply_dutchman");
    assert!(dest.is_file(), "the binary was not installed");
    let installed = pdm_update::dutchman_version_of(&dest)
        .await
        .expect("an installed binary reports a version");
    let want = tag.trim_start_matches('v');
    assert!(
        installed == want || installed.starts_with(want),
        "installed {installed} is not the released {want}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
