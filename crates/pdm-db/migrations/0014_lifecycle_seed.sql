-- PDM migration 0014: lifecycle state machine seed + approval assignees + org settings.
--
-- Seeds one lifecycle definition ("Standard") per org with states
--   wip -> in_review -> released -> obsolete
-- and transitions wip→in_review→released→obsolete. Items carry their state as a
-- denormalized name in items.lifecycle_state; PATCH /items/{id}/lifecycle validates
-- a requested flip against the org's definition before writing it.
--
-- Also adds `assignees` (JSONB uuid array) to approval_requests so release() can
-- route an approval to specific people, and `settings` (JSONB) on organizations
-- for org-scoped flags (e.g. require_approval_on_release).
--
-- Every statement is idempotent: the migration runner applies each migration once,
-- but the seed guards also survive a re-run.

ALTER TABLE organizations ADD COLUMN IF NOT EXISTS settings JSONB NOT NULL DEFAULT '{}';

ALTER TABLE approval_requests ADD COLUMN IF NOT EXISTS assignees JSONB NOT NULL DEFAULT '[]'::jsonb;

-- One lifecycle definition per org.
WITH def AS (
    INSERT INTO lifecycle_definitions (org_id, name)
    SELECT o.id, 'Standard'
    FROM organizations o
    WHERE NOT EXISTS (
        SELECT 1 FROM lifecycle_definitions d WHERE d.org_id = o.id
    )
    RETURNING id, org_id
)
INSERT INTO lifecycle_states (definition_id, name, is_released, is_terminal)
SELECT d.id, s.name, s.is_released, s.is_terminal
FROM def d
CROSS JOIN (VALUES
    ('wip',         FALSE, FALSE),
    ('in_review',   FALSE, FALSE),
    ('released',    TRUE,  FALSE),
    ('obsolete',    FALSE, TRUE )
) AS s(name, is_released, is_terminal)
WHERE NOT EXISTS (
    SELECT 1 FROM lifecycle_states st
    WHERE st.definition_id = d.id AND st.name = s.name
);

-- Transitions: wip -> in_review -> released -> obsolete (idempotent per definition).
INSERT INTO lifecycle_transitions (definition_id, from_state, to_state)
SELECT d.id, fs.id, ts.id
FROM (
    SELECT o.id AS org_id, d.id AS id
    FROM organizations o
    JOIN lifecycle_definitions d ON d.org_id = o.id AND d.name = 'Standard'
) d
JOIN lifecycle_states fs ON fs.definition_id = d.id AND fs.name = 'wip'
JOIN lifecycle_states ts ON ts.definition_id = d.id AND ts.name = 'in_review'
WHERE NOT EXISTS (
    SELECT 1 FROM lifecycle_transitions t
    WHERE t.definition_id = d.id AND t.from_state = fs.id AND t.to_state = ts.id
);

INSERT INTO lifecycle_transitions (definition_id, from_state, to_state)
SELECT d.id, fs.id, ts.id
FROM (
    SELECT o.id AS org_id, d.id AS id
    FROM organizations o
    JOIN lifecycle_definitions d ON d.org_id = o.id AND d.name = 'Standard'
) d
JOIN lifecycle_states fs ON fs.definition_id = d.id AND fs.name = 'in_review'
JOIN lifecycle_states ts ON ts.definition_id = d.id AND ts.name = 'released'
WHERE NOT EXISTS (
    SELECT 1 FROM lifecycle_transitions t
    WHERE t.definition_id = d.id AND t.from_state = fs.id AND t.to_state = ts.id
);

INSERT INTO lifecycle_transitions (definition_id, from_state, to_state)
SELECT d.id, fs.id, ts.id
FROM (
    SELECT o.id AS org_id, d.id AS id
    FROM organizations o
    JOIN lifecycle_definitions d ON d.org_id = o.id AND d.name = 'Standard'
) d
JOIN lifecycle_states fs ON fs.definition_id = d.id AND fs.name = 'released'
JOIN lifecycle_states ts ON ts.definition_id = d.id AND ts.name = 'obsolete'
WHERE NOT EXISTS (
    SELECT 1 FROM lifecycle_transitions t
    WHERE t.definition_id = d.id AND t.from_state = fs.id AND t.to_state = ts.id
);

CREATE INDEX IF NOT EXISTS idx_approval_requests_org_status ON approval_requests(org_id, status);
CREATE INDEX IF NOT EXISTS idx_approval_requests_assignees ON approval_requests USING GIN (assignees);
CREATE INDEX IF NOT EXISTS idx_approval_decisions_request ON approval_decisions(request_id);
CREATE INDEX IF NOT EXISTS idx_items_lifecycle_state ON items(org_id, lifecycle_state);
CREATE INDEX IF NOT EXISTS idx_file_locks_lease ON file_locks(lease_expires_at);
