-- Migration 0020: rich tool library — General / Cutter / Cutting data / Post processor.
-- Extends the existing tools registry with the fields the portal editor needs,
-- plus per-material cutting-data sets and attached documents (content-addressed
-- blobs, like item files).

ALTER TABLE tools ADD COLUMN IF NOT EXISTS description TEXT;
ALTER TABLE tools ADD COLUMN IF NOT EXISTS vendor TEXT;
ALTER TABLE tools ADD COLUMN IF NOT EXISTS part_number TEXT;
ALTER TABLE tools ADD COLUMN IF NOT EXISTS link TEXT;
ALTER TABLE tools ADD COLUMN IF NOT EXISTS units TEXT NOT NULL DEFAULT 'mm'
    CHECK (units IN ('mm','in'));
ALTER TABLE tools ADD COLUMN IF NOT EXISTS rotation TEXT NOT NULL DEFAULT 'cw'
    CHECK (rotation IN ('cw','ccw'));
ALTER TABLE tools ADD COLUMN IF NOT EXISTS flutes INTEGER;
ALTER TABLE tools ADD COLUMN IF NOT EXISTS tool_material TEXT;
ALTER TABLE tools ADD COLUMN IF NOT EXISTS tool_number INTEGER;

-- One cutting-data set per (tool, material, finish). All values are in the
-- tool's units (mm/min or in/min, rpm, degrees).
CREATE TABLE IF NOT EXISTS tool_cutting_data (
    id                    UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tool_id               UUID NOT NULL REFERENCES tools(id) ON DELETE CASCADE,
    label                 TEXT,
    material              TEXT,
    finish                TEXT,
    spindle_speed         DOUBLE PRECISION,
    surface_speed         DOUBLE PRECISION,
    ramp_spindle_speed    DOUBLE PRECISION,
    cutting_feedrate      DOUBLE PRECISION,
    feed_per_tooth        DOUBLE PRECISION,
    lead_in_feedrate      DOUBLE PRECISION,
    lead_out_feedrate     DOUBLE PRECISION,
    transition_feedrate   DOUBLE PRECISION,
    ramp_feedrate         DOUBLE PRECISION,
    ramp_angle            DOUBLE PRECISION,
    plunge_feedrate       DOUBLE PRECISION,
    plunge_feed_per_rev   DOUBLE PRECISION,
    created_at            TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at            TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS idx_tool_cutting_data_tool ON tool_cutting_data (tool_id);

-- Documents attached to a tool (datasheets, vendor pages, drawings, ...).
CREATE TABLE IF NOT EXISTS tool_documents (
    id             UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tool_id        UUID NOT NULL REFERENCES tools(id) ON DELETE CASCADE,
    filename       TEXT NOT NULL,
    blob_id        UUID NOT NULL REFERENCES file_blobs(id),
    size_bytes     BIGINT NOT NULL DEFAULT 0,
    content_type   TEXT,
    created_at     TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS idx_tool_documents_tool ON tool_documents (tool_id);
