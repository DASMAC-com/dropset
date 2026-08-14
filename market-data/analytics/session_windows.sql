-- Behavior by FX trading session: Sydney, Tokyo, London, New York.
--
-- Single-leg, like the weekend readout -- it needs only the venue's own
-- candles. For an FX stablecoin the interesting question is whether the
-- on-chain / CEX price inherits the rhythm of the underlying currency's home
-- session, or whether it trades on crypto's own clock and ignores it
-- entirely. This query is what answers that, and the answer feeds the
-- vol-ladder estimator's sigma-by-session input.
--
-- ## Sessions are defined in local time, on purpose
--
-- Each session is a local-wall-clock window in the city that defines it, NOT
-- a fixed UTC range. That is what keeps the classification correct across
-- daylight saving: London's 08:00-17:00 is 07:00-16:00 UTC in winter and
-- 08:00-17:00 UTC in summer, and the southern hemisphere shifts the opposite
-- way from the northern, so Sydney and London change in opposite directions
-- twice a year. Fixed UTC hours are wrong for a large fraction of any window
-- longer than a few months, which is exactly the window this is run over.
--
-- Postgres does the work: `AT TIME ZONE <tz>` resolves the offset from the
-- IANA database per timestamp, so historical DST transitions are handled
-- correctly rather than approximated from today's offset.
--
-- ## Sessions overlap, and rows are counted in each
--
-- A bar is emitted once per session it falls in, so a bar during the
-- London/New York overlap is counted under BOTH. This is deliberate: the
-- overlap is the highest-liquidity window of the FX day and suppressing it
-- into an exclusive bucket would hide the effect being measured. It does mean
-- the per-session bar counts sum to MORE than the total bar count -- that is
-- expected, not a double-count bug. Compare sessions to each other, and do
-- not add them up.
--
-- Weekends are excluded per session, in that session's own local calendar: a
-- Saturday in Sydney is not a Sydney session. The FX-week boundary logic in
-- weekend_vs_weekday.sql is the finer-grained treatment; here a plain
-- Monday-Friday local test is sufficient, because the session windows
-- themselves already exclude the Friday-evening and Sunday-evening hours
-- where the two definitions would disagree.
--
-- Log returns are computed ONCE over the full ordered series, with the same
-- adjacency guard used elsewhere in this directory, and only then attributed
-- to sessions. Computing them per session instead would silently treat the
-- gap across a session boundary as a one-bar return.
--
-- Parameters (psql):
--   source, product_id, granularity_secs
--
-- Example:
--   psql "$DROPSET_DB_URL" \
--     -v source=coinbase -v product_id=EURC-USDC -v granularity_secs=60 \
--     -f market-data/analytics/session_windows.sql

WITH bars AS (
    SELECT bucket_start, close
    FROM cex_prices
    WHERE source = :'source'
      AND product_id = :'product_id'
      AND granularity_secs = :granularity_secs
),
returns AS (
    SELECT
        to_timestamp(bucket_start) AS bucket_ts,
        close,
        CASE
            WHEN lag(bucket_start) OVER w = bucket_start - :granularity_secs
                 AND lag(close) OVER w > 0
                 AND close > 0
            THEN ln(close / lag(close) OVER w)
        END AS log_return
    FROM bars
    WINDOW w AS (ORDER BY bucket_start)
),
-- Local opening hours of the four centres that define the FX day. Close hour
-- is exclusive.
sessions (session, tz, open_hour, close_hour) AS (
    VALUES
        ('1-sydney',   'Australia/Sydney', 8, 17),
        ('2-tokyo',    'Asia/Tokyo',       9, 18),
        ('3-london',   'Europe/London',    8, 17),
        ('4-new_york', 'America/New_York', 8, 17)
),
tagged AS (
    SELECT
        s.session,
        r.bucket_ts,
        r.close,
        r.log_return
    FROM returns r
    JOIN sessions s
      ON EXTRACT(HOUR FROM r.bucket_ts AT TIME ZONE s.tz)
             >= s.open_hour
     AND EXTRACT(HOUR FROM r.bucket_ts AT TIME ZONE s.tz)
             < s.close_hour
     -- Monday-Friday in the session's own local calendar.
     AND EXTRACT(DOW FROM r.bucket_ts AT TIME ZONE s.tz)
             BETWEEN 1 AND 5
)
SELECT
    session,
    count(*)                                    AS bars,
    count(log_return)                           AS return_pairs,
    stddev_samp(log_return) * 10000             AS vol_bps_per_bar,
    avg(abs(log_return)) * 10000                AS mean_abs_move_bps,
    max(abs(log_return)) * 10000                AS max_abs_move_bps,
    count(DISTINCT close)                       AS distinct_closes,
    min(bucket_ts)                              AS first_bar,
    max(bucket_ts)                              AS last_bar
FROM tagged
GROUP BY session
ORDER BY session;
