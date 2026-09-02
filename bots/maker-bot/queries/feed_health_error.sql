-- A failed poll turn for one feed.
--
-- `last_ok_at` is left untouched: it is the column staleness is measured off,
-- and a feed that is failing must keep reporting the last time it actually
-- answered. Advancing any timestamp here on a failure is the bug this shape
-- exists to avoid — a single `updated_at` would keep looking fresh while the
-- feed was dead.
--
-- It is the FEED's staleness, though, and not a pair's. For a batched venue
-- `feed` is one name covering the whole roster, so `last_ok_at` advancing
-- means the request answered — not that every pair came back in it. A per-pair
-- alert built on this column is therefore silently wrong for exactly those
-- venues: the row stays fresh while one pair is dead. Read
-- `instrument_source_liveness` for the per-pair question.
INSERT INTO feed_health (
    feed,
    status,
    last_error_at,
    last_error,
    error_count,
    updated_at
)
VALUES ($1, 'error', $2, $3, 1, $2)
ON CONFLICT (feed) DO UPDATE SET
    status = 'error',
    last_error_at = EXCLUDED.last_error_at,
    last_error = EXCLUDED.last_error,
    error_count = feed_health.error_count + 1,
    updated_at = EXCLUDED.updated_at;
