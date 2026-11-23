# PDM worker jobs — blocker list (Increments 3–5)

Jobs: `bom.extract`, `preview.model`, `release.package`, `duplicate.search`.
Spec: `/workspace/solidcraft-pdm/docs/worker-jobs-spec.md`

| # | Blocker | Job(s) affected | Required action | Confirm early? |
|---|---------|-----------------|-----------------|----------------|
| 1 | No `solidcraft-cli` subcommand opens a design file and writes BOM JSON/CSV. `assembly.bom` engine command exists; `exec assembly.bom` runs on an empty design. | `bom.extract` | Add `solidcraft-cli bom|structure <design> --out bom.json` (runs `assembly.bom`). | Yes — Increment 3 (pdm-web-plan.md risk (a)) |
| 2 | No glTF export writer (`file.export` formats: stl/stla/obj/step/3mf/urdf/json; glTF is import-only). OBJ fallback via `run --in --out model.obj` exists. | `preview.model` | Add glTF export or ship OBJ-only model. | Yes |
| 3 | No PDF export in solidcraft-cli (`render` writes PNG only). PDF optional in package. | `release.package` | Add PDF export or drop PDF from package. | Yes (if PDF required) |
| 4 | No geometric fingerprint/hash command; `inspect.measure` volume/bbox/mass is a coarse proxy only. | `duplicate.search` | Property-only duplicates, or add geometric fingerprint CLI. | Yes |
| 5 | Schema columns missing: `file_versions.model_sha256/size/format/error`; `file_revisions.package_sha256/size/created_at/error`; optional `duplicate_candidates` table. | `preview.model`, `release.package`, `duplicate.search` | New migration(s) (cf. `0012_preview_errors.sql`, `0011_derived_cad.sql`). | No (schema work) |
| 6 | `PDM_SOLVECRAFT_CLI` must point at a `solidcraft-cli` build that includes any new subcommands. | All CAD jobs | Coordinate binary version with worker config. | No (ops) |
| 7 | `cad.solidcraft` needs a `dutchman` binary for the native formats (Fusion, Inventor, SolidWorks, FreeCAD, CATIA, Rhino, Creo View, ACIS, STEP); without it, Inventor/FreeCAD/Rhino/STEP fall back to `PDM_SOLVECRAFT_CLI` and meshes/IGES still use it. The PDM pulls the newest [dutchman-cad/dutchman](https://github.com/dutchman-cad/dutchman) release itself (`PDM_DUTCHMAN_CLI`, `PDM_DUTCHMAN_UPDATE`). | `cad.solidcraft` | None — self-updating. | No |

No routes for BOM/where-used/packages exist in `pdm-api/src/routes.rs` today either (`pdm-web-plan.md` §1.2) — backend routes are a separate, non-blocker follow-up to the job spec.
