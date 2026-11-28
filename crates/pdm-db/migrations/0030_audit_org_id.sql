-- Live updates read the audit log by id within an organization: the newest id (the catch-up
-- cursor) and the rows after a given id (GET /audit?since=, GET /events/stream?since=).
CREATE INDEX IF NOT EXISTS idx_audit_org_id ON audit_events(org_id, id);
