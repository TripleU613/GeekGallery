-- Google sign-in. Additive: upload stays anonymous-friendly, this only adds
-- attribution and admin gating for accounts that choose to log in.
CREATE TABLE IF NOT EXISTS users (
    id           TEXT    PRIMARY KEY NOT NULL,
    google_sub   TEXT    NOT NULL UNIQUE,
    email        TEXT    NOT NULL,
    display_name TEXT    NOT NULL,
    avatar_url   TEXT,
    is_admin     INTEGER NOT NULL DEFAULT 0,
    created_at   TEXT    NOT NULL
);

-- NULL for anonymous uploads, exactly as before this migration. Nothing about
-- existing rows changes; this only gives new uploads somewhere to record who
-- made them, when they're logged in.
ALTER TABLE items ADD COLUMN uploader_id TEXT REFERENCES users(id);

CREATE INDEX IF NOT EXISTS idx_items_uploader ON items (uploader_id);
