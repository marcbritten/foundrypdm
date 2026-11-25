-- Migration 0025: multiple sheet definitions per laser nest.
--
-- A nest can now stack sheet *definitions* of differing sizes: the worker
-- works through them in order, filling each up to its quantity (null = as
-- many as needed) until all part lines are placed. `sheet_defs` is an empty
-- array for legacy nests, which continue to use stock_width_mm/
-- stock_height_mm (the primary sheet) for a single-size run.
ALTER TABLE laser_nests
    ADD COLUMN IF NOT EXISTS sheet_defs JSONB NOT NULL DEFAULT '[]';
