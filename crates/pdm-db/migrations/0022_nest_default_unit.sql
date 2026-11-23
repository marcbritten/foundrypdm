-- Migration 0022: nest-level default part source unit.
--
-- Parts resolve their DXF source unit as: per-part override > file $INSUNITS >
-- this nest default > mm. The default lets an organization whose drawings are
-- in inches nest correctly even when a DXF declares no $INSUNITS.
ALTER TABLE laser_nests ADD COLUMN IF NOT EXISTS default_part_unit TEXT NOT NULL DEFAULT 'mm';
