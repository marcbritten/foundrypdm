-- Migration 0010: soft-delete (archive) support for projects and items.
-- `archived_at` is NULL for live rows; when set, the row is hidden from the
-- default listings but still addressable (hard-delete removes it for good).
ALTER TABLE projects ADD COLUMN IF NOT EXISTS archived_at TIMESTAMPTZ;
ALTER TABLE items ADD COLUMN IF NOT EXISTS archived_at TIMESTAMPTZ;
