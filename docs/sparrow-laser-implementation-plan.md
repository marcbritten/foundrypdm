# Sparrow laser nesting — local AI implementation plan

> **Status: implemented.** The pipeline is live: `crates/pdm-nesting` (DXF
> ingestion, validation, Sparrow I/O, transforms, sheet export), migration 0021
> (revision-bound flat-pattern artifacts + `laser_nests`/`_parts`/`_sheets`/
> `_placements`), the `laser.nest` worker job, the `/laser-nests` API, the portal
> **Laser Nests** view + **Create laser nest** bulk action, and the pinned
> `sparrow` binary in the Docker images. Verified end-to-end against a real
> Postgres and the pinned engine: single-sheet, multi-sheet (30 → 16 + 14),
> stale-on-new-revision, cancel, and rerun. `docs/sparrow-compatibility.md` holds
> the verified engine contract and `docs/laser-nesting.md` the user/API docs.
>
> Units: the shop's drawings and stock are in inches but the laser needs mm.
> Nest inputs accept `"units": "in"` (stock/margin/gap converted ×25.4) and part
> source units resolve as per-part override → file `$INSUNITS` →
> `default_part_unit` → mm. Verified: a 2×1.5 in part with a 0.5 in hole nested
> on 48×96 in stock exported a `$INSUNITS=4` (mm) DXF with 50.8×38.1 mm parts and
> 6.35 mm holes.
> Remaining: machine-specific CAM import acceptance (needs the user's laser
> model / CAM software / stock / layer conventions).

## Goal and delivery contract

Implement a Foundry PDM workflow that takes selected parts and quantities, uses
Sparrow to nest their flat patterns on rectangular stock, and exports a correctly
scaled DXF for each sheet, with a preview and a traceable manufacturing manifest.

User workflow:

1. Select sheet-metal parts in Items or a project and choose **Create laser nest**.
2. Choose exact source versions, quantities, material/thickness, stock dimensions,
   edge margin, part spacing, allowed rotations, and optionally a laser machine.
3. Resolve missing flat patterns using existing generate/upload controls.
4. Run nesting asynchronously; see status and per-part validation errors.
5. Preview sheets and quantities; download individual DXFs or a ZIP containing
   all sheets and a manifest.
6. Reopen the saved nest later with the same source and output artifacts.

The exported DXF contains nominal part geometry for import into the laser's CAM
software. That software supplies kerf compensation, lead-ins, cutting sequence,
power/speed settings, and machine-specific output. Do not describe a geometric
nest as a generated NC program. Existing manufacturing records can link to the
package; never create a fake G-code/NC record for a DXF.

Deliver this in the phases below. Finish the single-sheet vertical slice before
adding the multi-sheet batching phase. Both are required for the complete plan.

## 1. Repository map: reuse these integration points

Observed in this checkout; recheck names and migration numbers before editing.

| Concern | Existing location |
| --- | --- |
| Workspace and shared dependencies; Rust 1.90, edition 2024 | `Cargo.toml` |
| Domain structs and errors | `crates/pdm-core/src/lib.rs` |
| SQL queries, org ownership checks, audit, job queue | `crates/pdm-db/src/lib.rs` |
| Additive SQL migrations and explicit registration | `crates/pdm-db/migrations/`, `crates/pdm-db/src/migrate.rs` |
| Sheet-metal specs and flat-pattern pointer | `0008_printing_sheetmetal.sql`; `get_item_sheet_metal`, `set_item_flat_pattern`, `list_flat_patterns` in DB |
| Material/thickness standards | `0009_sheet_material_standards.sql` and associated DB/API/UI |
| File version numbering | `0019_version_revisions.sql`; pin `file_versions.id`, not a display revision alone |
| Machine registry and manufacturing chain | migrations `0005`–`0007`, `docs/manufacturing.md` |
| API and router | `crates/pdm-api/src/routes.rs` (`api_router`) |
| Existing flat-pattern API | `/items/{id}/sheet-metal`, `/items/{id}/flat-pattern`, `/items/{id}/flat-pattern/generate`, `/sheet-metal/flat-patterns?items=...` |
| Background execution | `crates/pdm-worker/src/lib.rs`: `WorkerConfig`, `run_job_kind`, `generate_flat_pattern` |
| CAD flat-pattern generation | existing `sheet.flat-pattern` job invoking `sheet.export_dxf` |
| Content-addressed storage | `crates/pdm-storage/src/lib.rs`, `BlobStore`; DB `upload_blob` / `get_blob_by_sha` |
| Web portal | `crates/pdm-api/static/app.js`, `style.css`, `index.html` |
| Existing bulk export / spec UI | `exportFlatPatterns`-related code and sheet-metal tab in `app.js` |
| Existing DXF preview | `crates/pdm-api/static/editors.js` (CADCraft integration) |
| Deployment | `compose.yaml`, `docker/Dockerfile.standalone`, other applicable Dockerfiles |
| Existing browser tests | `crates/pdm-api/tests/web-ui.test.cjs` |

