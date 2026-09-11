-- Structured reports, replacing the bare counter's lack of detail. items.reports
-- stays (auto-hide at AUTO_HIDE_REPORTS still reads it) but is now recomputed
-- from COUNT(*) here, same self-healing pattern as items.likes.
CREATE TABLE IF NOT EXISTS reports (
    item_id     TEXT NOT NULL REFERENCES items(id) ON DELETE CASCADE,
    voter_id   TEXT NOT NULL,
    -- 'not_item' | 'spam' | 'porn' | 'stolen'. Not an enforced CHECK constraint:
    -- new reasons should be addable without a migration, and the UI is the
    -- only thing that ever writes this value.
    reason     TEXT NOT NULL,
    message    TEXT,
    created_at TEXT NOT NULL,
    -- One report per voter per item: a report is a distinct signal, and without
    -- this a single voter could submit repeatedly to force auto-hide alone.
    PRIMARY KEY (item_id, voter_id)
);

CREATE INDEX IF NOT EXISTS idx_reports_item ON reports (item_id);
