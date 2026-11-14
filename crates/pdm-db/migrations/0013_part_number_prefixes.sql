-- PDM migration 0013: configurable part-number prefixes per category.
-- Maps a category (e.g. "Drain Tray") to a prefix (e.g. "DT") used when auto-generating
-- part numbers like "DT-000001". Org-scoped; only admins maintain the table.
CREATE TABLE IF NOT EXISTS part_number_prefixes (
    id         UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id     UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    category   TEXT NOT NULL,
    prefix     TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (org_id, category),
    UNIQUE (org_id, prefix)
);

CREATE INDEX IF NOT EXISTS idx_part_number_prefixes_org ON part_number_prefixes(org_id);

-- Seed sensible defaults so new orgs are usable out of the box.
INSERT INTO part_number_prefixes (org_id, category, prefix)
SELECT o.id, 'Drain Tray', 'DT'
FROM organizations o
ON CONFLICT (org_id, category) DO NOTHING;

INSERT INTO part_number_prefixes (org_id, category, prefix)
SELECT o.id, 'Glycol Unit', 'GL'
FROM organizations o
ON CONFLICT (org_id, category) DO NOTHING;

INSERT INTO part_number_prefixes (org_id, category, prefix)
SELECT o.id, 'Shaft Accessory', 'SA'
FROM organizations o
ON CONFLICT (org_id, category) DO NOTHING;
