# Foundry PDM — System Architecture

Self-hosted, Docker-deployable Product Data Management for mechanical engineering, robotics
teams, and manufacturing. Primary native CAD: **SolidCraft** (`https://github.com/bherbruck/solidcraft`);
other CAD supported as files (STEP, IGES, STL, DXF, …). Design references: Autodesk Vault,
Onshape PDM/branching, Fusion Manage, Git-style revision control — implemented originally, not copied.

## Tech stack
- **Language / runtime:** Rust (edition 2024, MSRV 1.90), tokio multi-thread.
- **HTTP:** axum 0.8 (multipart) + tower / tower-http (trace, cors, set-header, fs).
- **DB:** PostgreSQL via sqlx 0.8 (rustls, uuid, chrono, json, `migrate`). Migrations run on boot.
- **Object storage:** `object_store` 0.12 — S3/MinIO (production) or local filesystem (dev), chosen by env.
- **Passwords:** Argon2id. **API tokens:** bearer, stored SHA-256-hashed.
- **Frontend:** a single self-contained HTML/JS/CSS portal served at `/` (no build step) — see `docs/api-specification.md` §Frontend.

## Crate layout (this workspace)
| Crate | Responsibility |
|---|---|
| `pdm-core` | Entities + `PdmError`; domain rules (part-number format, check-in validation). No I/O. |
| `pdm-db` | Postgres migrations + repository layer (typed queries over sqlx). |
| `pdm-storage` | SHA-256 content-addressed blob store: `put_content_addressed`, `get_by_sha`, `verify_by_sha`, `exists`, `delete`. Local + S3/MinIO. |
| `pdm-api` | axum router: auth, catalog (projects/folders/items), files/versions, locks, check-in, blobs, search, audit, properties, thumbnails, settings, portal. |
| `pdm-worker` | Postgres job queue; `design.analyze` (measurements) and `preview.render` (thumbnails) by shelling to `solvecraft-cli`. |
| `pdm-mcp` | MCP server adapter (stub, Phase 5). |
| `pdm-erp` | ERP integration (stub, Phase 5). |
| `apps/pdm-server` | Binary: boots the router + optional in-process worker; runs migrations. |
| `apps/pdm-cli` | Ops CLI: `migrate`, `create-org`, `create-user`, `set-password`, `create-token`. |

**Layering:** `pdm-core` ← `pdm-db` ← `pdm-api`/`pdm-worker` ← `pdm-server`. The API and the
worker are the only I/O owners; `pdm-core` stays pure.

## Data flow
```
CAD app / web portal / CLI
        │  HTTPS/HTTP + Bearer token
        ▼
pdm-api (axum) ──► pdm-db (Postgres: catalog, versions, locks, audit, jobs)
        │            └─► pdm-storage (content-addressed blobs, S3/MinIO)
        │
        └─ enqueue job ──► pdm-worker ── solvecraft-cli (eval / snapshot) ──► store result on version
```
Check-in is **atomic and integrity-checked**: the client uploads bytes to `/blobs` (content-addressed,
SHA-256), then `POST /items/{id}/checkin` references the blob's SHA and a `base_version`; the server
verifies the blob's stored hash, validates the base version, creates the immutable `file_versions` row,
and releases the checkout lock.

## Content-addressed storage model
- Blobs are immutable, keyed by SHA-256; object-store keys are org-namespaced
  (`blobs/{org}/{sha256}`), staging is `staging/{org}/{upload_id}`.
- `file_blobs` records (sha256, size, org) let the catalog reference blobs without hitting the store.
- A design (SolidCraft or any CAD) is an opaque blob to the PDM; only the worker interprets it (via CLI).

## AuthN / AuthZ
- **AuthN:** bearer API tokens (`api_tokens`, SHA-256-stored). `POST /auth/login` exchanges
  username+password (Argon2id) for a web sign-in token that expires (`docs/security.md`); API
  tokens for connectors and scripts don't.
- **AuthZ:** RBAC via `users.role` (viewer/operator/editor/admin, see `security.md`) in Phase 1; full `roles`/`permissions`/
  `groups`/`memberships` schema exists for the later model. **Org isolation** is enforced in every
  query (`org_id` in the WHERE clause), not by convention.
- See `docs/security.md`.

## Deployment & scaling
- **Docker Compose:** `postgres` + `minio` (+ bucket init) + `pdm`. The PDM server runs migrations on
  boot and can host the worker in-process (`PDM_WORKER=1`, default) or run a dedicated worker.
- **Scaling:** the API is stateless (Postgres + object store are shared), so it scales horizontally; the
  worker scales separately off the Postgres job queue. Live-change fan-out (presence, "changes without
  refresh") needs a WebSocket/event channel — not yet present (roadmap).
- **Dev image** (`Dockerfile.dev`) is PDM-only (no CLI) — thumbnails come from the client. **Production
  image** (`Dockerfile`) cross-compiles `solvecraft-cli` into the PDM so the worker can render.

## Key architectural decisions
1. **PDM is a separate server**; SolidCraft remains fully usable without it (an optional adapter layer).
2. **Designs are opaque, content-addressed blobs.** The PDM never links the CAD kernel; it shells out to
   `solvecraft-cli` (headless) for analysis/preview. This keeps the CAD dependency tree out of the PDM.
3. **Stable identity:** SolidCraft `Document.doc_id` (UUID) is the PDM document identity, not the filename.
4. **Nested workspace:** the PDM is its own Cargo workspace so axum/tokio/sqlx never enter the CAD kernel.
