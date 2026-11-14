-- Migration 0002: rendered model thumbnails.
-- Each file version may record a PNG preview rendered by the worker via
-- `solvecraft-cli snapshot`. The PNG itself lives in the content-addressed
-- blob store; here we only keep its SHA-256 and byte size.

ALTER TABLE file_versions ADD COLUMN IF NOT EXISTS thumbnail_sha256 TEXT;
ALTER TABLE file_versions ADD COLUMN IF NOT EXISTS thumbnail_size_bytes BIGINT;
