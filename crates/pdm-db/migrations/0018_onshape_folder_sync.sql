CREATE TABLE IF NOT EXISTS onshape_folder_sync (
    project_id UUID PRIMARY KEY REFERENCES projects(id) ON DELETE CASCADE,
    org_id UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    folder_id TEXT NOT NULL,
    folder_name TEXT NOT NULL DEFAULT '',
    status TEXT NOT NULL DEFAULT 'idle',
    result JSONB NOT NULL DEFAULT '{}'::jsonb,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
