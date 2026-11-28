-- Each person's own sidebar menu: the order, groups, names, icons and hidden links they chose, and
-- whether opening a group closes their other groups. No row, or no items, means the default menu.
-- What a person can open does not depend on this table.
CREATE TABLE IF NOT EXISTS user_menu (
    user_id uuid PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
    items jsonb,
    close_others boolean NOT NULL DEFAULT false,
    updated_at timestamptz NOT NULL DEFAULT now()
);
