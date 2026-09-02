-- A successful poll turn for one feed.
--
-- `last_error` / `last_error_at` are deliberately left untouched, so what a
-- now-healthy feed was last failing with survives its recovery — an operator
-- arriving after a flap can still see what happened. Read them together with
-- `status`: a row whose `status` is 'ok' and whose `last_error` is set has
-- recovered, it has not failed.
--
-- `feed` is a SOURCE name, never a product. A batched venue prices its whole
-- roster in one request and reports one constant name for all of it, so this
-- row records that the poll succeeded — not that any particular pair was in
-- the response. Per-pair delivery is `instrument_source_liveness`
-- (0010_source_liveness.sql), which carries the argument in full — and the
-- two keys do NOT join: this is a framework `Source::name`, that is a bare
-- venue token.
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
