-- A push source's transport is down, with a diagnosis: a subscribe that
-- failed, or a producer that ended without closing its link.
--
-- The state written is 'down', the same as a clean close — a failed subscribe
-- and a dropped socket are one answer to "is this link carrying traffic", and
-- the difference between them is this row's `last_error`, not its state. That
-- keeps the alert rule a single `state <> 'up'` with nothing to enumerate.
--
-- `$3` must already be sanitized by the caller: it is a producer's client
-- error, which has no name-aware redaction of its own, and a subscribe URL
-- carries a hosted endpoint's credential in its query string. The framework's
-- `sanitize_error` is what strips it — see the INTEGRITY note in
-- `0006_push_liveness.sql`. This column is readable by the dashboard role.
--
-- `last_up_at` is left untouched: it records when the link was last
-- established, and a failure to re-establish it must not advance it.
INSERT INTO push_health (
    feed,
    state,
    last_down_at,
    last_error,
    last_error_at,
    disconnects,
    updated_at
)
VALUES ($1, 'down', $2, $3, $2, 1, $2)
ON CONFLICT (feed) DO UPDATE SET
    state = 'down',
    last_down_at = EXCLUDED.last_down_at,
    last_error = EXCLUDED.last_error,
    last_error_at = EXCLUDED.last_error_at,
    disconnects = push_health.disconnects + 1,
    updated_at = EXCLUDED.updated_at;
