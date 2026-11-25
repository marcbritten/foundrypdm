-- Migration 0024: professional sheet/part setup options for laser nests.
--
-- Nest-level (laser_nests):
--   * nesting_corner — which corner the fill starts from (lower_left default).
--     Implemented as a whole-layout flip, so relative grain alignment is kept.
--   * origin_x/y_mm — offset of the nesting origin from the stock lower-left.
--   * sheet_sheet_distance_mm — reference gap between sheets (manifest/package).
--   * guillotine — request a guillotine-cuttable layout (preference, stored).
--   * add_part_labels — draw part labels (P1, P2, …) on the sheet DXF + preview.
--   * fill_direction, auto_sheet_origins, fill_all_sheets, optimized_pairs —
--     layout preferences, stored + surfaced in the manifest/preview.
--
-- Per-part-line (laser_nest_parts):
--   * step_angle — rotation increment in degrees (90 => 0/90/180/270; 45 => 8
--     orientations; 180 => 0/180). Grain may further restrict these.
--   * priority — nesting priority (lower number placed first).
--   * label — the part label drawn when add_part_labels is on.
--   * use_bounding_box — nest by the part's bounding box instead of its outline.
--   * common_edge — request common-edge alignment (preference, stored).
--   * min_qty / filler_qty — minimum placed quantity / extra filler copies
--     (preferences, stored; quantity remains the primary demand).
ALTER TABLE laser_nests
    ADD COLUMN IF NOT EXISTS nesting_corner TEXT NOT NULL DEFAULT 'lower_left'
        CHECK (nesting_corner IN ('lower_left','lower_right','upper_left','upper_right')),
    ADD COLUMN IF NOT EXISTS origin_x_mm DOUBLE PRECISION NOT NULL DEFAULT 0,
    ADD COLUMN IF NOT EXISTS origin_y_mm DOUBLE PRECISION NOT NULL DEFAULT 0,
    ADD COLUMN IF NOT EXISTS sheet_sheet_distance_mm DOUBLE PRECISION NOT NULL DEFAULT 0,
    ADD COLUMN IF NOT EXISTS guillotine BOOLEAN NOT NULL DEFAULT FALSE,
    ADD COLUMN IF NOT EXISTS add_part_labels BOOLEAN NOT NULL DEFAULT FALSE,
    ADD COLUMN IF NOT EXISTS fill_direction TEXT NOT NULL DEFAULT 'auto',
    ADD COLUMN IF NOT EXISTS auto_sheet_origins BOOLEAN NOT NULL DEFAULT TRUE,
    ADD COLUMN IF NOT EXISTS fill_all_sheets BOOLEAN NOT NULL DEFAULT FALSE,
    ADD COLUMN IF NOT EXISTS optimized_pairs BOOLEAN NOT NULL DEFAULT FALSE;

ALTER TABLE laser_nest_parts
    ADD COLUMN IF NOT EXISTS step_angle DOUBLE PRECISION NOT NULL DEFAULT 90,
    ADD COLUMN IF NOT EXISTS priority INTEGER NOT NULL DEFAULT 1,
    ADD COLUMN IF NOT EXISTS label TEXT NOT NULL DEFAULT '',
    ADD COLUMN IF NOT EXISTS use_bounding_box BOOLEAN NOT NULL DEFAULT FALSE,
    ADD COLUMN IF NOT EXISTS common_edge BOOLEAN NOT NULL DEFAULT FALSE,
    ADD COLUMN IF NOT EXISTS min_qty INTEGER NOT NULL DEFAULT 0,
    ADD COLUMN IF NOT EXISTS filler_qty INTEGER NOT NULL DEFAULT 0;
