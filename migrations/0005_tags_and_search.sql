CREATE TABLE IF NOT EXISTS tags (
    id   TEXT PRIMARY KEY NOT NULL,
    name TEXT NOT NULL UNIQUE,
    slug TEXT NOT NULL UNIQUE
);

CREATE TABLE IF NOT EXISTS item_tags (
    item_id TEXT NOT NULL REFERENCES items(id) ON DELETE CASCADE,
    tag_id TEXT NOT NULL REFERENCES tags(id) ON DELETE CASCADE,
    PRIMARY KEY (item_id, tag_id)
);

CREATE INDEX IF NOT EXISTS idx_item_tags_tag ON item_tags (tag_id);

-- Real full-text search over titles, not a LIKE '%term%' scan. External-
-- content mode (content='items') so the indexed text isn't duplicated in the
-- FTS table -- it stays a pointer into items.title, kept in sync by the
-- triggers below. Scoped to title only for now; folding in tag names needs
-- triggers on item_tags too and is a reasonable follow-up, not a blocker here.
CREATE VIRTUAL TABLE IF NOT EXISTS items_fts USING fts5(
    title,
    content = 'items',
    content_rowid = 'rowid'
);

-- Keeps items_fts in sync with items.title. The 'delete' form (an FTS5 command,
-- not a plain DELETE) is what external-content mode requires for removing an
-- entry -- verified directly against D1 before relying on it here.
CREATE TRIGGER IF NOT EXISTS items_fts_ai AFTER INSERT ON items BEGIN
    INSERT INTO items_fts (rowid, title) VALUES (new.rowid, new.title);
END;

CREATE TRIGGER IF NOT EXISTS items_fts_ad AFTER DELETE ON items BEGIN
    INSERT INTO items_fts (items_fts, rowid, title) VALUES ('delete', old.rowid, old.title);
END;

CREATE TRIGGER IF NOT EXISTS items_fts_au AFTER UPDATE ON items BEGIN
    INSERT INTO items_fts (items_fts, rowid, title) VALUES ('delete', old.rowid, old.title);
    INSERT INTO items_fts (rowid, title) VALUES (new.rowid, new.title);
END;

-- Backfill: rows inserted before this migration existed have no FTS entry
-- yet, since the triggers above only fire on future writes.
INSERT INTO items_fts (rowid, title) SELECT rowid, title FROM items;
