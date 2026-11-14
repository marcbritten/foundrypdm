-- Derived CAD: when a version's file is in a format `solidcraft-cli convert` reads
-- (Inventor .ipt/.iam, FreeCAD .fcstd, Rhino .3dm, STEP, IGES), the worker's
-- `cad.convert` job restates it as STEP AP214 and stores that as a content-addressed
-- blob, recorded here. The portal and SolidCraft can then open or download the part
-- without the original file; the browser app (which does not read those native
-- formats itself) previews the converted STEP.
ALTER TABLE file_versions ADD COLUMN IF NOT EXISTS derived_step_sha256 TEXT;
ALTER TABLE file_versions ADD COLUMN IF NOT EXISTS derived_step_size_bytes BIGINT;
