# Online document editors

First deployed in web UI build **V.01-28**. Each web UI deployment increments
`PDM_BUILD_TAG`, tags the Docker image, and updates `docker/.build-tag`; the
portal displays that build from `GET /version`.

The web workspace offers **Download** and **Open in web editor** for PDFs
(PdfCraft) and DXF drawings (CADCraft). Actions are available in item details,
the file context menu, Preview, and individual version rows.

## Edit in SolidCraft web (V.01-39)

The Preview tab’s **✎ Edit in SolidCraft web** button (under the 3D model)
opens the selected version in the SolidCraft CAD app — the upstream web release,
pinned into the image by the same installer (`docker/install-web-editors.sh`) and
served at `/static/editors/solidcraft/`, so it runs on the portal’s own origin.

- **Opening.** The version’s bytes come from `GET /public/design/{version}/{filename}`.
  That route is unauthenticated on purpose: the editor’s design-link fetch sends no
  credentials, and the URL must name the exact immutable file — the derived `.sc`
  artifact when one exists, otherwise the version’s own source file. Convertible CAD
  versions are restated to a native design first (editor role), reusing the
  “Convert to SolidCraft” job.
- **Saving back.** The app keeps its stored designs in the browser’s own storage,
  which the same-origin portal can read. **Save to PDM** asks the running app to
  write the open design to a throwaway file, reads the bytes back, and checks them
  in as a new immutable version — same base-version and lock rules as the other web
  editors (editor role; a changed vault head blocks the check-in instead of
  overwriting someone else’s work). **Download original / Download edited file**
  are available to every signed-in user.
- **Size note.** The editor caps a design-link download at the SolidCraft limit
  (256 MiB). On insecure origins the app’s storage is the browser’s local storage,
  so very large edited designs are best saved over HTTPS (see the web lab’s
  Tailscale endpoint).

The selected immutable vault version opens inside PDM. Documents are fetched
with the user's existing authentication and passed as bytes to a same-origin
iframe using the upstream `?host=parent` bridge. API tokens are not passed to
the editor. Files are processed in the browser.

## Saving

Use the editor's **Save** command to return the edited bytes to PDM. Then choose
**Download edited file** or **Check in edited version** in the surrounding PDM
toolbar. Check-in requires an editor/admin role, takes the existing exclusive
checkout, and creates a new immutable version. It uses the version actually
opened as its base; a changed vault head blocks check-in instead of overwriting
someone else's changes. Older versions can be opened and downloaded but cannot
be checked in over a newer head.

Save As, additional document tabs with other filenames, and CAD exports download
as separate files. They do not replace the selected vault item. Check-in failures
retain the edited bytes for retry or download. Closing the workspace or reloading
while changes are pending asks before discarding them. Pending bytes are held in
memory: download a copy before leaving. Viewing does not acquire a checkout.

## DXF preview (V.01-30)

The Preview tab renders the selected DXF version with the bundled CADCraft web
app, rather than showing DXF source text. Pan, zoom, and fit use CADCraft's own
tools inside the preview; image-only PDM zoom buttons are hidden. This works
without waiting for a server-generated thumbnail.

CADCraft's embedding protocol has no read-only viewer mode. Changes made inside
the preview are temporary and its Save command cannot write to the vault. Use
**Open in web editor** for the download/check-in workflow. Changing versions or
leaving the Preview tab disposes the embedded app and cancels pending file loads.

## Deployment

`docker/Dockerfile.dev` bundles the pinned upstream web releases automatically.
For a local server, install them into the directory served by `PDM_PORTAL_DIR`:

```sh
sh docker/install-web-editors.sh crates/pdm-api/static
```

The installer requires curl, unzip and sha256sum. The complete distributions,
including LICENSE, NOTICE and attribution files, are retained. The downloaded
builds are ignored by Git. They are served at `/static/editors/pdfcraft/` and
`/static/editors/cadcraft/`, on the same origin as the portal. No third-party
hosted editor or document upload service is used.

Pinned builds (MIT OR Apache-2.0):

| App | Version | Source |
|---|---|---|
| PdfCraft | 0.15.0 | https://github.com/storytold/pdfcraft/releases/tag/v0.15.0 |
| CADCraft | 0.14.0 | https://github.com/storytold/cadcraft/releases/tag/v0.14.0 |
| SolidCraft web | 0.40.1 (image pin) | https://github.com/tangency-labs/solidcraft/releases/tag/v0.40.1 (`solidcraft-web-0.40.1.zip`) |

PdfCraft and CADCraft are pinned for life of the image. The SolidCraft web
build additionally **auto-updates at runtime** like the bundled CAD CLI: the
server checks the newest release on GitHub (`releases/latest`, public) on boot
and every 6 h (`PDM_WEB_UPDATE_HOURS`), verifies the zip against the release's
`SHA256SUMS.txt`, and swaps it into the portal's `editors/solidcraft/` in
place (opt out: `PDM_WEB_UPDATE=0`; manual: System › “Check / reinstall web
editor”). Open editors pick up a new build on the next open. The installed and
latest tags are reported on `GET /settings/solidcraft-web` and in the System
panel.

The GPU web lab (`solidcraft-web-lab`) mirrors and smoke-tests the same
upstream builds; when PDM's runtime update lands on a release, the lab's
verified build of that release is the reference. If a release ever breaks the
portal's assumptions (`?open=` launch, `window.solidcraft` API, the
`solidcraft.file.` browser-storage names, `file.save_as`), the auto-update
keeps the last working build until the installer pin is raised.

The installer verifies each archive against its pinned SHA-256 checksum before
extraction. Updating an editor requires updating both version and checksum and
checking the upstream embedding protocol. A browser with WebGL2 support is
required; WebGPU may also be used by CADCraft. Missing builds or startup failures
are displayed in the editor workspace, with the original download still available.
