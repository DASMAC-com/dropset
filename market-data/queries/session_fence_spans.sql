-- The FX session fence's spans from the instant `$1` (epoch seconds) onward:
-- the span covering it, and the one after.
--
-- Two rows rather than one so a consumer holding the answer does not go dark at
-- the boundary. The spans are contiguous, so the covering row's `ends_at` is the
-- next row's `starts_at`; with only the covering row cached, every Friday and
-- Sunday close would read as an unestablished session for the length of one
-- poll, until the next read fetched the span that had just begun. The consumer
-- still decides the session per tick from whichever cached span covers it, so a
-- second row it never reaches costs nothing.
--
-- `ends_at > t` is the half-open read the view's own COMMENT asks for, stated
-- from the other side: a span ending exactly at `t` does not cover it. No
-- `starts_at <= t` bound, deliberately — the covering row satisfies it anyway,
-- and the row after it must be allowed through. Whether the first row actually
-- covers `t` is the consumer's check, not this statement's: no covering row is
-- the fail-closed answer, and filtering it here would make it indistinguishable
-- from a gap the consumer has to see.
--
-- Projected as epoch seconds, the representation every consumer already keeps
-- its tick in. Postgres stays the daylight-saving authority: the view converts
-- the ET anchors, and this only renders the resulting instants.
SELECT
    state,
    EXTRACT(EPOCH FROM starts_at)::bigint AS starts_unix,
    EXTRACT(EPOCH FROM ends_at)::bigint AS ends_unix
FROM fx_session_window
WHERE ends_at > to_timestamp($1::bigint)
ORDER BY starts_at
LIMIT 2
