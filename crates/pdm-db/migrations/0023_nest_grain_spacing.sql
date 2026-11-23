-- Migration 0023: directional part spacing, grain directions, and per-line
-- mirror options for laser nests.
--
--   * spacing_x_mm / spacing_y_mm replace the single part_gap_mm for new nests
--     (horizontal vs vertical part spacing). Existing nests keep part_gap_mm,
--     which is back-filled as both spacings.
--   * grain_direction (nest level): the sheet's grain runs along the stock
--     length (the vertical axis in PDM layouts) or across it (the width).
--   * part_grain (per part line): whether that part's grain should run along
--     or across its own long axis. Together these drive the allowed rotations
--     so part grain aligns with sheet grain.
--   * mirror (per part line): bake a mirror of the original geometry into the
--     line (e.g. cutting the opposite face). allow_mirror (per part line):
--     permit the solver to flip copies of the line.
ALTER TABLE laser_nests
    ADD COLUMN IF NOT EXISTS spacing_x_mm DOUBLE PRECISION NOT NULL DEFAULT 0,
    ADD COLUMN IF NOT EXISTS spacing_y_mm DOUBLE PRECISION NOT NULL DEFAULT 0,
    ADD COLUMN IF NOT EXISTS grain_direction TEXT NOT NULL DEFAULT 'along_length'
        CHECK (grain_direction IN ('along_length','along_width','any'));
UPDATE laser_nests SET spacing_x_mm = part_gap_mm, spacing_y_mm = part_gap_mm
WHERE spacing_x_mm = 0 AND spacing_y_mm = 0 AND part_gap_mm > 0;

ALTER TABLE laser_nest_parts
    ADD COLUMN IF NOT EXISTS mirror BOOLEAN NOT NULL DEFAULT FALSE,
    ADD COLUMN IF NOT EXISTS allow_mirror BOOLEAN NOT NULL DEFAULT TRUE,
    ADD COLUMN IF NOT EXISTS part_grain TEXT NOT NULL DEFAULT 'any'
        CHECK (part_grain IN ('along_length','along_width','any')),
    ADD COLUMN IF NOT EXISTS ordinal INTEGER NOT NULL DEFAULT 0;
