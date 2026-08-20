-- A successful poll turn for one feed.
--
-- `last_error` / `last_error_at` are deliberately left untouched, so what a
-- now-healthy feed was last failing with survives its recovery — an operator
-- arriving after a flap can still see what happened. Read them together with
-- `status`: a row whose `status` is 'ok' and whose `last_error` is set has
-- recovered, it has not failed.
INSERT INTO feed_health (
    feed,
    status,
    last_ok_at,
    last_records,
    caught_up,
    ok_count,
    updated_at
)
VALUES ($1, 'ok', $2, $3, $4, 1, $2)
ON CONFLICT (feed) DO UPDATE SET
    status = 'ok',
    last_ok_at = EXCLUDED.last_ok_at,
    last_records = EXCLUDED.last_records,
    caught_up = EXCLUDED.caught_up,
    ok_count = feed_health.ok_count + 1,
    updated_at = EXCLUDED.updated_at;
