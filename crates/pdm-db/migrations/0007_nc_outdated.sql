-- Migration 0007: flag NC programs whose source design changed (data integrity).
-- When a new version of a part's design is checked in, the item's existing NC programs that
-- were based on an older revision are marked potentially-outdated (not auto-deleted).
ALTER TABLE nc_programs ADD COLUMN IF NOT EXISTS outdated BOOLEAN NOT NULL DEFAULT FALSE;
