# PDM worker job spec — Increments 3–5

**Scope:** `pdm-worker` job kinds `bom.extract`, `preview.model`, `release.package`, `duplicate.search`.
**Status:** planning. No code committed. `cargo xtask ci` must pass before any PR.
**Source of truth:** `pdm-web-plan.md` §1.4, §5 (P1-9/P1-12/P2-20), §6 (Increment 3); `pdm-worker/src/lib.rs`; `pdm-db/migrations/0001_init.sql`; `apps/solidcraft-cli` (the binary PDM calls `solvecraft-cli` via `PDM_SOLVECRAFT_CLI`).

---

## 0. Conventions the new jobs must follow (from `pdm-worker/src/lib.rs`)

- `WorkerConfig.solve_cli` = `PDM_SOLVECRAFT_CLI` env var; when `None`, CAD jobs are marked done with a `skipped` note — never fail the worker.
- Job payload carries `version_id` (and `item_id` where needed) as string UUIDs; resolve via `version_id_from_job` / `item_id_from_job` helpers.
- `run_job_kind` dispatches on `job.kind`; **unknown kinds → `Outcome::Done`** so enqueued follow-up work cannot jam the queue.
- On success: `record_job_attempt(ok=true)` + `complete_job(id, true, "")`, then persist outcome. On failure: `record_job_attempt(ok=false)`, retry to `queued` + `run_after = now() + 30s` while `attempts + 1 < max_attempts`, else `complete_job(id, false, err)` + `status='dead'`.
- `CliFailure::Fatal` = CLI ran and refused the file / wrote nothing → record a reason on the version, complete the job, no retry. `CliFailure::Transient` = CLI could not run → normal retry applies. Error line capped at 300 chars (`cli_error_line`).
- Blobs are content-addressed via `store.put_content_addressed(bytes)` → `(sha256, size)`; temp files under `std::env::temp_dir()` named `pdm-job-{job-id}.*`, always removed.
- All non-test Rust: `clippy::unwrap_used/expect_used/panic/unimplemented/todo/unreachable` denied; no `unsafe`; errors are `Result`.

## 1. solvecraft-cli surface confirmation

| Capability | solidcraft-cli subcommand / engine command | Status | Notes |
|---|---|---|---|
| measurements JSON | `eval <design> [--json-out FILE]` (runs `inspect.measure` + `document.inspect`) | EXISTS | body_count, bodies, total, timeline, params, sketches |
| PNG thumbnail | `snapshot <design> --out <png> [--width] [--height] [--view iso]` | EXISTS | used by `preview.render` |
| B-rep → STEP | `convert <step|igs|fcstd|3dm|ipt|iam> --out <step>` | EXISTS | reads B-rep CAD only, NOT `.solidcraft` designs; used by `cad.convert` |
| mesh/STEP export of a design | `run --in <design> --out <file.obj|step|stl|3mf|urdf|json>` (`file.export`) | EXISTS | `file.export` formats: `stl\|stla\|obj\|step\|3mf\|3mf-tagged\|urdf\|json` |
| **glTF export** | none | **MISSING** | glTF is import-only (`.glb/.gltf` open as components); no export writer |
| **BOM/structure dump** | `assembly.bom` engine command EXISTS (rows: item, part_number, description, revision, count, children, properties; CSV/JSON via `path`) but is **not exposed as a solidcraft-cli subcommand that takes a design file**; `exec assembly.bom` runs on an empty design | **MISSING CLI surface** | need `solidcraft-cli <subcommand> <design> --out bom.json` (e.g. `bom`/`structure`) or extend `eval` |
| PDF export | none | **MISSING** | `render` writes PNG only |
| geometric fingerprint/hash | none | **MISSING** | `inspect.measure` bodies (volume/bbox/mass) is a coarse proxy only |

**Blocker:** `bom.extract`, `preview.model` (glTF), `release.package` (PDF), and `duplicate.search` (geometric) each depend on a CLI surface that does not yet exist. Confirm early in Increment 3 (per pdm-web-plan.md cross-cutting risk (a)).

## 2. Job specs

### 2.1 bom.extract (Increment 3, P1-9)

Purpose: dump a SolidCraft document's structure into `bom_snapshots` + `file_references` so the project BOM tab and item BOM/Dependencies tabs are real.

**Input params (job payload):**
- `version_id` (string UUID, required) — the version whose committed blob is the source design.
- `item_id` (string UUID, required) — the item the version belongs to (for `from_item_id` and the `doc_id` identity).
- `org_id` (string UUID, required) — for audit/outbox scoping.
- `depth` (integer, optional, default 64) — passed through as the CLI walk cap (engine `MAX_DEPTH`).
- `format` (`json\|csv`, optional, default `json`) — snapshot content format.