There are pre-existing working-tree changes in `CHANGELOG.md`, `app.js`, and
`style.css` when this plan was written. Inspect and preserve local work.

## 2. Upstream facts and first implementation task

Sources inspected:

- https://github.com/JeroenGar/sparrow
- https://github.com/JeroenGar/sparrow/blob/main/Cargo.toml
- https://github.com/JeroenGar/sparrow/blob/main/cli/Cargo.toml
- https://github.com/JeroenGar/sparrow/blob/main/cli/src/cli.rs
- https://github.com/JeroenGar/jagua-rs

Important findings:

- Sparrow is a Rust **strip-packing optimizer**: fixed strip height, minimized
  strip length. It is not by itself a fixed-sheet, multi-bin scheduler.
- The inspected CLI reads instance JSON and writes final JSON and SVG under
  `output/final_{instance_name}.*` relative to its working directory. It does not
  provide the PDM's required DXF import/export pipeline.
- Inspected source has a separate `sparrow-cli` package and `sparrow` binary.
  CLI features include `only_final_svg`, and flags include input, time, seed,
  and worker count. Recheck flags with the pinned binary's `--help`.
- Upstream documentation and source currently disagree: README mentions
  `jagua-rs` 1.0.0 while the inspected Cargo manifest pins `jagua-rs =2.0.0`.
  Do not mix schemas or use a moving `main` dependency.
- README documents explicit `orientation.rotation` and rejection of item holes.
  Verify the selected revision's actual schema and transformation semantics.
  Preserve holes in PDM geometry independently of the nesting engine.
- Sparrow is MIT; jagua-rs identifies as MPL-2.0. Include applicable upstream
  notices/source information in distributed builds.

**Task 0 — compatibility spike (required before the main implementation):**

1. Select and record an exact upstream commit, its dependency versions, and a
   reproducible build recipe. Keep its lockfile/build inputs pinned.
2. Build on stable Rust supported by the project, without SIMD/nightly or
   host-specific CPU flags in portable Docker images. For the inspected source,
   the starting build command is:
   `cargo build --release -p sparrow-cli --bin sparrow --features only_final_svg`.
3. Run two asymmetric sample shapes with different quantities and restricted
   rotations. Save actual input/output JSON as compatibility fixtures.
4. Establish item ID mapping, multiplicity, coordinate axes, rotation units,
   reflection fields, and any normalization/recentering performed by the importer.
   Derive the complete source-DXF-to-placement transform; check it against SVG.
5. Confirm minimum-separation behavior, output feasibility, process exit behavior,
   time-limit behavior, and final result naming.
6. Write the findings in `docs/sparrow-compatibility.md`, including the chosen
   commit. The later adapter must target these fixtures.

## 3. Architecture

Use a worker-invoked, pinned Sparrow executable for the initial integration.
This fits the existing external CAD-tool pattern, allows hard timeout/cancellation,
and isolates optimizer CPU work from the async API runtime.

Add `crates/pdm-nesting` for reusable geometry, validation, versioned solver I/O,
placement transforms, and DXF export. Add its shared dependency to the workspace.
Keep HTTP and SQL orchestration in the existing API/DB/worker crates.

Pipeline:

```text
selected exact file versions + quantities
  -> revision-bound flat-pattern artifacts
  -> parse DXF and normalize to mm
  -> retain nominal curves + classify cut/mark entities
  -> conservative outer polygons for nesting
  -> Sparrow JSON -> bounded worker subprocess -> final placement JSON
  -> independent geometry/quantity/bounds validation
  -> transform original nominal curves
  -> per-sheet DXF + preview + manifest -> content-addressed blobs
```

