# Changelog

Foundry PDM server. Dates are UTC.

## V.01-77 — sheet metal, laser nesting and tool library

### Added
- **Tool library editor** (sidebar › **Tool library**): tools are now full
  records with four tabs, mirroring the CAM tool editor — **General**
  (description, vendor, part number, link, and attached documents uploaded as
  content-addressed blobs with download/remove), **Cutter** (type of tool,
  mm/in units, CW/CCW rotation, flutes/cutting edges, cutter material, and
  per-type geometry stored in the tool's units), **Cutting data** (any number
  of speed/feedrate sets, each scoped to a material and finish: spindle/surface/
  ramp speeds, cutting/lead/transition/ramp/plunge feedrates, feed per tooth,
  ramp angle, plunge feed per revolution) and **Post processor** (tool number
  in machine). New API: `GET/PUT/DELETE /tools/{id}`,
  `GET/POST /tools/{id}/cutting-data`, `PUT/DELETE /tools/{id}/cutting-data/{cid}`,
  `GET/POST /tools/{id}/documents`, `DELETE /tools/{id}/documents/{doc_id}` and
  `GET /tools/{id}/documents/{doc_id}/download` (schema migration 0020).
- **Sheet-metal parts, flat patterns and bulk DXF export**: an item with a
  sheet-metal spec (item details › **Sheet metal** tab — material, thickness,
  bend radius, K-factor) is now distinguishable from ordinary 3D models: item
  rows carry an **SM** badge (with a ▸ when a flat pattern is attached), and
  the Items view has **SHEET METAL / Not sheet metal** filter chips
  (`GET /items?sheet_metal=true|false`, likewise on
  `GET /projects/{id}/items` and `GET /search`). The item detail tab also
  attaches, replaces and downloads a part's generated flat-pattern DXF
  (`POST/GET /items/{id}/flat-pattern`, raw bytes, content-addressed like
  every other upload; `GET/POST /items/{id}/sheet-metal` for the spec).
  Select several items in the Items or project-Explorer views and use the
  bulk action **Export flat patterns** — `GET
  /sheet-metal/flat-patterns?items=id1,id2` returns a ZIP with one DXF per
  part (`<part-number>-flat.dxf`) plus a `manifest.json` listing included
  parts and the ones missing a pattern (up to 200 items per export).
  Item tables also carry a **Type** column (Sheet metal / Solid / —) so
  3D models and sheet-metal parts are distinguishable at a glance.
