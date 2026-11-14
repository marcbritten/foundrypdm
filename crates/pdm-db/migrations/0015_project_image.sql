-- Cover image for a project (data:image URL, set from the web workspace).
ALTER TABLE projects ADD COLUMN IF NOT EXISTS image_data TEXT;
