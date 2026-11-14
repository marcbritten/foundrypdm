# SolidCraft Integration

SolidCraft stays fully usable **without** PDM. PDM is an optional adapter layer. The native design file
`.solidcraft` is a **content-addressed, opaque blob** to the PDM; only the PDM worker interprets it via
the headless `solvecraft-cli`. Identity is `Document.doc_id` (a stable UUID), not the filename.

## Two repos, one integration
- **PDM server** (this repo): storage, catalog, versions, auth, worker, portal.
- **SolidCraft CAD** (outer monorepo `solvecraft`): the desktop/web app that connects to the PDM.
  - `crates/pdm-client` (`solidcraft-pdm-client`) — an async reqwest `Vault` client + offline-workspace
    path manager + pure helpers (unit-tested). Phase-1 foundation; not yet wired into engine/UI.
  - `crates/ui-egui/src/pdm.rs` — the **live** integration, using a host-injected *blocking* transport
    (`apps/solidcraft/src/main.rs::pdm_call`, ureq, TLS-verified, bearer). Rationale: the UI runs
    background jobs like the update-check; keeping the transport host-injected keeps PDM networking out
    of the geometry/document crates.
  - `Document.doc_id` (`crates/doc`) — stable PDM identity (generated on load for legacy files).

## What's built in the CAD app (today)
- **Connection** (Preferences ▸ PDM): server URL, org, username/password, connection name; secure login
  → bearer token (cached, refreshed on 401); "Test connection"; connection status; per-user settings
  sync to `/me/settings` (SolidCraft Prefs follow the account across app updates).
- **Start page:** Team Projects, connection status, signed-in user.
- **Project window:** browse items (with thumbnails), open an item (download head version → local
  design), **switch between projects**, **create projects**, **create folders**, "Save design here…"
  (create item + check-in the open design).
- **Document workflow:** open PDM document; save new version (check-in with base-version + integrity);
  version history; per-design PDM association (`PdmDocumentRef` + a `meta.json` sidecar that survives
  restarts); a **persistent, user-configurable local download folder** (Preferences ▸ PDM ▸ Downloads).
- **Thumbnails:** the CAD app renders a preview of the open design and uploads it to
  `PUT /versions/{id}/thumbnail` on check-in (the PDM-only server can't render CAD itself).

## Phase 2 — deepen native integration (next)
- **`pdm.*` engine commands** in `crates/engine/src/cmd/pdm.rs` (on top of `solvecraft-pdm-client` +
  `Document.doc_id`): connect, browse, open, save-version, list-versions, checkout/checkin, presence.
- **Dockable PDM panel** (left/right, resizable, remembered layout) with sections: Project Browser,
  Current Document, Versions, Branches, Collaborators, Comments, BOM, Manufacturing, Activity.
- **Offline drafts + local caching**; **detect remote changes**; **resolve version conflicts** without
  losing either side; open a historical version read-only.
- **Presence** ("who else has this open") — from `file_locks` / a new presence channel.

## Acceptance criteria (from `ToDoIntegrateSC.txt`)
Two users on separate machines: connect to the same PDM; browse the same project; open a shared
SolidCraft design; see who else has it open; save a new version; get notified of remote changes;
inspect history; resolve a concurrent-save conflict without losing work. (Simultaneous editing of the
*same feature* is a later milestone.) Deliver automated tests + a manual two-client procedure.

## API compatibility negotiation (needed)
Add a `GET /capabilities` (or extend `/version`) that reports the PDM API version + feature flags so the
CAD can adapt (graceful degradation, like today's 404-tolerant `/me/settings`). The CAD already degrades
gracefully on missing endpoints.
