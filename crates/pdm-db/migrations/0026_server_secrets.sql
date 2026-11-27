-- Migration 0026: server-wide secrets the server makes for itself (not per organization).
--
--   * design_link: the HMAC key that signs the short-lived /public/design/... links the
--     SolidCraft web editor opens. pdm_db::server_secret creates it with random bytes on the
--     first start after this migration, so it needs no configuration and every API process on
--     this database shares it. Deleting the row (and restarting) replaces the key, which ends
--     every design link handed out before.
CREATE TABLE IF NOT EXISTS server_secrets (
    name TEXT PRIMARY KEY,
    secret BYTEA NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
