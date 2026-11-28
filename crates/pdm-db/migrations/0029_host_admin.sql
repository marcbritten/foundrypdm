-- Migration 0029: host admins and server-wide settings.
--
-- A server can hold several organizations, but some things are shared by all of them: the
-- CAD CLI and web editor builds on the host, the Dutchman release installed for every
-- conversion, and the GPU policy of the preview renderer on the shared worker. Until now any
-- org admin could change those for everyone. They now belong to host admins.
--
--   * users.is_host_admin: the earliest-created active admin of the organization created first
--     (the one whose Dutchman channel the host already followed) becomes the first host admin;
--     if that organization has no active admin, the earliest active admin on the server. On a
--     single-organization server that is its first admin, who keeps every right they had.
--     Host admins grant and revoke it for others; `pdm-cli host-admin` is the recovery path.
--   * host_settings: server-wide values. The Dutchman channel and the GPU policy start from
--     the organization created first, which is the channel the host update loop already
--     followed.
ALTER TABLE users ADD COLUMN IF NOT EXISTS is_host_admin BOOLEAN NOT NULL DEFAULT false;

UPDATE users SET is_host_admin = true
 WHERE id = (SELECT u.id FROM users u JOIN organizations o ON o.id = u.org_id
              WHERE u.role = 'admin' AND u.active
              ORDER BY o.created_at, o.id, u.created_at, u.id LIMIT 1)
   AND NOT EXISTS (SELECT 1 FROM users WHERE is_host_admin);

CREATE TABLE IF NOT EXISTS host_settings (
    key TEXT PRIMARY KEY,
    value JSONB NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

INSERT INTO host_settings (key, value)
SELECT 'dutchman_channel', to_jsonb(ch)
  FROM (SELECT btrim(settings->'dutchman'->>'channel') AS ch
          FROM organizations ORDER BY created_at LIMIT 1) first_org
 WHERE ch IS NOT NULL AND ch <> ''
ON CONFLICT (key) DO NOTHING;

INSERT INTO host_settings (key, value)
SELECT 'gpu_policy', to_jsonb(gp)
  FROM (SELECT settings->>'gpu_policy' AS gp
          FROM organizations ORDER BY created_at LIMIT 1) first_org
 WHERE gp IN ('auto', 'hardware', 'software', 'cpu')
ON CONFLICT (key) DO NOTHING;
