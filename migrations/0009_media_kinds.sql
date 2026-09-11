-- Video and GIF support.
--
-- `kind` is what the original file is: 'image' (a PNG re-encoded from whatever
-- came in), 'gif' (stored byte-for-byte, because re-encoding an animated GIF as
-- PNG would keep one frame and throw the joke away) or 'video' (mp4 or webm,
-- stored as uploaded -- nothing in the container transcodes). Every row written
-- before this migration is an image, which the default says.
ALTER TABLE items ADD COLUMN kind TEXT NOT NULL DEFAULT 'image';

-- Seconds, videos only. NULL for anything that does not play.
ALTER TABLE items ADD COLUMN duration REAL;

-- A full-size JPEG still, videos only. The thumbnail is derived from it, but a
-- 480px thumbnail is too small to be an og:image or a <video poster>, so the
-- full frame is kept too. NULL for images and GIFs, whose original is its own
-- poster.
ALTER TABLE items ADD COLUMN poster_url TEXT;

-- The "items" the gallery sort can filter by kind on, if that is ever wanted.
CREATE INDEX IF NOT EXISTS idx_items_public_kind_created
    ON items (is_public, kind, created_at DESC);
