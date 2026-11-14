-- Migration 0005: manufacturing registry — machines, post-processors, and the tool library.
-- Reference data for the Part -> ... -> Machine -> Production Job -> Inspection chain
-- (docs/manufacturing.md). Org-scoped like every other catalog table.

CREATE TABLE IF NOT EXISTS machines (
    id                 UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id             UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    name               TEXT NOT NULL,
    kind               TEXT NOT NULL DEFAULT 'cnc_mill' CHECK (kind IN
        ('cnc_mill','cnc_lathe','cnc_router','laser_cutter','plasma_cutter','waterjet','printer','manual')),
    manufacturer       TEXT,
    model              TEXT,
    controller         TEXT,
    capabilities       JSONB,          -- e.g. { "max_spindle_speed": 24000, "tool_changes": 32 }
    work_envelope      JSONB,          -- e.g. { "x_mm": 300, "y_mm": 200, "z_mm": 125 }
    axes               TEXT[] NOT NULL DEFAULT '{}',
    materials          TEXT[] NOT NULL DEFAULT '{}',
    maintenance_status TEXT,           -- e.g. "ok" | "due" | "overdue"
    created_at         TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at         TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (org_id, name)
);
CREATE INDEX IF NOT EXISTS idx_machines_org ON machines (org_id);

CREATE TABLE IF NOT EXISTS post_processors (
    id           UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id       UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    name         TEXT NOT NULL,
    version      TEXT,
    dialect      TEXT,                 -- e.g. "gcode", "rs274ngc"
    machine_kind TEXT,                 -- which machine kind it targets (NULL: any)
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (org_id, name)
);
CREATE INDEX IF NOT EXISTS idx_post_processors_org ON post_processors (org_id);

CREATE TABLE IF NOT EXISTS tools (
    id                       UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id                   UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    name                     TEXT NOT NULL,
    kind                     TEXT NOT NULL DEFAULT 'end_mill' CHECK (kind IN
        ('end_mill','drill','tap','reamer','face_mill','chamfer','turning','holder','insert')),
    dimensions               JSONB,    -- e.g. { "diameter_mm": 6.0, "flutes": 3, "overall_length_mm": 38 }
    cutting_params           JSONB,    -- e.g. { "sfm": 90, "feed_per_tooth_mm": 0.1 }
    material_compatibility   TEXT[] NOT NULL DEFAULT '{}',
    inventory                INTEGER NOT NULL DEFAULT 0,
    tool_life                JSONB,    -- e.g. { "max_passes": 5000, "used_passes": 120 }
    created_at               TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at               TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (org_id, name)
);
CREATE INDEX IF NOT EXISTS idx_tools_org ON tools (org_id);