Do not convert Sparrow's visualization SVG into the manufacturing DXF. Its
collision polygons and drawing decorations are not the original cut geometry.

## 4. Data model and source integrity

Add migrations at the next free numbers (currently after `0020`) and register
them in `migrate.rs`. Use org-scoped references and indexes throughout.

### Revision-bound flat-pattern artifacts

The current `sheet_metal_specs.flat_pattern_sha` is a mutable per-item pointer;
its presence is not proof that the DXF belongs to the requested file version.

Add immutable artifact records with:

- ID, org ID, item ID, source `file_version_id`, DXF blob SHA/size;
- source kind (`generated`, `uploaded`, or direct DXF version), creation time;
- generator identity/version and generation settings when applicable;
- unit override, source-unit metadata, and geometry parser version where needed.

Update generation and upload paths to record an explicit source version. Validate
that the version belongs to the item and organization. Keep the current pointer
for existing UI compatibility, but resolve nests through the immutable artifacts.
Do not silently assign legacy unbound attachments to today's CAD revision: require
explicit binding or regeneration. Direct DXF items can use their version blob.

### Nest records

Suggested tables (names can follow repository conventions):

- `laser_nests`: org/project/creator, name, status, optional machine, material and
  thickness snapshot, stock width/height, margins, spacing, unit convention,
  tolerances, bounded runtime, seed, engine/build/schema identity, attempt/job ID,
  timestamps, error, manifest SHA, stale-source indicator.
- `laser_nest_parts`: nest + org, source item/version/artifact IDs and DXF SHA,
  part number and revision snapshots, quantity, allowed rotations, layer mapping,
  unit override and material/thickness snapshot.
- `laser_nest_sheets`: nest + org, sheet index and dimensions, DXF/preview SHAs,
  solver input/output SHAs, utilization metrics.
- `laser_nest_placements`: sheet/part + org, unique copy index, rigid transform,
  including source normalization transform or a fully composed source transform.

Use relational constraints for positive quantities, valid states, unique copy
indices, and tenant-consistent relationships. Check finite positive dimensions,
thickness and tolerances in application validation too. Store inputs immutably
when queued. Reruns create a new run/record with a parent reference.

Suggested states: `queued`, `running`, `completed`, `failed`, `cancelled`.
Use a separate stale flag/reason rather than rewriting a completed result.

Creating the record, snapshotting inputs, and enqueueing the background job must
be atomic. Validate all cross-record ownership; a blob hash alone is not access
authorization. Once completed, input snapshots and manufacturing outputs remain
immutable. A new CAD version or replacement flat pattern marks dependent nests
stale while keeping historical artifacts downloadable and unchanged. Recheck
staleness at completion to cover changes made while a job was running.

## 5. Geometry contract

Implement and test this before connecting the optimizer to production uploads.

### DXF ingestion

- Evaluate a maintained Rust DXF reader/writer and robust polygon operations
  library in Task 0; record selected versions and reasons before adding them.
- Initial supported geometry: planar LINE, ARC, CIRCLE, LWPOLYLINE (including
  bulges), and supported 2D POLYLINE entities. Reject unsupported cutting entities
  with entity/layer details. Add SPLINE/ELLIPSE only with tested approximation.
- Resolve BLOCK/INSERT transforms correctly or reject them explicitly in v1;
  never silently drop inserted geometry. Reject non-planar cutting geometry.
- Read DXF units and convert to millimeters. Unitless or ambiguous input requires
  a saved user override; never infer inches/mm from bounding-box size.
- Join endpoints with a documented mm tolerance; reject open cutting contours,
  self-intersections, zero-area loops, duplicates, NaN/infinite coordinates, and
  ambiguous topology. Avoid silent repairs that change manufactured dimensions.
- Distinguish cut, mark/etch, bend/reference, and ignored layers through explicit
  mapping. Open lines are allowed only for mapped marking/reference operations.
  Unknown layers require mapping; never turn bend lines into cuts by default.
- Require one connected outer part boundary per part in v1; preserve all its
  interior cutouts. Multiple disconnected outer islands need explicit handling.
