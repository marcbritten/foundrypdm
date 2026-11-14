-- Designate the main assembly for a project: the root item the BOM is built
-- off, and the source of the project's cover image (its rendered thumbnail).
ALTER TABLE projects ADD COLUMN IF NOT EXISTS main_assembly_id UUID
  REFERENCES items(id) ON DELETE SET NULL;
CREATE INDEX IF NOT EXISTS projects_main_assembly_id_idx
  ON projects(main_assembly_id);
