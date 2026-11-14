# CAD Connectors

The PDM is **CAD-neutral**: a design is an opaque, content-addressed blob; PDM stores it and only the
worker/adapter layer interprets it. `Document.doc_id` (SolidCraft) or a stable item id is the identity.

## SolidCraft (primary, native)
- **Store:** `.solidcraft` files as blobs (SHA-256, immutable, versioned).
- **Analyze / preview:** the PDM worker shells out to headless `solvecraft-cli`:
  - `solvecraft-cli eval` → measurements → stored on the version (`design.analyze`).
  - `solvecraft-cli snapshot` → PNG preview → the version's thumbnail (`preview.render`).
  - In the **PDM-only dev image** the CLI is absent, so the **CAD app uploads its own rendered
    thumbnail** (`PUT /versions/{id}/thumbnail`). Both paths converge on the same thumbnail slot.
- **Identity:** `Document.doc_id` (UUID) is embedded in the file; PDM references use it, not the name.
- **Branches/versions:** PDM versions are linear per item today; the roadmap adds Git-style branches
  (see `docs/development-roadmap.md`).

## Generic CAD (STEP, IGES, STL, DXF, 3MF, glTF, OBJ, …)
- **Store:** as blobs; `files.mime_type` + `filename` carry the type (shown in the portal/UI).
- **Preview/analyze:** adapters per format where worthwhile (DXF/PDF/images → 2D preview in the web
  detail page; STL/mesh → mesh preview). These are **optional** — a file with no supported preview shows
  a clean "no preview" state, not a broken one.
- **CAD-neutral metadata:** classification, material, key dimensions captured as `properties` so a part
  is searchable/usable even if its native format isn't understood.

## Other native CAD (Inventor, Fusion, Onshape)
- Out of scope for Phase 2; treat as **foreign formats** (store + optional preview). Native two-way
  integration is a later, per-vendor effort. Onshape/Inventor could eventually use their own APIs; keep
  the connector an adapter behind the same PDM version/checkout model.

## Design constraints
- No CAD-format parsing in the low-level PDM crates. Adapters live in the worker or a dedicated
  `pdm-cad` layer (roadmap).
- Every connector: (1) check the blob's SHA on download; (2) never overwrite an immutable version;
  (3) surface processing status + errors in the UI instead of empty previews.
