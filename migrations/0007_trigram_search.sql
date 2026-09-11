-- Search that actually finds things.
--
-- The previous index (items_fts, migration 0005) tokenised titles into whole
-- words, which made three common searches return nothing at all:
--
--   "cap"    -> no match, because "capri" is one token and there is no prefix
--               matching, so nothing appears until you finish typing a token
--   "flower" -> no match for "Sunflower", because a token only matches from
--               its start
--   "logic"  -> no match, because tag names were never indexed (0005 says so
--               in its own comment and defers it)
--
-- The middle one matters most for a gallery built around one running joke:
-- the titles are words with the joke's noun buried inside them, so the
-- substring nobody can search for is exactly the substring everybody types.
--
-- The trigram tokeniser fixes all three. It indexes every 3-character run, so
-- a query matches anywhere inside the text, prefix or infix, case-insensitively.
-- Verified against a live D1 database before writing this migration: "flower"
-- finds Sunflower, "apri" finds Capri-Sun, "cap" finds Capri-Sun, and an OR of
-- a misspelling's trigrams ranks the right item far above the noise
-- (sunflwer -> Sunflower at bm25 -1.43, everything else at -1e-6), which is
-- what search_items() uses as its fuzzy fallback.
--
-- Two deliberate differences from 0005:
--
-- 1. This is a regular FTS5 table, not external-content (content='items').
--    External content mode maps each FTS column onto a column of one table, and
--    tag names live in item_tags/tags behind a join, so they cannot be projected
--    that way. A regular table stores its own copy; titles are capped at 80
--    characters and tags are short, so the duplication is negligible. It also
--    means a plain DELETE works here, instead of the 'delete' command form that
--    external-content mode requires.
--
-- 2. items_fts is left in place, unused. Search moves over by code change alone,
--    so rolling back is reverting a commit rather than restoring an index.
CREATE VIRTUAL TABLE IF NOT EXISTS items_search USING fts5(
    title,
    tags,
    tokenize = 'trigram'
);

-- rowid is kept equal to items.rowid so results join straight back to the items
-- table, the same way the old index did.
CREATE TRIGGER IF NOT EXISTS items_search_ai AFTER INSERT ON items BEGIN
    INSERT INTO items_search (rowid, title, tags) VALUES (new.rowid, new.title, '');
END;

CREATE TRIGGER IF NOT EXISTS items_search_ad AFTER DELETE ON items BEGIN
    DELETE FROM items_search WHERE rowid = old.rowid;
END;

-- Re-reads the tags on update rather than blanking them: a title edit must not
-- silently drop a item out of tag search.
CREATE TRIGGER IF NOT EXISTS items_search_au AFTER UPDATE ON items BEGIN
    DELETE FROM items_search WHERE rowid = old.rowid;
    INSERT INTO items_search (rowid, title, tags)
    SELECT new.rowid,
           new.title,
           COALESCE((SELECT group_concat(t.name, ' ')
                     FROM item_tags st JOIN tags t ON t.id = st.tag_id
                     WHERE st.item_id = new.id), '');
END;

-- Tags are attached after the item row exists, so the item's indexed tag text has
-- to be rebuilt whenever the join table changes. Full recompute rather than
-- appending one name: removing a tag has to shrink the text too.
CREATE TRIGGER IF NOT EXISTS item_tags_search_ai AFTER INSERT ON item_tags BEGIN
    DELETE FROM items_search WHERE rowid = (SELECT rowid FROM items WHERE id = new.item_id);
    INSERT INTO items_search (rowid, title, tags)
    SELECT s.rowid,
           s.title,
           COALESCE((SELECT group_concat(t.name, ' ')
                     FROM item_tags st JOIN tags t ON t.id = st.tag_id
                     WHERE st.item_id = s.id), '')
    FROM items s WHERE s.id = new.item_id;
END;

CREATE TRIGGER IF NOT EXISTS item_tags_search_ad AFTER DELETE ON item_tags BEGIN
    DELETE FROM items_search WHERE rowid = (SELECT rowid FROM items WHERE id = old.item_id);
    INSERT INTO items_search (rowid, title, tags)
    SELECT s.rowid,
           s.title,
           COALESCE((SELECT group_concat(t.name, ' ')
                     FROM item_tags st JOIN tags t ON t.id = st.tag_id
                     WHERE st.item_id = s.id), '')
    FROM items s WHERE s.id = old.item_id;
END;

-- Backfill every existing item, titles and tags together. DELETE first so this
-- migration is safe to re-run against a database that already has the table.
DELETE FROM items_search;

INSERT INTO items_search (rowid, title, tags)
SELECT s.rowid,
       s.title,
       COALESCE((SELECT group_concat(t.name, ' ')
                 FROM item_tags st JOIN tags t ON t.id = st.tag_id
                 WHERE st.item_id = s.id), '')
FROM items s;
