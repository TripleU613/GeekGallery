-- Where an imported item came from: the page URL that was pasted into the
-- upload form (a tweet, a reel, a YouTube watch page, or a bare media URL).
-- NULL for a file upload. Shown on the item's page as a "via <host>" link,
-- which is the attribution the source deserves and the first place a takedown
-- request needs to look.
ALTER TABLE items ADD COLUMN source_url TEXT;
