-- Per-client app settings, one row per user (SolidCraft's Prefs JSON).
CREATE TABLE IF NOT EXISTS client_settings (
    user_id uuid PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
    payload jsonb NOT NULL,
    updated_at timestamptz NOT NULL DEFAULT now()
);