- **Automatic sheet-metal detection and flat-pattern generation**: check-in
  analysis (`design.analyze`) now inspects the design's feature timeline; a
  design built with sheet-metal features (flanges, hems, folds,
  convert-to-sheet-metal, …) is flagged as a sheet-metal part on its own
  (its spec is never overwritten) and queues a `sheet.flat-pattern` job that
  runs the engine's `sheet.export_dxf` (`solidcraft-cli run <script>
  --in <design>`) and attaches the resulting DXF. The head version's native
  `.sc` design is used directly; imported CAD uses its converted `.sc`
  artifact. The same job is triggered on demand from the item's Sheet metal
  tab (**Generate from 3D model**, `POST
  /items/{id}/flat-pattern/generate`).
- **Portal static assets are revalidated on every load**
  (`Cache-Control: no-cache` on `/` and `/static/*`), so a redeployed portal
  no longer stays hidden behind a stale browser cache.
- **New part in SolidCraft web** (project explorer, editor role): **✎ New part in
  SolidCraft web** opens a blank SolidCraft web session; model the part and
  **Save to PDM** stores it as a new item — description, classification, folder
  and part number (blank = auto-assign from the org's part-number prefix, e.g.
  `BE-000116`) — and checks the design in as the first immutable version.
  Further saves check in new versions of that item; a cancelled save keeps the
  design available to download or retry.
- **Onshape import via Dutchman**: project Settings → "Import from Onshape
  (Dutchman)" fetches a Part Studio with the org's Onshape API keys through
  `dutchman onshape fetch` (worker job `onshape.import`), checks the
  `.onshape-bundle` in as a version and runs the SolidCraft conversion. The
  referenced Dutchman build must carry the Onshape reader.
- **Dutchman release channels + conversion settings** (System › Dutchman):
  a per-org channel dropdown (latest stable, nightly, the onshape-preview
  builds, pinned tags; `GET/PUT /settings/dutchman`,
  `POST /settings/dutchman/update`) and fields for preset, part positions,
  history verification, conversion timeout and the Onshape call budget. The
  update loop (boot + hourly + on demand) now chases the referenced release,
  tracked in a `.info` file beside the binary (`PDM_DUTCHMAN_CHANNEL`
  overrides the host loop).
- **Format badges show the real extension** (F3D, STEP, STL, FCSTD, ONSHAPE, …)
  instead of a generic "CAD".
- **Dutchman for feature-tree conversion**: `Convert to SolidCraft`
  (`cad.solidcraft`) now routes native CAD — Fusion (`.f3d`/`.f3z`), Inventor
  (`.ipt`/`.iam`), SolidWorks (`.sldprt`/`.sldasm`), FreeCAD (`.FCStd`), CATIA
  (`.CATPart`), Rhino (`.3dm`), Creo View (`.pvz`), ACIS (`.sat`/`.sab`/`.smt`/`.smb`)
  and STEP — through [Dutchman
  Convert](https://github.com/dutchman-cad/dutchman), which decodes the native
  file into a SolidCraft design **with editable feature history where it maps**
  (verified against the PDM's own `solidcraft-cli`). Meshes and IGES keep the
  SolidCraft-CLI path; if Dutchman is missing, formats the CLI also reads fall
  back to it. The PDM keeps the host on the referenced Dutchman release: the
  image bakes the newest stable in at build (`docker/install-dutchman.sh`), the
  server re-checks GitHub on boot and hourly (`PDM_DUTCHMAN_UPDATE_HOURS`), and
  a conversion job pulls the referenced release on demand when the binary is
  missing or from another channel
   (`PDM_DUTCHMAN_UPDATE=0` opts out; `PDM_DUTCHMAN_CLI` names the binary,
   default `/usr/local/bin/dutchman`; `PDM_DUTCHMAN_TIMEOUT_SECS` default 1800).
   System › **Dutchman** reports installed vs latest with a check/update button
   (`GET/POST /settings/dutchman[/update]`).
 - **Laser nesting (Sparrow)**: sheet-metal flat patterns are now nested onto
   rectangular stock with the pinned
   [Sparrow](https://github.com/JeroenGar/sparrow) engine and exported as
   per-sheet DXF for the laser's CAM import. **Laser Nests** (sidebar) has a
   **New nest** create panel that lists the org's sheet-metal parts to add, with
   sheet information (length × width, **grain direction** along length/width,
   edge margin, separate **horizontal / vertical part spacing**) and per-part
   lines (quantity, **mirror original**, **flipping allowed**, **part grain**).
   The same part can be added on multiple lines with different options. You can
   also select items in **Items** and use the **Create laser nest** bulk action,
   or `POST /laser-nests` / `POST /laser-nests/validate`. A `laser.nest` worker
   job pins the exact CAD version + flat-pattern blob per part (immutability),
   runs Sparrow as a bounded subprocess, splits demand across identical sheets,
   independently validates each sheet (bounds, spacing, quantity), and stores
   per-sheet DXF + SVG preview (with a grain indicator) + a manifest. Sheet
   grain + part grain drive rotation restrictions so part grain aligns with the
   sheet grain; spacing is enforced directionally by dilating each part's
   collision outline by the spacing box. **Laser Nests** lists runs with live
   status, sheet previews, stale-source badges, per-sheet DXF/preview download,
   re-run, cancel, and a whole-package ZIP
   (`GET /laser-nests/{id}/sheets/{i}/dxf|preview`,
   `GET /laser-nests/{id}/package`, `POST …/{id}/cancel|rerun`). A superseded
   source CAD version flags a nest **stale** without rewriting it. The pinned
    `sparrow` binary is bundled into the Docker images (`PDM_SPARROW_CLI`); see
    `docs/laser-nesting.md`, `docs/sparrow-compatibility.md`, and
    `docs/sparrow-laser-implementation-plan.md`. New schema: migrations 0021–0023
    (`flat_pattern_artifacts`, `laser_nests`, `laser_nest_parts`,
    `laser_nest_sheets`, `laser_nest_placements`, plus directional spacing,
    grain, and per-line mirror/ordinal columns).
  - **Laser-nest create panel, professional-style**: the create flow is now a
    **Sheets | Parts** tab pair like a nesting workstation. *Sheets*: a
    sheet list that **adds/removes sizes of differing dimensions**, each with
    its own quantity ("as many as needed" or a fixed count) — the worker works
    through the definitions in order and carries parts that don't fit one size
    onto the later, larger ones; plus **units of measure** (inches/millimetres,
    labelled on every dimensional field), grain direction, **nesting corner**
    (the fill starts at the chosen corner), origin X/Y, sheet–sheet distance,
    **guillotine cut**, and automatic sheet origins. *Parts*: a **searchable
    part picker** with flat-pattern SVG thumbnails (falling back to the
    rendered 3D thumbnail, via `GET /items/{id}/flat-pattern/preview`) and
    checkbox multi-select + **Add selected**; each part line can be **removed
    individually** and carries quantity, minimum/filler quantity, **step angle**
    (90/45/30/180° rotation increments), priority, label, grain, a three-way
    **Mirror** (Original Only / Both / Mirror Only), and use-bounding-box /
    common-edge flags; shared part–part spacing (horizontal/vertical),
    **add part labels** (drawn on the sheet DXF + preview), optimized pairs and
    fill-all-sheets. Sheet/part choices pre-fill from new **org laser-nesting
    defaults** in **Settings** ("Laser nesting defaults" — units, sheet size,
    margin, spacing, grain, corner, guillotine, labels, part source unit;
     `GET/PUT /settings/nest-defaults`). Migrations 0024–0025 add the
     sheet/part option columns and `laser_nests.sheet_defs`.
  - **The sheet DXF records the checked part gap**: each sheet DXF leads with a
    DXF comment (group 999, which readers skip) — `FoundryPDM nest:
    part_gap_mm=…` — the least distance between parts the nest's spacing check
    proved (the requested spacing less rounding and twice the curve allowance,
    rounded down to 0.001 mm), so CAM downstream can check a tool against it:
    on a router, a cut-out bit wider than the gap cuts into the next part. A
    layout stored before this change carries no record.
  - **Nest layouts sit in the chosen corner on both axes**: Sparrow shortens
    the strip lengthwise only, so a layout could start at the chosen corner's
    side but float between the sheet's top and bottom edges (leaving the
    off-cut in two pieces). After each solve the whole layout is pressed down
    the strip until its lowest part has the solver's own edge clearance, so the
    parts sit in the corner on both axes; the move is uniform, so part spacing
    is unchanged, and if the moved layout failed validation the solver's own is
    kept.
  - **Optional router layers** (`PDM_ROUTER_LAYERS=1`, off by default): a
    generated flat pattern is put on router layers with every depth taken from
    the SolidCraft model — `OUTLINE`, `THROUGH` (cut-outs) and `DEPTH_<mm>`
    (pockets open on the top face) — and the nest keeps each contour's layer
    through to the sheet DXF, so CAM can tell a pocket from a hole and won't
    cut the pocket through. A blind feature leaves material under it, so it
    doesn't come off the part's area; a part with pockets is never nested
    mirrored (a *Mirror Only* line for it is refused), and a pocket that opens
    on the bottom face or inside the material is refused by name. Uploaded flat
    patterns are not renamed.

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
- **Versions and revisions**: a check-in is now a new **revision** of the current
  version by default (V1 - R1, V1 - R2, …); the major version only advances when
  the user asks for it. `POST /items/{id}/checkin` accepts `new_version: true`
  to start the next major version (V2 - R1). The portal offers this as a
  "new version" checkbox next to Check in; version history, item listings and
  the checkout banner show `V{version} - R{revision}`. `seq` remains the global
  per-save counter (stale-base check-ins, head selection and derived filenames
  are unchanged). Existing saves keep their old `seq` as their version number
  (migration 0019 backfills `version = seq`, `revision = 1`). NC programs are
  now flagged `outdated` only when a new major version is checked in —
  revisions within a version stay compatible with the same manufacturing
  chain. `ItemSummary` gains `head_version` / `head_revision`.
- Bundled CAD CLI tracks GitHub latest (0.29.0 as of this entry) instead of a frozen image-only 0.27.1.

## Unreleased

## V.01-39 — edit in SolidCraft web

- The Preview tab’s **✎ Edit in SolidCraft web** button (under the 3D model) opens
  the selected version in the SolidCraft CAD app: the upstream web release is
  pinned into the image (same installer as the other web editors) and runs in a
  same-origin iframe. Convertible CAD versions are restated to a native `.sc`
  design first, on demand.
- New unauthenticated `GET /public/design/{version}/{filename}`: the URL names the
  exact immutable file (the derived `.sc` artifact when one exists, otherwise the
  version’s source file), so the editor’s design-link fetch needs no token.
- **Save to PDM** in the editor collects the edited design from the app's browser
  storage and checks it in as a new immutable version (editor role; same base-version
  and lock rules as the other web editors). **Download original / Download edited
  file** are offered alongside.

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
