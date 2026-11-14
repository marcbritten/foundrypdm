-- A native SolidCraft download derived from an immutable source version.
-- Source blobs, version sequence numbers and item heads are never replaced.
ALTER TABLE file_versions ADD COLUMN derived_solidcraft_sha256 TEXT;
ALTER TABLE file_versions ADD COLUMN derived_solidcraft_size_bytes BIGINT;
ALTER TABLE file_versions ADD COLUMN derived_solidcraft_error TEXT;
ALTER TABLE file_versions ADD COLUMN derived_solidcraft_job_id UUID REFERENCES jobs(id) ON DELETE SET NULL;
