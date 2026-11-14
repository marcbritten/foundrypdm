-- Migration 0006: manufacturing execution chain.
-- Part(revision) -> Manufacturing Setup -> CAM Project -> NC Program -> Production Job -> Inspection.
-- NC programs pin the exact source CAD revision, setup, tool configuration and post-processor
-- version (docs/manufacturing.md "Critical Data Integrity").

CREATE TABLE IF NOT EXISTS mfg_setups (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id          UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    item_id         UUID NOT NULL REFERENCES items(id) ON DELETE CASCADE,
    file_version_id UUID,
    name            TEXT NOT NULL,
    stock           JSONB,        -- { "width_mm": 100, "height_mm": 50, "depth_mm": 10 }
    workholding     TEXT,
    wcs             TEXT,         -- work coordinate system, e.g. "G54"
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS idx_mfg_setups_org ON mfg_setups (org_id);
CREATE INDEX IF NOT EXISTS idx_mfg_setups_item ON mfg_setups (item_id);

CREATE TABLE IF NOT EXISTS cam_projects (
    id           UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id       UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    mfg_setup_id UUID NOT NULL REFERENCES mfg_setups(id) ON DELETE CASCADE,
    name         TEXT NOT NULL,
    source       TEXT,          -- "external" | "solidcraft" | ...
    tool_ids     UUID[] NOT NULL DEFAULT '{}',
    toolpaths    JSONB,
    feeds_speeds JSONB,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS idx_cam_projects_org ON cam_projects (org_id);

CREATE TABLE IF NOT EXISTS nc_programs (
    id                       UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id                   UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    item_id                  UUID NOT NULL REFERENCES items(id) ON DELETE CASCADE,
    file_version_id          UUID,        -- exact source CAD revision
    mfg_setup_id             UUID,
    cam_project_id           UUID,
    machine_id               UUID REFERENCES machines(id) ON DELETE SET NULL,
    post_processor_id        UUID REFERENCES post_processors(id) ON DELETE SET NULL,
    post_processor_version   TEXT,        -- pinned version (data integrity)
    gcode_sha256             TEXT,        -- the G-code blob (content-addressed)
    simulation_result        JSONB,
    released                 BOOLEAN NOT NULL DEFAULT FALSE,
    released_at              TIMESTAMPTZ,
    created_at               TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS idx_nc_programs_org ON nc_programs (org_id);
CREATE INDEX IF NOT EXISTS idx_nc_programs_item ON nc_programs (item_id);

CREATE TABLE IF NOT EXISTS production_jobs (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id          UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    nc_program_id   UUID NOT NULL REFERENCES nc_programs(id) ON DELETE CASCADE,
    item_id         UUID,
    machine_id      UUID REFERENCES machines(id) ON DELETE SET NULL,
    status          TEXT NOT NULL DEFAULT 'pending'
                    CHECK (status IN ('pending','in_progress','done','rejected','rework')),
    operator        TEXT,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    completed_at    TIMESTAMPTZ
);
CREATE INDEX IF NOT EXISTS idx_production_jobs_org ON production_jobs (org_id);

CREATE TABLE IF NOT EXISTS inspection_records (
    id                UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id            UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    production_job_id UUID REFERENCES production_jobs(id) ON DELETE CASCADE,
    item_id           UUID REFERENCES items(id) ON DELETE SET NULL,
    passed            BOOLEAN,
    notes             TEXT,
    measured          JSONB,        -- measured dimensions vs. spec
    created_at        TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS idx_inspection_org ON inspection_records (org_id);
