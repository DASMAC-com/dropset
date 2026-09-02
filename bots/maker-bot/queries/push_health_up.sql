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
--
-- `connects` counts **transitions into** 'up', not writes of this statement,
-- which is why the increment is conditional on the stored state rather than
-- unconditional. The column's contract is that it distinguishes a flapping
-- link from a steadily-connected one; a counter that moved on every write
-- would instead measure how often the producer happened to report, which is a
-- property of the producer's loop and not of the link.
--
-- `feed` keys a CONNECTION, not a product. One subscription carries many
-- instruments, so nothing in this table can say whether a particular pair is
-- still arriving — and since silence is a push source's healthy state, no
-- duration of it is evidence either way. That is not a gap to close here (see
-- `feeds/src/liveness.rs`); the per-pair question belongs to
-- `instrument_source_liveness`, whose `source` is a bare venue token and so
-- does not join this column.
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
    connects = push_health.connects
        + CASE WHEN push_health.state = 'up' THEN 0 ELSE 1 END,
    updated_at = EXCLUDED.updated_at;
