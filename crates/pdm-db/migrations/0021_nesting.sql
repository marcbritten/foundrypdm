-- Migration 0021: laser nesting (Sparrow integration) + revision-bound flat patterns.
--
-- This supports the PDM's laser nesting workflow:
--   * flat_pattern_artifacts — immutable, revision-bound flat-pattern DXFs
--     (the existing sheet_metal_specs.flat_pattern_sha is a mutable pointer;
--     these pin the exact CAD version + generator + units a DXF came from).
--   * laser_nests / laser_nest_parts / laser_nest_sheets / laser_nest_placements
--     — a nesting run: its settings snapshot, the exact source parts/versions,
--     the per-sheet output DXF + preview blobs, and every placed copy's rigid
--     transform (stock coordinates).
--
-- Every table is org-scoped. Inputs are snapshotted at queue time and treated as
-- immutable; a later CAD revision or replaced flat pattern marks dependent nests
-- stale rather than rewriting their results.

-- ── Revision-bound flat-pattern artifacts ─────────────────────────────────
CREATE TABLE IF NOT EXISTS flat_pattern_artifacts (
    id               UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id           UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    item_id          UUID NOT NULL REFERENCES items(id) ON DELETE CASCADE,
    file_version_id  UUID NOT NULL REFERENCES file_versions(id) ON DELETE CASCADE,
    sha256           TEXT NOT NULL,
    size_bytes       BIGINT NOT NULL,
    source_kind      TEXT NOT NULL CHECK (source_kind IN ('generated','uploaded','version')),
    generator        TEXT NOT NULL DEFAULT '',
    unit             TEXT NOT NULL DEFAULT 'unknown'
                     CHECK (unit IN ('mm','inch','unknown')),
    notes            TEXT NOT NULL DEFAULT '',
    created_at       TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS idx_flat_pattern_artifacts_org ON flat_pattern_artifacts (org_id);
CREATE INDEX IF NOT EXISTS idx_flat_pattern_artifacts_item ON flat_pattern_artifacts (item_id, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_flat_pattern_artifacts_version ON flat_pattern_artifacts (file_version_id);

-- ── Nesting runs ──────────────────────────────────────────────────────────
CREATE TABLE IF NOT EXISTS laser_nests (
    id                 UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id             UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    project_id         UUID REFERENCES projects(id) ON DELETE SET NULL,
    name               TEXT NOT NULL,
    status             TEXT NOT NULL DEFAULT 'queued'
                       CHECK (status IN ('queued','running','completed','failed','cancelled')),
    machine_id         UUID REFERENCES machines(id) ON DELETE SET NULL,
    material           TEXT,
    thickness_mm       DOUBLE PRECISION,
    stock_width_mm     DOUBLE PRECISION NOT NULL,
    stock_height_mm    DOUBLE PRECISION NOT NULL,
    edge_margin_mm     DOUBLE PRECISION NOT NULL DEFAULT 0,
    part_gap_mm        DOUBLE PRECISION NOT NULL DEFAULT 0,
    chord_tol_mm       DOUBLE PRECISION NOT NULL DEFAULT 0.05,
    join_tol_mm        DOUBLE PRECISION NOT NULL DEFAULT 0.01,
    allowed_rotations  JSONB NOT NULL DEFAULT '[0,90,180,270]',
    time_budget_sec    INTEGER NOT NULL DEFAULT 600,
    attempt_limit_sec  INTEGER NOT NULL DEFAULT 60,
    max_sheets         INTEGER NOT NULL DEFAULT 16,
    seed               BIGINT,
    solver_version     TEXT,
    engine_build       TEXT,
    job_id             UUID,
    error              TEXT,
    manifest_sha       TEXT,
    stale              BOOLEAN NOT NULL DEFAULT FALSE,
    stale_reason       TEXT,
    parent_nest_id     UUID,
    created_by         UUID,
    created_at         TIMESTAMPTZ NOT NULL DEFAULT now(),
    completed_at       TIMESTAMPTZ
);
CREATE INDEX IF NOT EXISTS idx_laser_nests_org ON laser_nests (org_id, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_laser_nests_project ON laser_nests (project_id);
CREATE INDEX IF NOT EXISTS idx_laser_nests_status ON laser_nests (org_id, status);

-- ── Parts in a nest (exact source snapshots) ──────────────────────────────
CREATE TABLE IF NOT EXISTS laser_nest_parts (
    id               UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    nest_id          UUID NOT NULL REFERENCES laser_nests(id) ON DELETE CASCADE,
    org_id           UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    item_id          UUID NOT NULL REFERENCES items(id) ON DELETE CASCADE,
    file_version_id  UUID NOT NULL REFERENCES file_versions(id) ON DELETE CASCADE,
    artifact_id      UUID REFERENCES flat_pattern_artifacts(id) ON DELETE SET NULL,
    dxf_sha          TEXT NOT NULL,
    dxf_size         BIGINT NOT NULL,
    part_number      TEXT NOT NULL,
    revision_label   TEXT,
    quantity         INTEGER NOT NULL CHECK (quantity > 0),
    layer_cut        TEXT NOT NULL,
    layers_mark      JSONB NOT NULL DEFAULT '[]',
    unit             TEXT NOT NULL DEFAULT 'mm'
);
CREATE INDEX IF NOT EXISTS idx_laser_nest_parts_nest ON laser_nest_parts (nest_id);
CREATE INDEX IF NOT EXISTS idx_laser_nest_parts_org ON laser_nest_parts (org_id);
CREATE INDEX IF NOT EXISTS idx_laser_nest_parts_item ON laser_nest_parts (item_id);

-- ── Per-sheet outputs ─────────────────────────────────────────────────────
CREATE TABLE IF NOT EXISTS laser_nest_sheets (
    id                UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    nest_id           UUID NOT NULL REFERENCES laser_nests(id) ON DELETE CASCADE,
    org_id            UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    sheet_index       INTEGER NOT NULL CHECK (sheet_index >= 0),
    dxf_sha           TEXT,
    dxf_size          BIGINT,
    preview_sha       TEXT,
    strip_width_mm    DOUBLE PRECISION,
    utilization       DOUBLE PRECISION,
    solver_input_sha  TEXT,
    solver_output_sha TEXT,
    placed_count      INTEGER NOT NULL DEFAULT 0,
    UNIQUE (nest_id, sheet_index)
);
CREATE INDEX IF NOT EXISTS idx_laser_nest_sheets_org ON laser_nest_sheets (org_id);
CREATE INDEX IF NOT EXISTS idx_laser_nest_sheets_nest ON laser_nest_sheets (nest_id);

-- ── Every placed copy and its rigid transform (stock coordinates) ─────────
CREATE TABLE IF NOT EXISTS laser_nest_placements (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    sheet_id        UUID NOT NULL REFERENCES laser_nest_sheets(id) ON DELETE CASCADE,
    nest_id         UUID NOT NULL REFERENCES laser_nests(id) ON DELETE CASCADE,
    org_id          UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    part_index      INTEGER NOT NULL CHECK (part_index >= 0),
    copy_index      INTEGER NOT NULL CHECK (copy_index >= 0),
    item_id         UUID NOT NULL,
    file_version_id UUID NOT NULL,
    reflected       BOOLEAN NOT NULL DEFAULT FALSE,
    rotation_deg    DOUBLE PRECISION NOT NULL,
    tx              DOUBLE PRECISION NOT NULL,
    ty              DOUBLE PRECISION NOT NULL,
    UNIQUE (sheet_id, part_index, copy_index)
);
CREATE INDEX IF NOT EXISTS idx_laser_nest_placements_nest ON laser_nest_placements (nest_id);
CREATE INDEX IF NOT EXISTS idx_laser_nest_placements_org ON laser_nest_placements (org_id);
