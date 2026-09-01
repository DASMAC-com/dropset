-- A push source's transport is down, with a diagnosis: a subscribe that
-- failed, or a producer that ended without closing its link.
--
-- The state written is 'down', the same as a clean close — a failed subscribe
-- and a dropped socket are one answer to "is this link carrying traffic", and
-- the difference between them is this row's `last_error`, not its state. That
-- keeps the alert rule a single `state <> 'up'` with nothing to enumerate.
--
-- `$3` must already be redacted by the caller: it is a producer's client
-- error, which has no redaction of its own, and a subscribe URL can carry a
-- hosted endpoint's credential in its query string, in a path segment, or in
-- userinfo. The framework's `redact_to_origin` reduces every URL in the text
-- to scheme and host — a query-only strip is the wrong axis here — and it is
-- applied to the whole rendered cause chain, not just to the endpoint the
-- caller formats in. See the INTEGRITY note in `0008_push_liveness.sql`. This
-- column is readable by the dashboard role.
--
-- `last_up_at` is left untouched: it records when the link was last
-- established, and a failure to re-establish it must not advance it.
--
-- `disconnects` counts **transitions into** 'down', not writes of this
-- statement, and this is the statement where that distinction is load-bearing.
-- A producer retrying an unreachable endpoint re-runs this every reconnect
-- delay for as long as the outage lasts, so an unconditional increment would
-- turn one sustained outage into hundreds of "disconnects" — presenting the
-- deadest link in the table as the flappiest, which inverts the exact reading
-- the counters exist to support. The timestamps and `last_error` *are* still
-- refreshed on every attempt, deliberately: the newest failure is the most
-- useful diagnosis, and only the counter claims to be counting transitions.
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
    disconnects = push_health.disconnects
        + CASE WHEN push_health.state = 'down' THEN 0 ELSE 1 END,
    updated_at = EXCLUDED.updated_at;
