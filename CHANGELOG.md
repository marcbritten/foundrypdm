# Changelog

Foundry PDM server. Dates are UTC.

## V.01-39 — edit in SolidCraft web

- The Preview tab’s **✎ Edit in SolidCraft web** button (under the 3D model) opens
  the selected version in the SolidCraft CAD app: the upstream web release is
  pinned into the image (same installer as the other web editors) and runs in a
  same-origin iframe. Convertible CAD versions are restated to a native `.sc`
  design first, on demand.
- New unauthenticated `GET /public/design/{version}/{filename}`: the URL names the
  exact immutable file (the derived `.sc` artifact when one exists, otherwise the
  version’s source file), so the editor’s design-link fetch needs no token.
- **Save to PDM** in the editor collects the edited design from the app’s browser
  storage and checks it in as a new immutable version (editor role; same base-version
  and lock rules as the other web editors). **Download original / Download edited
  file** are offered alongside.

## Unreleased

### Fixed
- SolidCraft web editor updates now work on Docker overlay filesystems and
  restore the previous build if installing the replacement fails.
- Account status can be changed directly with **Disable / Enable** in both member
  management screens. Disabling blocks sign-in and existing user tokens; enabling
  restores access. The last active administrator cannot be disabled or demoted,
  including by concurrent requests.
- Portal dialogs now validate and submit correctly, support Enter to submit,
  and close only the topmost dialog on Escape. Apostrophes in names and filenames
  are escaped correctly in HTML attributes.
- SolidCraft's **Download edited file** now downloads the design and releases the
  busy state. Saves collect the current design, support successive check-ins, and
  retain a downloadable copy after a failed check-in.
- Historical PDF/DXF versions open using their own filenames. Native conversions
  deduplicate by content rather than filename and report check-in conflicts without
  silently retrying against a newer vault head.
- Workspace formatting and strict all-target Clippy checks pass; test setup errors
  propagate without the unwrap/expect calls prohibited by the crate lint policy.

### Added
- **SolidCraft web editor auto-update**: the bundled "Edit in SolidCraft web"
  build (image pin 0.40.1) now tracks the newest
  [tangency-labs/solidcraft](https://github.com/tangency-labs/solidcraft)
  release on boot and every 6 h (`PDM_WEB_UPDATE_HOURS`), verified against the
  release's `SHA256SUMS.txt` and swapped in place (`PDM_WEB_UPDATE=0` to opt
  out). System reports installed/latest tags with a check/update button
  (`GET/POST /settings/solidcraft-web[/update]`).
- **SolidCraft CLI auto-update** from [tangency-labs/solidcraft](https://github.com/tangency-labs/solidcraft) GitHub releases. The API and worker check every 6 hours (`PDM_CLI_UPDATE_HOURS`) and install `solidcraft-cli` when a newer tag exists. Opt out with `PDM_CLI_UPDATE=0`.
- **System → SolidCraft CAD CLI → Check / reinstall CLI** to pull the latest tarball without a reboot.
- **Onshape “Send to manufacturing”** right-panel extension (`/static/onshape.html`): export the **selected part** (STEP/STL) and check it into PDM as a version. Uses Onshape’s part picker and `partIds` on the translation API.
- **Mail server** in Settings (SMTP) and **Add user** vs **Invite user** in Administration. Invites email a temporary password.
- **Preview renderer** org setting: Auto / GPU / GPU software / CPU (`SOLIDCRAFT_GPU` on the worker).
- **Project edit** (name, description, cover image) on the project ⋮ menu.
- **Portal themes** (Dark, Light, Slate, High contrast) in Settings → Appearance.
- Docker GPU passthrough (`/dev/dri`) and `mesa-vulkan-drivers` in the runtime image.
- Docker/DB setup in the [README](README.md) and [docs/docker-server.md](docs/docker-server.md).

### Changed
- Bundled CAD CLI tracks GitHub latest (0.29.0 as of this entry) instead of a frozen image-only 0.27.1.

## V.01-38 — native conversions and transparent previews

- **Convert to SolidCraft** on item headers, Preview and version rows creates a
  downloadable native `.sc` artifact alongside the selected immutable source.
- Default previews use `render --transparent`, without grid, axes or a baked-in
  background. Settings retains the previous viewport snapshot style.
- **Regenerate preview** replaces older rendered images and refreshes only the
  affected preview/thumbnails. Version thumbnails now scale the entire model.
- Worker STEP restatement uses the current CLI `run --out` interface. Native
  `.sc` uploads are recognized, and job results persist before completion.

## V.01-37 — 2026-10-31

- CLI-only image update; Inventor-importing CAD CLI in the worker image.
- `pdm-cli update check/apply` against private Gitea releases.
- System panel: PDM build, SolidCraft CLI path/version, Postgres, object storage.

## V.01-36 and earlier

- Web portal regroup (Workspace / Data / Manufacturing / Action / Admin).
- Part-number prefixes, project/item archive and delete, 2 GiB upload cap.
- CAD convert job (checked-in files restated as STEP).
- MIT licence.
