-- Framework-owned cursor store (docs/data-feeds.md §3). One row per feed,
-- keyed by `Source::name`, holding that source's opaque JSON resume position
-- (a CEX feed stores `{ "next_start": <epoch> }`, an RPC feed a signature or
-- slot). The store sink upserts this after each committed batch; a poll source
-- reads it at startup to resume. A forward-only (live) feed never writes here.
CREATE TABLE IF NOT EXISTS feed_cursors (
    feed       TEXT        PRIMARY KEY,
    cursor     JSONB       NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