**solvecraft-cli command (NOT yet exposed — confirm early):**
```
solidcraft-cli <bom|structure> <design-file> --out <bom.json> [--depth N] [--format json|csv]
```
which opens the design and runs the existing `assembly.bom` engine command (flat levels, group_by part_number, group_children off by default) and writes JSON. Rows: `{item, part_number, description, revision, bom_type, count, definition, components, properties, children:[{item, part_number, description, revision, bom_type, each, count, definition}]}` plus `total_occurrences`.

Fallback until the CLI subcommand exists: none — do not synthesize a BOM from `inspect.measure`/`document.inspect` (they have no parent/child/quantity structure). Mark the job done with `{skipped: true, reason: "bom CLI surface not available"}`.

**Output blobs / DB rows:**
- `bom_snapshots`: one row per `(item, version)` — `revision_id` = the file_revision row for this version if released, else a placeholder; `generated_at = now()`; `content` = the full BOM JSON (rows + total_occurrences + doc_id + part_number + version seq).
- `file_references`: one row per BOM line — `from_item_id = item_id`, `to_item_id` = the child item resolved by matching `part_number` (or `doc_id` when the child is a SolidCraft document), `occurrence_count` = the line's `count`, `revision_policy = 'latest'`, `status = 'resolved'`, `suppressed = false`. `from_feature_id` / `resolved_revision_id` left NULL unless resolvable.
- `file_versions.measurements` untouched (that is `design.analyze`).

