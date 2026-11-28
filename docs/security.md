# Security

## Authentication
- **Bearer API tokens** (Phase 1). `POST /auth/login` exchanges `username` + `password` for a token;
  tokens are stored SHA-256-hashed in `api_tokens`. On a 401 the portal asks the person to sign in again.
- **Web sign-ins expire; API tokens do not.** A token from `POST /auth/login` is a web sign-in
  (`api_tokens.kind = 'web'`): it ends after `PDM_SESSION_IDLE_MINUTES` without a request (default
  120) or `PDM_SESSION_LIFETIME_MINUTES` after sign-in (default 720). The first request with an
  ended sign-in gets 401 `session_expired` and deletes it, so it stays refused even if the limits
  are raised later; one never presented again stays in the table and follows whatever limits are
  set. `POST /auth/logout` deletes it at once. Tokens an
  administrator or `pdm-cli create-token` issues (`kind = 'api'`) are for CAD connectors, agents
  and scripts and never expire on their own.
- **Passwords:** Argon2id (`m=19MiB, t=2, p=1`). Optional (users can be token-only, e.g. via
  `service_accounts` / `pdm-cli create-token`).
- **OIDC** is a later hardening item (provider TBD; keep Argon2id as fallback).

## Authorization
- **RBAC** via `users.role` in Phase 1. Roles rank `viewer` < `operator` < `editor` < `admin`;
  each can also do everything the roles before it can:

  | Role | Adds |
  |---|---|
  | `viewer` | Reads the organization's projects, items, files and records; comments; opens versions through design links; frees a lock they hold; decides an approval request that names them. |
  | `operator` | Records what happens on the shop floor: production job status (`POST /production-jobs/{id}/status`), print job status (`POST /print-jobs/{id}/status`) and inspections (`POST /inspections`). |
  | `editor` | Changes the vault (projects, items, check-out and check-in, uploads, properties, release, lifecycle, archive, delete; Onshape imports, CAD conversions, previews), tools, machines, post-processors and manufacturing records (setups, CAM projects, NC programs and their release, production and print jobs, sheet metal, nests); raises approval requests and decides any of them. |
  | `admin` | Manages users, API tokens and the organization's settings. |

  Set a role with `pdm-cli create-user --role <role>` or in **Administration → Users**. The full
  `roles`/`permissions`/`groups`/`memberships` model is in the schema for the later phase.
- **Org isolation** is enforced in **every** query (`org_id` in the WHERE clause), not by convention —
  a user of one org cannot read another org's items/blobs/versions.
- Service accounts (`service_accounts`) for headless/non-user credentials.
- **Host admins.** Some things are shared by every organization on a server: the CAD CLI and
  SolidCraft web editor builds on the host, the Dutchman release channel (one binary converts for
  everyone) and the GPU policy of the shared preview renderer. Only a host admin
  (`users.is_host_admin`, an active admin) changes them; org admins see them read-only and get
  `403` from the API. Org admins keep everything scoped to their own organization.
  - The first admin made with `pdm-cli create-user --role admin` on a server with no host admin
    becomes one. On upgrade (migration 0029), the earliest-created active admin of the
    organization created first does (any organization's, if that one has none), and the channel
    and GPU policy start from that organization. After upgrading, check with
    `pdm-cli host-admin list` and grant or revoke as needed.
  - Host admins grant and revoke it for admins of any organization (`PUT /users/{id}/host-admin`,
    or the Users table in Administration). The last host admin cannot be revoked, and a host admin
    cannot be disabled or demoted until it is revoked. Grants and revocations are audited.
  - From the grant on, only a host admin can reset a host admin's password, give them an API token
    or edit their account. Passwords and API tokens set before the grant are left as they are: if
    another admin ever set that account's password or issued it a token, change the password and
    replace the tokens (after an upgrade too).
  - Recovery when no host admin can sign in: `pdm-cli host-admin grant --email … [--org …]`;
    `pdm-cli host-admin list` shows who holds it.
  - What describes the whole server is shown to host admins only: on the System page and
    `GET /system-info`, the infrastructure (PostgreSQL, object storage), uptime, server-wide
    organization and user counts, binary and build paths on the host, and the GPU survey
    (which runs the CAD CLI on the host). Everyone else sees the PDM build, the CAD CLI
    version, the GPU policy and their own organization's counts.

## Data & storage
- **Content-addressed, immutable blobs** (SHA-256). Integrity is verified on check-in
  (`verify_by_sha`) and exposed on download (`Content-Digest`).
- Object-store keys are **org-namespaced** (`blobs/{org}/{sha256}`) so a bucket can hold many tenants.
- **Credential exposure:** MinIO/S3 credentials stay server-side (`PDM_S3_*`); the browser/portal never
  sees them — files move through PDM's blob endpoints, not direct S3 (the roadmap item "do not expose
  object-storage credentials to the browser" is met by the current design).

## Transport
- CAD client (desktop) uses **ureq with platform TLS + certificate validation** (https only for
  https:// URLs; http:// allowed for trusted LAN servers). The PDM worker/CLI honor the same.
- **Do not disable TLS validation.**

## Threats / open items
- API token expiry/rotation policy (API tokens are long-lived until deleted; web sign-ins expire,
  see above).
- Upload size limit (`PDM_MAX_UPLOAD_BYTES`, default 256 MiB).
- Rate limiting and request logging for production.
- WebSocket event channel must carry only what the token allows (org-scoped events).
- Secrets at rest: tokens hashed; passwords Argon2id; S3 creds env-injected.
- Audit trail is **append-only** (no update/delete) for forensics.

## Check-list for the roadmap
- [ ] OIDC (or SAML) provider + keep Argon2id fallback.
- [ ] Wire the full RBAC model (roles/permissions/groups/memberships) into route guards.
- [ ] Token TTL + refresh + revocation.
- [ ] Production TLS termination guidance (reverse proxy / Let's Encrypt).
- [ ] Signed downloads (Content-Digest is there; add optional signature for supply-chain trust).
