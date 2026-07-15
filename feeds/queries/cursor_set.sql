-- Save a feed's resume position, overwriting any previous one.
INSERT INTO feed_cursors (feed, cursor, updated_at)
VALUES ($1, $2, now())
ON CONFLICT (feed) DO UPDATE
SET cursor = EXCLUDED.cursor,
    updated_at = now();
