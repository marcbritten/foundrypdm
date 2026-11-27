# Data Model

PostgreSQL. Migrations in `crates/pdm-db/migrations/` (applied on boot, tracked in `schema_migrations`).
UUID PKs; `org_id` on every tenant-scoped table. Postgres 13+.

## Migrations
- `0001_init.sql` — the full schema (catalog, files/versions/locks, plus BOM/change/lifecycle/
  approval/material tables for the target model).
- `0002_thumbnails.sql` — `file_versions.thumbnail_sha256`, `.thumbnail_size_bytes`.
- `0003_client_settings.sql` — `client_settings` (per-user jsonb Prefs payload).
- `0004_comments.sql` — `item_comments` + index.

## Tenancy & identity
- **organizations** — top-level tenant.
- **users** — `org_id`, `email` (unique per org), `display_name`, `password_hash` (Argon2id, optional),
  `role` (`viewer`|`editor`|`admin`), `active`.
- **api_tokens** — bearer tokens, stored SHA-256; scoped to user/service-account. `kind`: `web` for
  a password sign-in (expires), `api` for every other token.
- **service_accounts** — non-interactive credentials.
- **groups / roles / permissions / memberships** — full RBAC model (schema present, Phase-1 uses `users.role`).

## Catalog
- **projects** — `org_id`, `name`, `description`, `image_data` (cover `data:image` URL),
  `main_assembly_id` (FK → `items`; the project's main assembly — the BOM root and the source of the
  project's cover image, `0017_main_assembly.sql`).
- **folders** — `org_id`, `project_id`, `parent_id` (nested), `name`.
- **items** — a part/document: `org_id`, `project_id`, `folder_id`, `part_number`, `classification`,
  `description`, `owner_id`, `lifecycle_state`.
- **number_sequences** — atomic per-classification part-number counters (e.g. `SC-000001`).
- **properties** — typed key/value attributes on items.

## Files & versions (the core)
- **files** — one per item: `item_id`, `filename`, `mime_type`.
- **file_blobs** — content-addressed: `org_id`, `sha256`, `size_bytes`, storage key.
- **file_versions** — `file_id`, `seq` (monotonic per-save counter), `version` (major version,
  bumped only on an explicit "save as new version"), `revision` (per-save counter within the
  major version; e.g. V2 - R5 is the fifth save inside version 2), `blob_id`, `author_id`,
  `commit_message`, `base_version_id` (for conflict detection), `native_format`, `measurements`
  (jsonb, from the worker), `thumbnail_sha256`/`thumbnail_size_bytes` (a blob, PNG preview), `committed_at`.
- **file_revisions** — named labels onto versions (`label`, e.g. `A`, `B`, `REL-1`).
- **file_references** — cross-document refs (`pdm://`), where-used.
- **file_locks** — exclusive checkout: `item_id`, `user_id`, `workstation`, `lease_expires_at`.

## Target-model tables (present, not yet wired)
- **BOM:** `bom_snapshots` (+ `file_references`).
- **Change management:** `change_requests`, `change_orders`, `change_notices`, `change_affected_items`.
- **Lifecycle:** `lifecycle_definitions`, `lifecycle_states`, `lifecycle_transitions`.
- **Approvals:** `approval_requests`, `approval_decisions`.
- **Materials:** `materials`.
- **Audit:** `audit_events` (immutable; `action`, `object_type`, `object_id`, `actor_id`, `detail`).
- **Jobs:** `jobs` + `job_attempts` (the worker queue: `type`, `payload`, state, attempts).
- **Integration:** `integration_outbox` (event outbox for fan-out / webhooks).

## Invariants
- A version row is **immutable** once created; the head per item = max `seq`.
- Every check-in creates a new `file_versions` row: a plain save is the next `revision` of the
  current `version`; "save as new version" bumps `version` and resets `revision` to 1.
- Check-in is rejected (409) if `base_version` (a `seq`) is stale or the blob's stored SHA doesn't verify.
- NC programs are flagged `outdated` only when a new major version is checked in; revisions
  within a version stay compatible with the same manufacturing chain.
- Part numbers are unique within their classification; allocation is atomic.
- Org isolation: every read/write is filtered by the caller's `org_id`.

> Manufacturing / CAM tables (machines, tool library, setups, NC programs, inspection) are **not in the
> schema yet** — the target model is in `docs/manufacturing.md`.