- Produce actionable errors identifying part, version, layer, and entity.

### Solver geometry versus exported geometry

- Tessellate curves for collision calculations with a specified maximum chord
  error. Make solver envelopes conservative: an inscribed approximation of a
  circle/arc can otherwise allow overlapping original curves. Include a proven
  approximation allowance and independently validate nominal geometry distances.
- Send only supported outer polygons to Sparrow; preserve holes and marks in
  the nominal geometry. Do not place parts inside holes in this implementation.
- Default allowed rotations to `[0, 90, 180, 270]`, editable per part. Grain-bound
  parts can use `[0, 180]` or `[0]`. Disable reflection/mirroring explicitly.
- Define `part_spacing_mm` as the minimum distance between nominal outer part
  boundaries. Do not apply the requested gap twice using both offsets and solver
  separation. Record any additional numerical/approximation allowance separately.
- Edge margin is measured from nominal outer geometry to the stock boundary.
- Export nominal curves without kerf offsets. Optional kerf metadata must not
  silently alter dimensions; CAM applies compensation once.
- Apply the full rigid transform to outer contours, holes, and marking entities.
  Preserve arcs/circles/bulges wherever supported; never export the coarse nesting
  polygon as the cut shape. Output at Z=0 in mm with `$INSUNITS=4`.

Before accepting results, independently verify every requested copy occurs once,
all transforms are finite and allowed, all nominal geometry fits within margins,
and pairwise spacing is satisfied. Reject unknown/duplicate solver item IDs.
Do not rely on SVG appearance or release-build assertions for feasibility.

## 6. Fixed stock and multi-sheet algorithm

### Phase A: single sheet

For stock width W, height H, and uniform margin M, use strip height `H - 2M`
and allow only placements whose used strip length is at most `W - 2M`.
Translate accepted placements by `(M, M)` into stock coordinates after composing
the upstream normalization transform. Confirm axis mapping in Task 0.

Precheck that each part can fit the usable rectangle in at least one allowed
orientation. A time-limited heuristic failing to fit a batch means **no fitting
layout found within this run**, not proof of mathematical impossibility.

### Phase B: multiple identical sheets

Implement a bounded batching heuristic around Sparrow:

1. Expand demand into uniquely identified copies; enforce a total-copy limit.
2. Sort copies by descending conservative area, breaking ties deterministically.
3. For each sheet, incrementally try adding a remaining copy to the accepted
   subset. Solve the candidate subset and accept it only if validated within W/H.
4. Retain the last accepted feasible layout if a larger candidate fails to fit.
   Continue trying smaller remaining copies instead of discarding them all.
5. Finalize the sheet, remove only its accepted copies, and repeat.
6. Enforce one global wall-clock budget across all attempts, per-attempt limits,
   maximum sheets, and progress checks. Stop with a clear incomplete/failure
   result if copies remain; never label partial demand as completed.

Cache accepted candidates and optionally warm-start only after Task 0 validates
the format. Do not split an unrestricted long-strip solution at arbitrary sheet
boundaries: that can slice parts in half. This batching method yields feasible
sheets; it does not guarantee the minimum sheet count.

Compute per-sheet material utilization using nominal outer area minus holes
divided by full stock area. Label this separately from strip-length utilization
and conservative solver-envelope utilization.

## 7. Background worker and deployment

- Add `laser.nest` to `run_job_kind`. Add explicit handling; the worker currently
  treats unknown job kinds as successful no-ops.
- Add `PDM_SPARROW_CLI` and documented limits for runtime, solver threads,
  concurrent nesting jobs, copies, entities/vertices, sheets, and artifact sizes.
- Use a fresh temporary working directory per attempt. Generate a safe internal
  instance name from UUIDs; never use user text in solver output paths.
- Execute with `tokio::process::Command` and argument arrays, without a shell.
  Bound logs/output, set hard wall-clock timeout, kill and reap on timeout or
  cancellation, and prevent a killed attempt from publishing results.
- Run CPU-heavy parsing and geometry checks outside async executor threads.
- Fail explicitly if Sparrow is unavailable. An absent binary must not mark a
  nesting request successfully completed or silently substitute a fake layout.
- Inspect existing queue claim/retry/restart behavior. Add attempt identity and
  recovery for interrupted nests; use conditional state transitions so duplicate
  execution cannot publish competing sheet sets or regress a cancelled run.
