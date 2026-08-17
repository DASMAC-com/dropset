-- cspell:word annualization
-- Realized volatility by hour of day, split by weekday / weekend regime.
--
-- This is the shaped-for-consumers query: the vol-ladder estimator wants a
-- sigma it can index by session and regime, and this is that table. It is
-- also the finest-grained of the single-leg analytics, so it is the one most
-- sensitive to a thin tape -- see the density note at the bottom before
-- pointing it at a new product.
--
-- `tz` selects the wall clock the hours are expressed in. UTC is the neutral
-- default and the right choice for comparing across currencies; pass a
-- centre's own zone (`Australia/Sydney`, `Europe/London`) when you want the
-- profile as a local trading day, which is DST-correct because Postgres
-- resolves the offset per timestamp rather than once.
--
-- ## Annualization is offered, with a caveat you should read
--
-- `vol_bps_annualized` scales the per-bar figure by sqrt(bars per year),
-- assuming CONTINUOUS trading -- which is true of a crypto venue and false of
-- interbank FX, and is applied uniformly here regardless of regime. So the
-- annualized weekend column is a useful comparative number and NOT a
-- forecast: nobody can trade an FX stablecoin's weekend vol for a year. Use
-- the per-bar column when you want the honest measurement and the annualized
-- column only when you need something on the same scale as a quoted vol.
--
-- ## The adjacency guard, again
--
-- Same as the sibling queries: a return is computed only between genuinely
-- consecutive buckets. It matters doubly here because the output is bucketed
-- by hour -- a gap-spanning pseudo-return would be attributed to whichever
-- single hour its later bar lands in, spiking exactly one cell of the profile
-- and looking like a real time-of-day effect.
--
-- ## Read `bars` before reading `vol_bps_per_bar`
--
-- A per-hour, per-regime cell divides the series 48 ways. On a liquid product
-- that is fine: the 60-day EURC-USDC backfill carries ~1,170 bars/day, so
-- every cell is well populated. On a thin one it is not -- the AUDD-USDC book
-- on the same venue and window averages 14.1 bars/day and produces cells with
-- single-digit counts, where a standard deviation is arithmetic rather than
-- information. The `bars` column is printed first for that reason. Treat any
-- cell below a few dozen bars as not reportable rather than a small number.
--
-- Parameters (psql):
--   source, product_id, granularity_secs, tz
--
-- Example:
--   psql "$DROPSET_DB_URL" \
--     -v source=coinbase -v product_id=EURC-USDC \
--     -v granularity_secs=60 -v tz=UTC \
--     -f market-data/analytics/realized_vol_by_hour.sql

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
        CASE
            WHEN lag(bucket_start) OVER w = bucket_start - :granularity_secs
                 AND lag(close) OVER w > 0
                 AND close > 0
            THEN ln(close / lag(close) OVER w)
        END AS log_return
    FROM bars
    WINDOW w AS (ORDER BY bucket_start)
),
classified AS (
    SELECT
        EXTRACT(HOUR FROM bucket_ts AT TIME ZONE :'tz')::int AS hour_of_day,
        CASE
            WHEN EXTRACT(DOW FROM bucket_ts AT TIME ZONE 'America/New_York') = 6
                THEN 'weekend'
            WHEN EXTRACT(DOW FROM bucket_ts AT TIME ZONE 'America/New_York') = 0
                 AND EXTRACT(HOUR FROM bucket_ts AT TIME ZONE 'America/New_York') < 17
                THEN 'weekend'
            WHEN EXTRACT(DOW FROM bucket_ts AT TIME ZONE 'America/New_York') = 5
                 AND EXTRACT(HOUR FROM bucket_ts AT TIME ZONE 'America/New_York') >= 17
                THEN 'weekend'
            ELSE 'weekday'
        END AS regime,
        log_return
    FROM returns
)
SELECT
    hour_of_day,
    regime,
    count(log_return)                       AS bars,
    stddev_samp(log_return) * 10000         AS vol_bps_per_bar,
    -- sqrt(bars per year) at this granularity. See the caveat above: this
    -- assumes continuous trading and is comparative, not a forecast.
    stddev_samp(log_return) * 10000
        * sqrt(365.25 * 86400.0 / :granularity_secs) AS vol_bps_annualized,
    avg(abs(log_return)) * 10000            AS mean_abs_move_bps,
    max(abs(log_return)) * 10000            AS max_abs_move_bps
FROM classified
GROUP BY hour_of_day, regime
ORDER BY hour_of_day, regime;
