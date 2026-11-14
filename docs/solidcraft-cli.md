# SolidCraft CLI bundled with PDM

PDM build **V.01-31** upgrades the CAD CLI from **0.21.0** to **0.27.1**.
The fork's PDM client and Inventor import additions are retained: this is a
build of the integrated fork, not a replacement with the upstream-only binary.

Source provenance:

- Fork: `/workspace/solidCraft-WPDM`, base `937277deb` (`main-v.02`).
- Latest published upstream release: `v0.27.1`, commit
  `aa1fe28ddff42cac2990a312765a1a6c50d97dfe`.
- The release changes were integrated with `git merge --squash v0.27.1` and
  the fork's workspace lockfile version was updated with `cargo update --workspace`.
  These source changes are local and uncommitted.
- Built with Rust 1.99.1 using `cargo build --release --locked -p solidcraft-cli`.

Both the API's in-process worker and the dedicated worker use
`/usr/local/bin/solidcraft-cli`. The existing `PDM_SOLVECRAFT_CLI` configuration
key is retained for compatibility. The legacy binary alias is also bundled,
but normal jobs call the canonical name without a deprecation warning.

Jobs continue to use the CLI's `eval`, `snapshot`, and `convert` interfaces.
System → SolidCraft CAD CLI reports the configured path and actual CLI version.

V.01-31 also fixes a worker path collision: conversion output uses a distinct
`-converted.step` filename, so clearing an old output cannot delete an uploaded
`.step` input. It also fixes the column aliases used to download a stored
derived STEP. The executable was rebuilt from PDM source base `5385a189`
plus these CAD pipeline fixes, in `/tmp/opencode/pdm-cli-worker-build`; pending backend
changes in the main PDM checkout are not part of this deployment.

## Rebuild only the CLI layer

Build the CAD checkout first, then use its `target/release` directory as the
Docker build context. The base image supplies the existing API, portal and
runtime libraries; this path does not rebuild pending PDM backend work.

```sh
# From the CAD checkout:
cargo build --release --locked -p solidcraft-cli

# From the PDM checkout (advance the image/build tags for the next deployment):
docker build -f docker/Dockerfile.cli-update \
  --build-arg PDM_BASE_IMAGE=docker-pdm:V.01-30 \
  --build-arg PDM_BUILD_TAG=V.01-31 \
  --build-arg SOLIDCRAFT_VERSION=0.27.1 \
  -t docker-pdm:V.01-31 ../solidCraft-WPDM/target/release
PDM_BUILD_TAG=V.01-31 docker compose -f docker/pdm-dev.yml \
  up -d --no-build --no-deps pdm pdm-worker
```

For this deployment, the rebuilt `pdm-server` was applied to the prepared CLI
image with `docker/Dockerfile.worker-update` and the PDM build's `target/release`
directory as context before recreating the services. Normal full image builds
include the worker fix directly from `crates/pdm-worker/src/lib.rs`.

## Validation

- Release CLI and alias report 0.27.1 inside the image.
- All 9 worker unit tests pass. The new STEP source-preservation regression
  was reproduced failing with the old filename, then passed with the fix.
- Production worker library passes targeted Clippy with `-D warnings`.
- Real API uploads/check-ins and queued jobs were tested with an isolated
  database and local blob store: a native design and its STEP each measure
  1920 mm³; the repository's Inventor box fixture measures 4000 mm³. All
  produce 512×512 PNGs; STEP and Inventor also produce downloadable derived STEP.
- System info reports the canonical CLI path and actual version.

The wider legacy test/check suite is not claimed passing: the existing vault
test has a `HeaderValue::as_str` compile error, and workspace-wide formatting
and dependency-inclusive Clippy report pre-existing issues outside these fixes.

Update `docker/.build-tag` after deployment and verify `/version`, CLI `--version`
in both containers, and a measurement/preview job. Full dev and production image
builds also use the canonical CLI path.
