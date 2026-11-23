-- Split the version number (previously every save got a new `seq`) into a
-- major `version` (bumped only on an explicit "save as new version") and a
-- per-save `revision` within it. `seq` stays as the global monotonic order.
ALTER TABLE file_versions ADD COLUMN IF NOT EXISTS version INTEGER NOT NULL DEFAULT 1;
ALTER TABLE file_versions ADD COLUMN IF NOT EXISTS revision INTEGER NOT NULL DEFAULT 1;

-- Every past save was "a new version": keep its seq as the version number,
-- each starting at revision 1 (so the next plain save becomes V{seq}-R2).
UPDATE file_versions SET version = seq;
