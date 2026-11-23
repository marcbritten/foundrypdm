# Native SolidCraft conversion and transparent previews

Introduced in PDM **V.01-38**.

## Convert to SolidCraft

The item header, Preview tab and version rows offer **Convert to SolidCraft**
for the native CAD formats (Fusion `.f3d`/`.f3z`, Inventor `.ipt`/`.iam`,
SolidWorks `.sldprt`/`.sldasm`, FreeCAD `.FCStd`, CATIA `.CATPart`, Rhino `.3dm`,
Creo View `.pvz`, ACIS `.sat`/`.sab`/`.smt`/`.smb`, STEP `.step`/`.stp`), IGES
and supported mesh files. The authenticated editor/admin action queues a bounded
worker job for the selected immutable version. Repeated clicks reuse its active
job; an existing successful artifact is reused. Actual format/version support
depends on the configured converters; a refused source records the converter's
reason rather than claiming that conversion succeeded.

The worker picks the converter from the file format. The native CAD formats run
through [Dutchman Convert](https://github.com/dutchman-cad/dutchman) —
`dutchman convert INPUT -o OUTPUT.sc --preset standard --quiet --json`
(`--solidcraft` points it at the PDM's CLI so a rebuilt history is verified) —
which decodes the native file and **keeps the editable feature tree where it
maps** (the JSON report's `mode` is `history`, `partial_history` or `import`).
The PDM always converts with the newest published Dutchman release: the server
re-checks GitHub on boot and hourly, and the job pulls the release on demand if
the binary is missing. Meshes and IGES restate through
`solidcraft-cli run INPUT --save OUTPUT.sc --quiet` (geometry only); if Dutchman
is unavailable, Inventor/FreeCAD/Rhino/STEP fall back to that path as well. A
successful result becomes a content-addressed native download alongside the
source. The button changes to **Download SolidCraft**. Read-only users can
download an already created artifact.

Conversion does not replace the original blob, create an item head, change the
part number, or rewrite history. A STEP or other format without a readable
feature tree becomes a native design containing its imported geometry; conversion
does not reverse-engineer an original parametric feature tree that the format
does not carry. Use normal check-in separately if the native file should become
a new vault version. Native `.sc` uploads are recognized for analysis and preview.

New check-ins record the source extension on the version so a later change of
format does not make historical version processing use the wrong extension.

API (bearer authentication; organization-scoped):

- `POST /versions/{id}/solidcraft`: create/reuse the conversion, editor/admin.
- `GET /versions/{id}/solidcraft`: filename, artifact metadata and job state.
- `GET /versions/{id}/solidcraft/blob`: download the ready `.sc`.
- `GET /jobs/{id}`: inspect a job in the caller's organization.

Failures retain the source and record the conversion error. CLI processes have
timeouts and isolated temporary directories. Artifact persistence completes before
the queue job is marked done.

## Transparent PNG previews

The default is a **transparent product render**:

```sh
solidcraft-cli render INPUT --out preview.png --transparent --overwrite \
  --width 512 --height 512 --timeout 60
```

This uses the builtin CPU product-image renderer. It produces real PNG alpha,
without a viewport grid or axes. It does not erase pixels by matching a background
color, so white surfaces and visible material remain intact. The browser draws
the transparent areas against its active theme; rendered images have no rectangular
box shadow. Small version thumbnails fit the whole image rather than cropping
the top-left of a full-resolution PNG.

Settings → Preview renderer → **Preview style** selects `transparent` or the
previous `viewport` snapshot behavior. The old viewport path still uses the GPU
policy. API: `GET/PUT /settings/preview-style`; changing the style requires admin.

Existing PNGs have their old background baked in. Select a version in Preview and
use **Regenerate preview** (`POST /versions/{id}/preview`, editor/admin). The old
working image remains until replacement succeeds. Completion updates only the
affected preview/thumbnail and invalidates the portal's object-URL cache; it does
not periodically rebuild the item view or overwrite metadata drafts.

The conversion-to-STEP fallback now uses the supported CLI `run --out FILE.step`
interface rather than the removed `convert` subcommand.

## Validation

The existing vault e2e file includes an opt-in test using its original, generated
STEP pin. It covers concurrent conversion request reuse, viewer permissions,
native ZIP/SHA-256 download, preservation of original bytes/head/history,
preview regeneration and PNG alpha/visible-object checks. Run it against a
disposable server with worker/CLI enabled, never a production organization.

```sh
PDM_E2E=1 PDM_E2E_CLI=1 PDM_E2E_ADMIN_TOKEN=... \
  cargo test --release -p pdm-server --test e2e_vault \
  e2e_native_conversion_and_transparent_preview -- --ignored --nocapture
```

The token is supplied through the test environment and must not be committed or logged.

Verified for this delivery:

- 14 core and 9 worker unit tests pass.
- Production API/database/worker libraries pass targeted Clippy with `-D warnings`.
- The real-CLI e2e conversion/alpha test passes with both the image's bundled
  0.27.1 CLI and the live auto-updated 0.29.0 CLI.
- Browser tests against an isolated real API exercise conversion/download,
  preservation of metadata drafts, transparent outer pixels and a through-hole,
  both light/dark themes, the legacy viewport preference and thumbnail sizing.
- Cross-organization access to native artifacts, rendering actions and jobs is
  refused. Native uploads and format-changing versions retain their correct bytes
  and are processed using the selected version's format.
- The base-plate preview requested by the owner was regenerated after deployment;
  its source version/blob identity was checked unchanged.

Database migration 0016 adds nullable artifact metadata only. The migration runner
uses a transaction advisory lock so concurrently starting API/worker processes
apply the schema once. A pre-deployment database backup and the prior image are
retained for rollback.
