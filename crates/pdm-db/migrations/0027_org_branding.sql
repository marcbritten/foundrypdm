-- Migration 0027: organization branding images.
-- The display name and color overrides live in organizations.settings->'branding' like the
-- other org settings; only the image bytes need a table. They are small (the API caps them at
-- 256 KiB) and are read by the unauthenticated sign-in page, so they live here rather than in
-- the blob store, whose keys are global SHA-256s with no organization scope.
-- kind 'logo' is the main logo; 'icon' is reserved for a separate browser-tab icon later.
CREATE TABLE IF NOT EXISTS org_branding_assets (
    org_id     UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    kind       TEXT NOT NULL CHECK (kind IN ('logo', 'icon')),
    mime       TEXT NOT NULL CHECK (mime IN ('image/png', 'image/webp')),
    bytes      BYTEA NOT NULL CHECK (octet_length(bytes) <= 262144),
    sha256     TEXT NOT NULL,
    width      INTEGER NOT NULL CHECK (width BETWEEN 16 AND 2048),
    height     INTEGER NOT NULL CHECK (height BETWEEN 16 AND 2048),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_by UUID REFERENCES users(id) ON DELETE SET NULL,
    PRIMARY KEY (org_id, kind)
);
