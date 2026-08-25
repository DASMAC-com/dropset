-- A push source's transport closed, with no diagnosis to record.
--
-- The undiagnosed case is its own statement precisely so it can leave
-- `last_error` / `last_error_at` untouched. A socket the venue closed is a
-- state, not an error, so it has nothing to say there — and binding a NULL
-- would erase the last thing an operator had to go on, which is the failure
-- this split exists to prevent. `last_up_at` is likewise left alone: it
-- records when the link was established and a disconnect does not change that.
INSERT INTO push_health (
    feed,
    state,
    last_down_at,
    disconnects,
    updated_at
)
VALUES ($1, 'down', $2, 1, $2)
ON CONFLICT (feed) DO UPDATE SET
    state = 'down',
    last_down_at = EXCLUDED.last_down_at,
    disconnects = push_health.disconnects + 1,
    updated_at = EXCLUDED.updated_at;