**Error handling:**
- CLI not configured → skip (`{skipped:true}`), complete job, no retry.
- CLI refuses the file / empty output → `Fatal`: record reason, complete job, no retry (do not dead-letter a file that will never yield a BOM).
- Transient (CLI won't run, OOM, IO) → normal retry → dead-letter after `max_attempts`.

**Idempotency:** keyed by `(item_id, version_id)`. Re-running overwrites the `bom_snapshots.content` row (delete+insert on that revision) and refreshes `file_references` for that `from_item_id` (recompute from the new snapshot, clear stale rows for the item first). Safe to run on every check-in; cheap no-op when the structure is unchanged (compare content hash before writing).

**UI surfacing:**
- Project *Bill of Material* tab (tree, expand/collapse, quantities, rollup counts, CSV export) — `GET /items/{id}/bom?rev=`, `GET /bom/{item}?depth=`.
- Item *BOM* / *Dependencies* tabs — `GET /items/{id}/where-used` (inverse of `file_references.to_item_id`).
- Revision row: released revision carries its BOM snapshot (`P1-9` pairs with P0-3 lifecycle/release snapshot).

---

### 2.2 preview.model (Increment 5 next-wave / P1-12)

Purpose: an interactive 3D model per version (glTF preferred, OBJ fallback), content-addressed like thumbnails, so the Preview tab becomes a real review surface.

**Input params (job payload):**
- `version_id` (string UUID, required).
- `item_id` (string UUID, required) — for extension detection (`preview_plan`).
- `format` (`gltf\|obj`, optional, default `gltf`).

**solvecraft-cli commands:**
- glTF: **none** (`file.export` has no gltf writer; glTF is import-only) → **confirm early / new export writer needed**. Until available, fall back to OBJ without erroring.
- OBJ fallback: `solidcraft-cli run --in <design-file> --out <model.obj>` (runs `file.export format=obj`). Also works for `.step`/meshes the CLI opens directly; B-rep designs are exported as tessellated mesh.

**Output blobs / DB rows:**
- New content-addressed blob (the `.obj` or `.gltf` bytes) via `store.put_content_addressed`.
- New column on `file_versions` (new migration, e.g. `0014`): `model_sha256 TEXT`, `model_size_bytes BIGINT`, `model_format TEXT` (`obj`/`gltf`), plus `model_error TEXT` for fatal refusals (mirrors `thumbnail_error`).
- Do **not** reuse `thumbnail_sha256` — model and thumbnail are separate deliverables with separate lifecycles.

**Error handling:**
- CLI not configured → skip, complete job, no retry.
- File type the CLI can't open (non-CAD) → `Skip`: clean "no model" state, no error recorded.
- Direct export fails on a convertible B-rep → not applicable (designs export directly); if a future convert-then-export path is added, mirror `preview.render`'s convert-then-snapshot fallback.
- Fatal refusal → record `model_error`, complete job, no retry. Transient → retry → dead-letter.

**Idempotency:** keyed by `version_id`; recompute and overwrite `model_*` columns when the version's blob SHA changes. Content-addressed storage deduplicates identical models.

**UI surfacing:** Item detail *Preview* tab — version selector; WebGL viewer (vendored vanilla JS, ~300 lines, no build step) with orbit/pan/zoom/fit + orientation cube; version A/B side-by-side; measurements overlay from `design.analyze` (bbox + mass). PNG remains fallback + thumbnail source. Processing-status chip wired to `/jobs` (rendering… / done / failed+reason).

---

### 2.3 release.package (Increment 5 next-wave / P2-20)

Purpose: on release, assemble a downloadable data package (BOM CSV + properties JSON + optional STEP/PDF) attached to the revision row.

**Input params (job payload):**
- `revision_id` (string UUID, required) — the released revision.
- `item_id` (string UUID, required), `version_id` (string UUID, required) — the released version.
- `include_step` (bool, optional, default true), `include_pdf` (bool, optional, default false).

**solvecraft-cli commands:**
- BOM CSV: `solidcraft-cli <bom|structure> <design> --out bom.csv --format csv` (same CLI surface as `bom.extract`; confirm early).
- properties JSON: `solidcraft-cli eval <design> --json-out properties.json` → `document.inspect` (timeline, params, sketches) + `inspect.measure` (mass, bbox, volume). EXISTS.
- STEP: `solidcraft-cli run --in <design> --out model.step` (`file.export format=step`). EXISTS for designs that export as STEP.
- PDF: **none** → **confirm early / new export needed** (optional; omit without failing the package).

**Output blobs / DB rows:**
- Content-addressed package blob: a zip (worker-side, in-Rust, no external CLI) containing `bom.csv`, `properties.json`, optional `model.step`, optional `model.pdf`. SHA = `package_sha256`.
- `file_revisions`: new columns (new migration): `package_sha256 TEXT`, `package_size_bytes BIGINT`, `package_created_at TIMESTAMPTZ`, `package_error TEXT`. One package per revision; regenerating overwrites.

**Error handling:**
- Missing CLI → skip package, record `package_error = "solvecraft-cli not configured"`, complete job (release is not blocked by the package).
- BOM CLI missing → package ships without `bom.csv` (note in `package_error`), not fatal.
- STEP/PDF export fails → omit that artifact, note in `package_error`, package still completes.
- Only a total inability to write the zip / store the blob is `Transient` → retry → dead-letter.

**Idempotency:** keyed by `revision_id`; release is one-shot per revision, but re-release overwrites the package row. Package content hash compared before writing to avoid redundant storage.

**UI surfacing:** Item detail *Revisions* tab — per-revision "download as package" button (F13 / P2-20). Revision row shows package size + generated-at.

---

### 2.4 duplicate.search (Increment 5 next-wave / P1-13-adjacent)

Purpose: property + geometric duplicate detection so users find "the same part" under different part numbers.

**Input params (job payload):**
- `item_id` (string UUID, required) — candidate item.
- `scope` (`org\|project`, optional, default `org`).
- `mode` (`property\|geometric\|both`, optional, default `both`).

**solvecraft-cli command:** none for property mode (pure DB). Geometric mode reuses `design.analyze`'s `eval` output (`inspect.measure` bodies: volume_mm3, bbox, mass, body_count) as a coarse geometric fingerprint — **not a dedicated hash; confirm early whether a geometric fingerprint command is needed**.

**Output blobs / DB rows:**
- No new table required initially. Results surfaced via a search endpoint (`GET /search?dup_of={item_id}`) or an outbox event; optionally a `duplicate_candidates` table (new migration): `id, item_id, candidate_item_id, mode, similarity, created_at`.
- Property duplicates: match on `properties` key/value + `items` (part_number, description, classification, doc_id).
- Geometric duplicates: fingerprint equality / near-equality of `file_versions.measurements.total` (volume/mass) + body_count.

**Error handling:** pure read-side; failures are query errors → `Transient` retry → dead-letter. No CLI involved.

**Idempotency:** deterministic given the same item + scope; recompute on demand, no stored state required (or refresh the candidates row).

**UI surfacing:** Search results page with a "duplicates of this item" facet; item detail *Dependencies* tab "possible duplicates" card.

## 3. Exact DB rows written (summary)

| Job | Table | Columns written | Keyed by |
|---|---|---|---|
| `bom.extract` | `bom_snapshots` | `revision_id`, `generated_at`, `content` (JSONB) | `(item, version)` |
| `bom.extract` | `file_references` | `from_item_id`, `to_item_id`, `occurrence_count`, `revision_policy`, `status` | `from_item_id` (refresh per item) |
| `preview.model` | `file_versions` (+new migration) | `model_sha256`, `model_size_bytes`, `model_format`, `model_error` | `version_id` |
| `release.package` | `file_revisions` (+new migration) | `package_sha256`, `package_size_bytes`, `package_created_at`, `package_error` | `revision_id` |
| `duplicate.search` | (none / optional `duplicate_candidates`) | — | `item_id` |

Existing columns already used by current jobs (do not reuse): `file_versions.measurements` (`design.analyze`), `file_versions.thumbnail_sha256`/`thumbnail_size_bytes`/`thumbnail_error` (`preview.render`), `file_versions.derived_step_sha256` (`cad.convert`).

## 4. Worker integration (pdm-worker)

- Register four new match arms in `run_job_kind` (`pdm-worker/src/lib.rs:145`): `bom.extract`, `preview.model`, `release.package`, `duplicate.search`.
- Keep `Outcome` enum or add per-job persist functions following `store_measurements` / `persist_thumbnail` / `store_derived_step` patterns (`lib.rs:211`).
- New `Outcome` variants: `BomSnapshot`, `Model { sha256, size_bytes, format }`, `Package { sha256, size_bytes }`.
- Each job function: validate `version_id`/`item_id`/`revision_id` from payload → `get_version` / `get_file` / `get_blob_by_id` → download blob via `store.get_by_sha` → write temp file → run CLI via `tokio::process::Command` → handle `CliFailure::Fatal`/`Transient` → `put_content_addressed` → persist rows.
- Enqueue points: `bom.extract` on check-in (after `design.analyze`/`preview.render`) and on release (snapshot); `preview.model` on check-in; `release.package` on `release()`; `duplicate.search` on demand (search action) or on check-in.

## 5. Blocker list (resolve before implementation)

1. **`bom.extract` CLI surface missing.** `assembly.bom` engine command exists but no `solidcraft-cli` subcommand takes a design file and writes BOM JSON/CSV. `exec assembly.bom` runs on an empty design. → add `solidcraft-cli bom <design> --out bom.json` (or extend `eval`). **Confirm early (Increment 3).** [pdm-web-plan.md risk (a)]
2. **`preview.model` glTF export missing.** `file.export` has no `gltf` writer (glTF import-only). OBJ fallback via `run --in --out model.obj` exists. → either add glTF export or ship OBJ-only. **Confirm early.**
3. **`release.package` PDF export missing.** No PDF writer in solidcraft-cli. PDF is optional in the package; package can ship without it. **Confirm early if PDF is required.**
4. **`duplicate.search` geometric fingerprint.** No geometric hash command; `inspect.measure` volume/bbox/mass is a coarse proxy only. → decide property-only vs add fingerprint CLI. **Confirm early.**
5. New migrations needed: `file_versions.model_*` columns, `file_revisions.package_*` columns (and optional `duplicate_candidates`). Schema is "far ahead of API" but these columns do not exist yet.
6. `solvecraft-cli` binary path is the PDM `PDM_SOLVECRAFT_CLI` env var; the binary is the SolidCraft `solidcraft-cli` crate (`apps/solidcraft-cli`). Ensure the worker's `solve_cli` points at a build that includes any new subcommands.

## 6. Where results surface in the UI (portal, `crates/pdm-api/static/`)

| Job | Route | UI surface |
|---|---|---|
| `bom.extract` | `GET /items/{id}/bom?rev=`, `GET /bom/{item}?depth=`, `GET /items/{id}/where-used`, `POST /bom/export` | Project *Bill of Material* tab; item *BOM* / *Dependencies* tabs; revision row snapshot |
| `preview.model` | `GET /versions/{id}/blob` (model) + `/jobs` status | Item detail *Preview* tab: 3D viewer, status chip (rendering/done/failed), A/B compare, measurements overlay |
| `release.package` | `GET /versions/{id}/blob` (package zip) | Item detail *Revisions* tab: "download as package" per revision |
| `duplicate.search` | `GET /search?dup_of=` (new facet) | Search "duplicates of"; item detail *Dependencies* "possible duplicates" card |

No routes for BOM/where-used/packages exist today (`pdm-web-plan.md` §1.2: "no route for BOMs"); they must be added in `pdm-api/src/routes.rs` alongside the worker jobs.
