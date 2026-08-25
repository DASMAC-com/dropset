-- A push source's transport came up: subscribed, and able to deliver.
--
-- Reported on a successful subscribe, never on the first record — "able to
-- deliver" and "did deliver" are different facts and only the former is a
-- health signal, which is the entire reason this table exists apart from
-- `feed_health`.
--
-- `last_down_at`, `last_error` and `last_error_at` are deliberately left
-- untouched, so what a now-connected link was last failing with survives its
-- recovery. Read them together with `state`: a row whose `state` is 'up' and
-- whose `last_error` is set has reconnected, it has not failed.
INSERT INTO push_health (
    feed,
    state,
    last_up_at,
    connects,
    updated_at
)
VALUES ($1, 'up', $2, 1, $2)
ON CONFLICT (feed) DO UPDATE SET
    state = 'up',
    last_up_at = EXCLUDED.last_up_at,
    connects = push_health.connects + 1,
    updated_at = EXCLUDED.updated_at;
