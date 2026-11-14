# Implementation Status

Point-in-time inventory (2026-10-16). Source of truth for "what exists vs. what's missing." See
`STATUS.md` (progress tracker) and `ToDoIntegrateSC.txt` (integration spec).

## Working (Phase 1 — minimum functional vault) ✅
- **Catalog:** organizations, projects, folders, items (parts) — CRUD + part-number sequences.
- **Files/versions:** content-addressed blobs, immutable versioned check-in with base-version + SHA-256
  integrity validation, exclusive checkout with leases, version history, blob download (Content-Digest).
- **Locks:** per-item exclusive checkout (`workstation`, `lease_expires_at`).
- **Search:** FTS + prefix search over items. **Audit:** immutable audit trail.
- **Properties/Revisions/Release:** typed properties, revision labels, item release.
- **Storage:** local filesystem (dev) + S3/MinIO (prod), content-addressed, org-namespaced.
- **Worker (Postgres job queue):** `design.analyze` (measurements) + `preview.render` (thumbnails) via
  `solvecraft-cli`; jobs skip cleanly when the CLI is absent (PDM-only dev image).
- **Auth:** bearer tokens (SHA-256), Argon2id passwords, RBAC roles, org isolation in every query.
- **Per-user client settings:** `client_settings` (jsonb) — `GET/PUT /me/settings` (SolidCraft Prefs sync).
- **Comments:** `item_comments` table (migration 0004) — **schema exists; API not yet exposed.**
- **Version build info:** `GET /version` (public) returns name/version/commit/built_at + test-build tag.
- **Thumbnails:** `GET/PUT /versions/{id}/thumbnail`, `GET /items/{id}/thumbnail` (client uploads its own
  preview in the PDM-only build).
- **E2E test:** `apps/pdm-server/tests/e2e_vault.rs` driven by `e2e.sh` — two users, checkout contention,
  stale base-version 409, integrity, audit, search.
- **CAD-side (outer SolidCraft monorepo):** `crates/pdm-client` (async Vault client, Phase-1 foundation);
  `crates/ui-egui/src/pdm.rs` (live PDM: connection, start-page Team Projects, project window with
  project switching + create project/folder + items + check-in + thumbnails, settings sync, per-design
  PDM association, persistent local download folder); `Document.doc_id`.

## Present but NOT yet wired (schema ahead of API)
- **BOM:** `bom_snapshots` (+ cross-document `file_references`, `pdm://` where-used) — table only.
- **Change management:** `change_notices`/`change_orders`/`change_requests`/`change_affected_items`.
- **Lifecycle:** `lifecycle_definitions`/`lifecycle_states`/`lifecycle_transitions` (items carry a
  `lifecycle_state`, but no workflow engine).
- **Approvals:** `approval_requests`/`approval_decisions`.
- **Materials:** `materials`.
- **Full RBAC:** `roles`/`permissions`/`groups`/`memberships` (Phase-1 uses `users.role`).
- **`pdm-mcp` / `pdm-erp`:** stubs.

## Missing (target of the roadmap)
- **Web workspace** (dashboard, project explorer tree/table, engineering-file detail w/ tabs + 3D/2D
  preview, version-control UI, comments, live updates) — in progress (web-UI task).
- **Native CAD integration (Phase 2):** `pdm.*` engine commands (`crates/engine/src/cmd/pdm.rs`), dockable
  PDM panel, offline drafts, presence, conflict resolution, branch switching.
- **Other CAD connectors** (STEP/IGES/STL/DXF) — stored as blobs; analysis/preview adapters.
- **Manufacturing / CAM subsystem** — **core done**: machine registry + tool library + post-processors,
  the setup→CAM→NC→job→inspection chain, per-item status + org dashboard, and the NC-outdated
  data-integrity invariant (migrations 0005–0007, deployed V.01-07→V.01-11, `e2e_mfg.rs` passing).
  Remaining: sheet metal, 3D printing, G-code parse/viz, released-package immutability.
- **Organizations/teams management API**, **pagination**, **WebSocket live updates**, **OIDC**.

## Build / test commands (existing app)
```bash
# PDM workspace (this repo)
cargo build -p pdm-api                     # the API crate
cargo test                                  # all PDM unit tests
cargo fmt --check && cargo clippy --all-targets -D warnings
# E2E (needs a running postgres + minio; see e2e.sh / docker/pdm-dev.yml):
./e2e.sh

# Run locally (full stack, PDM-only dev image):
PDM_BUILD_TAG=V.01-XX bash docker/build-dev.sh   # -> http://127.0.0.1:8080
curl -s http://127.0.0.1:8080/version
```
