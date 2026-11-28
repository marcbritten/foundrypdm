-- Operator role, between viewer and editor: operators record what happens on the shop floor
-- (production and print job status, inspections). Everything else stays editor or admin.
ALTER TABLE users DROP CONSTRAINT IF EXISTS users_role_check;
ALTER TABLE users ADD CONSTRAINT users_role_check
    CHECK (role IN ('viewer', 'operator', 'editor', 'admin'));
