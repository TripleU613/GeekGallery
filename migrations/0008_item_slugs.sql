-- Readable URLs: /item/sunflower instead of /item/211d65ac-7d21-4eb8-8ed2-....
--
-- A UUID in a shared link tells a human nothing and looks like spam in a group
-- chat, which is the main way anything here gets seen. The slug comes from the
-- title, so the URL says what it is.
--
-- Nullable, with no default: every row gets one backfilled below, and every new
-- row gets one at insert. Left nullable rather than NOT NULL because SQLite
-- cannot add a NOT NULL column without a default, and a default would be a lie
-- (there is no sensible generic slug).
ALTER TABLE items ADD COLUMN slug TEXT;

-- UNIQUE so two items can never share a slug: the lookup returns one row, and
-- without this constraint "which one" would depend on row order. Insert appends
-- -2, -3 on collision. Partial (WHERE slug IS NOT NULL) so any row that somehow
-- has no slug does not collide with every other such row -- in SQLite, NULLs are
-- distinct for uniqueness anyway, but being explicit documents the intent.
CREATE UNIQUE INDEX IF NOT EXISTS idx_items_slug ON items (slug) WHERE slug IS NOT NULL;

-- Backfill from the title, mirroring what slugify() in db.rs does: lowercase,
-- alphanumerics kept, every other run of characters collapsed to a single dash,
-- no leading or trailing dash. Done with nested REPLACEs because D1 has no
-- regexp_replace, and only for the characters titles actually contain (spaces
-- and hyphens) -- anything more exotic falls back to the id below.
UPDATE items
SET slug = TRIM(
        REPLACE(REPLACE(REPLACE(LOWER(title), ' ', '-'), '--', '-'), '--', '-'),
        '-'
    )
WHERE slug IS NULL
  AND title IS NOT NULL
  AND TRIM(title) <> '';

-- Anything still without one (blank title, or a title that reduced to nothing)
-- uses its id, so every item has a working URL.
UPDATE items SET slug = id WHERE slug IS NULL OR slug = '';