- Persist all output blob metadata and finalize sheet/placement rows and status
  transactionally after validation. Failed attempts must not expose partial final
  packages. A retry uses the same immutable source snapshots.
- Bundle the pinned binary and required runtime dependencies in the standalone
  Docker image as well as applicable advanced/worker images. Include build
  identity and notices; keep the runtime usable offline after image creation.
- If images are multi-platform, build Sparrow for each target architecture.

## 8. API contract

Follow current auth, role, JSON response, audit, and error conventions. Proposed
routes (the API's existing prefix conventions apply):

| Method/path | Purpose |
| --- | --- |
| `POST /laser-nests/validate` | Preflight exact sources, layer/unit mapping, stock and compatibility |
| `POST /laser-nests` | Snapshot inputs, create and queue; return HTTP 202 with nest/job IDs |
| `GET /laser-nests?project_id=...` | Paginated org-scoped history |
| `GET /laser-nests/{id}` | Settings, source snapshots, status, errors, sheets, stale reasons |
| `POST /laser-nests/{id}/cancel` | Cancel pending/running work |
| `POST /laser-nests/{id}/rerun` | New linked run from explicit settings/source selection |
| `GET /laser-nests/{id}/sheets/{index}/dxf` | Download validated sheet geometry |
| `GET /laser-nests/{id}/sheets/{index}/preview` | Preview from validated placements |
| `GET /laser-nests/{id}/package` | ZIP of DXFs, preview(s), and manifest |

Creation input includes part item/version/artifact IDs, quantity, allowed
rotations, unit/layer mapping, stock W/H/margin, part spacing, material/thickness,
optional project/machine ID, bounded time limit, and optional seed. The server
resolves and verifies blob hashes rather than trusting arbitrary client hashes.

Use the existing viewer/editor authorization split: viewers can read authorized
nests; editors can create/cancel/rerun. Verify machine kind/envelope when selected.
Reject mixed materials or thicknesses within a run with a clear error; offer the
UI separate runs per compatible group. Do not silently rename materials to make
them match. Link source metadata overrides explicitly if existing data is missing.

Return structured preflight errors, not only a single concatenated string.
Revalidate source identity and settings when creating the run; preflight alone
does not eliminate races. Audit creation, cancellation, rerun, and export using
existing conventions.

Manifest must include schema version, nest/sheet IDs, timestamps, material and
stock settings, source item/version/artifact/hash snapshots, quantities, complete
placement transforms, solver build/schema/seed/thread/time settings, geometry
tolerances, layer mapping, output hashes, and stale-source information. Record
seed/settings for provenance; do not promise bitwise repeatability from a
time-limited parallel heuristic. Stored outputs provide exact historical replay.

## 9. Portal experience

- Add **Create laser nest** beside existing bulk flat-pattern export and a
  **Laser nests** manufacturing history view. Follow the vanilla JS UI patterns.
- Show selected part number, V/R label, quantity, material, thickness, and
  flat-pattern availability before running. Resolve missing generation with the
  existing asynchronous job UI, then require valid revision-bound artifacts.
- Provide stock presets plus custom dimensions, margin/spacing, rotations and
  unit/layer mapping. Use machine envelope as a selectable default, not as an
  assumed usable sheet size.
- Show job status and failures; poll using existing patterns and stop polling
  when the view is disposed. Do not invent exact percentages if unavailable.
- Preview all sheets with pan/zoom, dimensions, utilization, part selection, and
  per-copy labels. Render labels/borders as preview overlays, not cutting lines.
- Keep stock rectangles, sheet numbers, and part labels out of cutting layers.
  Export marking geometry only when mapped/requested; laser CAM may treat every
  imported entity as a cut unless configured otherwise.
- Provide per-sheet DXF and whole-package downloads, source links, saved settings,
  and clear outdated-source badges. Generate previews from validated geometry;
  never inject arbitrary untrusted SVG markup into the page.

## 10. Ordered implementation tasks and acceptance gates

1. **Compatibility spike:** pin/build Sparrow; capture real schema and asymmetric
   transform fixtures; document verified flags and dependency versions.
2. **Geometry core:** add `pdm-nesting`, mm-normalized ingestion, topology/layer
   validation, conservative solver outlines, nominal-curve transforms and writer.
   Gate: round-trip dimensions, circles, holes and marks remain correct.
3. **Revision-bound artifacts and nest persistence:** migrations/models/queries;
   update upload/generation provenance; transactional queueing; org isolation.
   Gate: replacing the current flat pattern cannot change a saved run's inputs.
4. **Single-sheet worker/API vertical slice:** bounded subprocess, parse final
   JSON, independent validation, persist DXF/preview/manifest, download endpoints.
   Gate: two asymmetric real parts nest and exported DXF reopens at correct scale.
5. **Multi-sheet completion:** bounded batching, quantity reconciliation, timeout
   and cancellation/recovery. Gate: a batch requiring two sheets exports exactly
   the requested quantities with no boundary-crossing parts.
6. **Portal:** create dialog, preflight correction flow, progress/history,
   per-sheet preview/download, stale-source indication. Gate: complete workflow
   from selected existing PDM parts without using raw API calls.
7. **Deployment/docs:** bundle pinned engine, describe configuration, update
   manufacturing/API documentation and changelog while preserving local edits.
   Gate: Docker deployment can nest uploaded flat DXFs without the CAD CLI.
   Generating a flat pattern from a CAD model still uses the existing CAD tooling.

For each task, report changed files, commands run, results, and remaining blockers.
Do not mark an acceptance gate passed using stub solver output.

## 11. Verification matrix

Meaningful automated coverage:

- mm and inch inputs; required unitless override; nonzero/negative source origins;
  asymmetric rotated parts; circles, bulge arcs and off-center holes; marking
  transforms; layer mapping; open/self-intersecting/non-planar/unsupported input.
- Conservative curve envelopes, exact stock/margin fit and just-outside failures,
  concave geometry spacing, restricted rotation, mirror rejection, duplicate IDs.
- Multiple copies, multi-sheet overflow, oversized parts, mixed-material rejection,
  no-progress batching and partial-demand failure.
- Malformed/missing final solver files, nonzero exit, timeout/cancel, concurrency,
  duplicate attempt publication, restart recovery and unavailable binary.
- Cross-org item/version/artifact/machine/nest/blob access; immutable source
  snapshots; updates during execution; stale detection without historical mutation.
- API creation/status/download and UI preflight/error/quantity/history behavior.

Keep fast geometry/schema fixtures deterministic. Add a separate real-Sparrow
integration test and assert feasibility/quantity/scale rather than an exact layout
or runtime-sensitive utilization score. Mark environment-dependent DB/solver
tests clearly and ensure at least one configured run executes them.

Repository checks after implementation:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
node --test crates/pdm-api/tests/web-ui.test.cjs
```

Also execute the documented real-engine integration test, database migration tests,
and Docker build/smoke workflow. Capture the exact commands in the implementation
report; do not claim unavailable-environment checks passed.

Final real-world acceptance:

1. Upload/generate two actual flat-pattern DXFs, including a part with holes/arcs.
2. Nest several copies on the user's actual sheet size and material.
3. Import the downloaded DXF into the user's laser CAM software; verify a known
   dimension, holes, layers, orientation, and total copy count. Repeat for a
   two-sheet package.
4. Update a source version, confirm the old nest is flagged stale, and verify its
   downloaded artifacts are unchanged.

Before the CAM import acceptance step, ask the user for laser model/controller,
CAM application and supported DXF version, typical stock sizes, preferred origin,
layer conventions, and spacing/grain requirements. Implementation can proceed
with configurable defaults; machine-specific compatibility remains an explicit
acceptance item until tested with that software.

## Copy/paste instruction for the local AI

> Implement `docs/sparrow-laser-implementation-plan.md` in order, starting with
> Task 0's pinned-upstream compatibility spike. Inspect existing code and local
> changes first. Reuse PDM authentication, jobs, blob storage, sheet-metal inputs,
> machine records, and portal patterns. Complete and verify each acceptance gate
> before moving on. Produce real nested DXF sheets from original nominal curves,
> preserve exact source-version provenance, and validate quantities/geometry
> independently of the solver. Report changed files and actual verification
> results after each phase. Ask for the laser/CAM details needed for the final
> import test while continuing the machine-independent implementation.
