# Development Roadmap & Implementation Backlog

Ordered, dependency-aware plan. Status as of 2026-10-16. Milestones build on Phase 1 (the working
vault, see `docs/implementation-status.md`).

## Milestones
- **Phase 1 — Minimum functional vault:** ✅ done, E2E-proven (`e2e.sh`).
- **Phase 2 — Native CAD integration (active):** `pdm.*` engine commands, dockable PDM panel, offline
  drafts, presence, remote-change detection, conflict resolution. (`ToDoIntegrateSC.txt`)
- **Phase 3 — Collaboration:** BOM + where-used, comments (schema ready), approvals, lifecycle
  workflows, change orders, Git-style branches, live event channel.
- **Phase 4 — Web workspace:** dashboard, project explorer (tree/table), engineering-file detail with
  3D/2D preview, version-control UI, permission-aware actions. (`ToDoWebUI.txt` — in progress)
- **Phase 5 — Multi-CAD + Manufacturing + Platform:** CAD connectors, MFG/CAM subsystem, MCP, ERP,
  OIDC, full RBAC, horizontal scaling.

## Prioritized backlog (Phase 2 focus; tasks are small & independently testable)
Legend: **[PDM]** = this repo; **[CAD]** = outer SolidCraft monorepo; deps in `(← …)`.

**API / backend**
- T1 [PDM] `GET /capabilities` — API version + feature flags (client negotiation). *(none)*
- T2 [PDM] `GET /items/{id}/lock` — read the active checkout (presence basis). *(none)*
- T3 [PDM] `GET/POST /items/{id}/comments` — wire migration 0004 to the API. *(none)*
- T4 [PDM] WebSocket `/events` (org-scoped) fanning out from `integration_outbox`. *(← T1)*
- T5 [PDM] Branches on a file (`file_branches` + check-in `base_version_id` already modeled). *(← T1)*

**CAD — engine commands & client**
- T6 [CAD] `pdm.*` engine commands in `crates/engine/src/cmd/pdm.rs` on `solidcraft-pdm-client`
  + `Document.doc_id`: `pdm.connect/browse/open/save-version/versions/checkout/presence`. *(← T1,T2)*
- T7 [CAD] Dockable PDM panel (left/right, resizable, remembered layout) with sections: Project
  Browser, Current Document, Versions, Branches, Collaborators, Comments, BOM, Manufacturing,
  Activity. *(← T6)*
- T8 [CAD] Offline drafts + local caching on the persistent download folder. *(← T6)*
- T9 [CAD] Detect remote changes; resolve conflicts without losing either side (stale `base_version`
  409 → show both heads, rebase/rename). *(← T6)*
- T10 [CAD] Open a historical version read-only. *(← T6)*

**Verification**
- T11 [PDM+CAD] Two-client automated E2E (connect→browse→open→both-save→resolve) + manual procedure. *(← all)*

### First ten actionable coding tasks
1. T1 — add `GET /capabilities` (pdm-api) + a CAD `capabilities()` on the client.
2. T2 — add `GET /items/{id}/lock` (pdm-api).
3. T3 — expose `GET/POST /items/{id}/comments` (pdm-api + portal already expects it).
4. T6 — scaffold `crates/engine/src/cmd/pdm.rs` with `pdm.connect` + `pdm.browse` (CAD), registered in
   the engine command registry, layered on `solidcraft-pdm-client`.
5. T7 — the dockable panel shell (layout persist + section frames) in `crates/ui-egui`.
6. T8 — local offline-draft store keyed by `Document.doc_id` under the PDM download folder.
7. T9 — remote-change polling + conflict UI (stale base → show both).
8. T10 — read-only open of a version (download + open with a "read-only" flag).
9. T4 — `/events` WebSocket + a CAD subscriber for live "someone saved" notifications.
10. T11 — the two-client E2E test.

## Build & test commands
```bash
# PDM (this repo) — run from the PDM root:
cargo build --workspace
cargo test                                    # unit + integration tests per crate
cargo fmt --check && cargo clippy --all-targets -D warnings
./e2e.sh                                      # two-user vault E2E (needs postgres+minio up; see docker/pdm-dev.yml)

# Run the full dev stack (PDM-only image) locally:
bash docker/build-dev.sh                      # -> http://127.0.0.1:8080  (PDM_BUILD_TAG to version it)

# CAD (outer SolidCraft monorepo):
cargo xtask ci                                # fmt, clippy, tests, layering, wasm, assets, licences
cargo test -p solidcraft-ui-egui pdm          # PDM UI tests
cargo test -p solidcraft-pdm-client           # the Vault client

```

## Deploy / versioning
Bump the test-build label `V.01-NN` per iteration (`PDM_BUILD_TAG` → `GET /version` `build` field +
docker image tag). Current host build: V.01-03 (web-portal file-type/build-tag/thumbnails).
