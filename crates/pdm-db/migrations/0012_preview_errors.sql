-- Preview errors: when the worker's `preview.render` job cannot produce a
-- thumbnail for a version (the CLI could not open or convert the file), it
-- records the reason here so the portal shows "why" instead of an empty
-- preview that looks like it is still rendering. NULL = no error recorded.
ALTER TABLE file_versions ADD COLUMN IF NOT EXISTS thumbnail_error TEXT;
