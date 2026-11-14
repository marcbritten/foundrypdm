-- Migration 0008: 3D printing jobs + sheet-metal specs (remaining MFG/CAM domains).

CREATE TABLE IF NOT EXISTS print_jobs (
    id                 UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id             UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    item_id            UUID NOT NULL REFERENCES items(id) ON DELETE CASCADE,
    file_version_id    UUID,
    printer_id         UUID REFERENCES machines(id) ON DELETE SET NULL,   -- machine kind = 'printer'
    material           TEXT,
    slicer_profile     TEXT,
    orientation        TEXT,
    supports           BOOLEAN NOT NULL DEFAULT FALSE,
    layer_height_mm    DOUBLE PRECISION,
    estimated_minutes  INTEGER,
    machine_file_sha   TEXT,          -- sliced output (3MF / G-code) content-addressed blob
    status             TEXT NOT NULL DEFAULT 'queued'
                       CHECK (status IN ('queued','printing','done','failed')),
    created_at         TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS idx_print_jobs_org ON print_jobs (org_id);
CREATE INDEX IF NOT EXISTS idx_print_jobs_item ON print_jobs (item_id);

CREATE TABLE IF NOT EXISTS sheet_metal_specs (
    id                 UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id             UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    item_id            UUID NOT NULL REFERENCES items(id) ON DELETE CASCADE,
    file_version_id    UUID,
    material           TEXT,
    thickness_mm       DOUBLE PRECISION,
    bend_radius_mm     DOUBLE PRECISION,
    k_factor           DOUBLE PRECISION,
    bend_table         JSONB,          -- per-thickness bend deductions
    flat_pattern_sha   TEXT,           -- generated flat-pattern DXF (content-addressed) or NULL
    cutting_operation  TEXT,
    forming_operation  TEXT,
    created_at         TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at         TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS idx_sheet_metal_org ON sheet_metal_specs (org_id);
CREATE INDEX IF NOT EXISTS idx_sheet_metal_item ON sheet_metal_specs (item_id);
