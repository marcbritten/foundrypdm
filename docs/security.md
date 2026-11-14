# Security

## Authentication
- **Bearer API tokens** (Phase 1). `POST /auth/login` exchanges `username` + `password` for a token;
  tokens are stored SHA-256-hashed in `api_tokens`. On a 401 the client re-authenticates and refreshes.
- **Passwords:** Argon2id (`m=19MiB, t=2, p=1`). Optional (users can be token-only, e.g. via
  `service_accounts` / `pdm-cli create-token`).
- **OIDC** is a later hardening item (provider TBD; keep Argon2id as fallback).

## Authorization
- **RBAC** via `users.role` (`viewer` | `editor` | `admin`) in Phase 1. The full
  `roles`/`permissions`/`groups`/`memberships` model is in the schema for the later phase.
- **Org isolation** is enforced in **every** query (`org_id` in the WHERE clause), not by convention —
  a user of one org cannot read another org's items/blobs/versions.
- Service accounts (`service_accounts`) for headless/non-user credentials.

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
- Token expiry/revocation policy (currently long-lived until logout/rotation).
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
