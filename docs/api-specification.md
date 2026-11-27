# API Specification

HTTP/JSON, bearer-token auth (except `/version`, `/healthz`, `/readyz`, `/auth/login`). All
tenant-scoped routes operate in the caller's `org_id`. Errors: `4xx/5xx` with body
`{ "error": { "code", "message" } }` (some include structured detail, e.g. check-in conflict).
Base URL in the dev image: `http://<host>:8080`.

## Endpoints (as implemented, `crates/pdm-api/src/routes.rs`)

### Liveness / identity
- `GET /healthz`, `GET /readyz` — probes.
- `GET /version` — public. `{ name, version, build, commit, built_at }`. `build` = test-build tag
  (`PDM_BUILD_TAG`, e.g. `V.01-03`), `commit` baked at build (`PDM_GIT_COMMIT`).
- `POST /auth/login` — `{ username, password, org? }` → `{ token, user, org }`. The token is a web
  sign-in: it ends after an idle spell or at its lifetime (`PDM_SESSION_IDLE_MINUTES`,
  `PDM_SESSION_LIFETIME_MINUTES`). The first request after that gets 401 with code
  `session_expired` and the token is deleted, so later ones get 401 `auth_failed`. Tokens from
  `POST /users/{id}/tokens` never expire on their own.
- `POST /auth/logout` — invalidate the current token.
- `GET /me` — the signed-in identity.
- `GET /me/settings`, `PUT /me/settings` — per-user jsonb payload (`{ payload }`). SolidCraft
  syncs its Prefs here.

### Projects / folders / items
- `GET /projects` → `{ projects: [ {id, name, description, ...} ] }`.
- `POST /projects` `{ name, description? }` → `{ project }`.
- `GET /projects/{id}/items` → `{ items: [ItemSummary] }`. **ItemSummary** includes
  `part_number`, `description`, `classification`, `lifecycle_state`, `doc_id`, `head_version_seq`,
  `head_version`, `head_revision`, `latest_revision`, `modified_by`, `locked_by`, and (new) `filename`, `mime_type`.
- `POST /projects/{id}/items` `{ part_number?, description?, classification?, folder_id? }` → `{ item }`.
- `GET /projects/{id}/folders` → `{ folders: [...] }`. `POST /projects/{id}/folders` `{ name, parent_id? }` → `{ folder }`.
- `PUT /projects/{id}/main-assembly` `{ item_id? }` → `{ project }` — set a project's **main assembly** (the
  item the BOM is built off); `item_id: null` clears it. The item must belong to the project. The
  project's cover image is this item's thumbnail; `Project` responses now include `main_assembly_id`.
- `GET /items/{id}` — item detail (incl. head version + where-used).
- `GET /items/{id}/properties`, `PUT /items/{id}/properties/{key}`.
- `POST /items/{id}/release`.

### Files / versions
- `PUT /blobs` — raw body → `{ blob: { sha256, size_bytes, ... } }` (content-addressed, org-scoped).
- `POST /items/{id}/checkout` `{ workstation? }` → `{ lock, base_version }` (409 if held by another).
- `DELETE /items/{id}/checkout` — undo.
- `POST /items/{id}/checkin` `{ sha256, commit_message?, base_version?, filename?, mime_type?, new_version? }` →
  `{ version }` (409 on stale `base_version`; verifies blob SHA). `new_version` (default `false`)
  controls versioning: a plain save is a new **revision** of the current version (V1 - R2), while
  `new_version: true` starts a new **major version** (V2 - R1). A new major version also flags the
  item's NC programs as outdated; a plain revision does not. **Version** includes `id`, `seq`,
  `version`, `revision`, `file_id`, `author_id`/`author_name`, `commit_message`, `base_version_id`,
  `blob_id`, `sha256`, `size_bytes`, `native_format`, `mime_type`, `measurements`,
  `thumbnail_sha256`/`size`.
- `GET /items/{id}/versions` → `{ versions: [Version] }` (newest last).
- `GET /versions/{id}/blob` — raw bytes + `Content-Digest: sha-256=…`.
- `GET /versions/{id}/thumbnail`, `GET /items/{id}/thumbnail` — PNG preview (**auth-gated**; 401 without
  a token, 404 when none). `PUT /versions/{id}/thumbnail` — store a client-rendered preview (raw PNG body).

### Search / audit
- `GET /search?q=&project?` → `{ results: [ItemSummary] }`.
- `GET /audit?item?` → `{ events: [ {id, action, object_type, actor_id, at, ...} ] }`.

### Admin (Phase-1)
- `POST /users` — create a user. `POST /users/{id}/tokens` — issue a token.
- `PUT /users/{id}` — admin updates a member's `display_name`, `role`, or `active`
  status within the current organization. Set `active: false` to disable access
  and `active: true` to enable it. Disabled users cannot sign in or authenticate
  with existing tokens. Disabling or demoting the last active administrator
  returns `409 Conflict`; this constraint also applies to concurrent updates.

## Gaps (roadmap)
- **Pagination** on list endpoints (currently unbounded).
- **Comments** (`GET/POST /items/{id}/comments`) — schema exists (0004), API not exposed.
- **BOM**, **change orders**, **lifecycle**, **approvals**, **org/team management**, **presence**,
  **WebSocket** events — not yet exposed.
- **API compatibility negotiation** (a `capabilities`/version handshake) — see `docs/development-roadmap.md`.

## Frontend
The web portal (`crates/pdm-api/src/portal.rs`) is a single self-contained HTML/JS page served at `/`
(no build step), authenticated with the same bearer token, calling the endpoints above. A `static/`
dir (or `PDM_PORTAL_DIR`) overrides it when present. The web workspace build-out is a separate effort
(`ToDoWebUI.txt`).
