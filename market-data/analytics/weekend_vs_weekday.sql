-- Weekend vs. weekday behavior for one venue product.
--
-- This is the "what does the Australian dollar do on weekends" readout, and
-- it is a SINGLE-LEG query: it needs only the venue's own candles, no FX
-- anchor. That matters for sequencing -- the Coinbase collector backfills 60
-- days on a cold start (market-data/src/config.rs), so this query has real
-- history to chew on well before a forward-only feed accumulates any.
--
-- The question it answers is a genuine one rather than a curiosity. The FX
-- market closes for the weekend; crypto venues never do. So for ~48 hours a
-- week an FX stablecoin trades with NO anchor to arbitrage against, and
-- whatever it does in that window is what a maker quoting through the
-- weekend is exposed to.
--
-- FX week convention: the interbank week runs Sunday 17:00 to Friday 17:00
-- New York time. It is expressed here in `America/New_York` local time rather
-- than as fixed UTC hours precisely so that it stays correct across DST --
-- the UTC offset of the weekly close shifts twice a year, and a hardcoded
-- 21:00-or-22:00 UTC boundary is wrong for half the year.
--
-- Realized vol is computed from ADJACENT-BUCKET log returns only. The guard
-- matters more here than anywhere else in this directory: the weekend is a
-- ~48-hour hole in an FX series, and a plain window function would happily
-- compute one "return" spanning the entire closure, producing a single
-- enormous pseudo-return that lands in whichever regime the later bucket
-- falls into and dominates its variance. Gapped pairs are dropped, not
-- bridged. This is not hypothetical even for a 24/7 crypto product: on the
-- 60-day EURC-USDC backfill roughly 12% of bars have a gap before them,
-- because Coinbase emits no candle for a minute with no trades.
--
-- ## The weekend anchor caveat -- and a heuristic that does NOT work
--
-- An FX source may keep publishing bars through the weekend even though the
-- interbank market is shut. One vendor surveyed for the anchor leg returns a
-- full 1440-bar Saturday for AUD/USD, where OANDA returns nothing at all.
-- That is worth knowing before you pool sources into one statistic.
--
-- It is tempting to detect such a series by looking for a collapse in
-- `distinct_closes`. Do NOT: that heuristic was tested against this repo's
-- own genuinely-traded tape and it is wrong. Measured on the 60-day
-- EURC-USDC backfill, an individual Saturday carries only 3-15 distinct
-- closes across ~1000 bars, and across the whole window weekends show MORE
-- distinct closes per bar than weekdays (8.9 vs 5.5 per 1000). A quiet
-- weekend tape with very few distinct prices is the NORMAL appearance of a
-- real 24/7 market, not a symptom. Anything keyed on that ratio fires on
-- honest data.
--
-- The distinction that does hold is magnitude, not price count. A real 24/7
-- crypto tape still travels: EURC-USDC ranges 2-16 bps over a Saturday. The
-- vendor series above moved about 0.7 bps across its entire Saturday, which
-- is quieter than a traded market rather than noisier -- so if it is sourcing
-- some venue that runs on a weekend, that venue is doing almost no price
-- discovery. A carried-forward or indicative quote fits the magnitude best,
-- though the decisive test is correlation against a 24/7 venue over the same
-- window, which has not been run yet.
--
-- What follows from this for the analytics is narrow and worth stating
-- exactly, because it is easy to over-correct:
--
--   * Do NOT compute a weekend VOLATILITY figure from an anchor whose market
--     was closed. A frozen or indicative series yields a plausible-looking
--     ultra-low sigma for a session that never traded. Prefer a source whose
--     weekend bar count is zero -- absence is the honest signal.
--   * DO still compute the weekend DEVIATION series. An anchor that holds
--     flat while the venue leg keeps moving is not a defect; it is precisely
--     the mechanism this issue exists to measure. With interbank FX shut
--     there is no arbitrage channel, so the basis is free to widen, and that
--     widening is the finding rather than an artifact.
--
-- `distinct_closes` and `zero_range_bars` are still reported below, because
-- they are cheap and genuinely informative about a series. They are simply
-- not a fabrication detector -- read them as texture, not as a test.
--
-- Parameters (psql):
--   source, product_id, granularity_secs
--
-- Example:
--   psql "$DROPSET_DB_URL" \
--     -v source=coinbase -v product_id=AUDD-USDC -v granularity_secs=60 \
--     -f market-data/analytics/weekend_vs_weekday.sql

WITH bars AS (
    SELECT
        bucket_start,
        close,
        high,
        low,
        to_timestamp(bucket_start) AS bucket_ts
    FROM cex_prices
    WHERE source = :'source'
      AND product_id = :'product_id'
      AND granularity_secs = :granularity_secs
),
regimes AS (
    SELECT
        bucket_start,
        close,
        high,
        low,
        bucket_ts,
        CASE
            -- Local wall clock in the city that defines the FX week boundary.
            WHEN EXTRACT(DOW FROM bucket_ts AT TIME ZONE 'America/New_York') = 6
                THEN 'weekend'
            WHEN EXTRACT(DOW FROM bucket_ts AT TIME ZONE 'America/New_York') = 0
                 AND EXTRACT(HOUR FROM bucket_ts AT TIME ZONE 'America/New_York') < 17
                THEN 'weekend'
            WHEN EXTRACT(DOW FROM bucket_ts AT TIME ZONE 'America/New_York') = 5
                 AND EXTRACT(HOUR FROM bucket_ts AT TIME ZONE 'America/New_York') >= 17
                THEN 'weekend'
            ELSE 'weekday'
        END AS regime
    FROM bars
),
returns AS (
    SELECT
        regime,
        bucket_ts,
        close,
        high,
        low,
        CASE
            WHEN lag(bucket_start) OVER w = bucket_start - :granularity_secs
                 AND lag(close) OVER w > 0
                 AND close > 0
            THEN ln(close / lag(close) OVER w)
        END AS log_return
    FROM regimes
    WINDOW w AS (ORDER BY bucket_start)
)
SELECT
    regime,
    count(*)                                        AS bars,
    count(log_return)                               AS return_pairs,
    min(bucket_ts)                                  AS first_bar,
    max(bucket_ts)                                  AS last_bar,
    -- Per-bar realized vol, in bps. Multiplying a log return by 1e4 is the
    -- standard small-return approximation to bps and is exact to well within
    -- a bp at the magnitudes an FX stablecoin actually moves.
    stddev_samp(log_return) * 10000                 AS vol_bps_per_bar,
    avg(abs(log_return)) * 10000                    AS mean_abs_move_bps,
    max(abs(log_return)) * 10000                    AS max_abs_move_bps,
    -- Tape-quality columns. See the header: these are what distinguish a
    -- genuinely quiet market from an indicative feed that never traded.
    count(DISTINCT close)                           AS distinct_closes,
    count(*) FILTER (WHERE high = low)              AS zero_range_bars,
    min(close)                                      AS min_close,
    max(close)                                      AS max_close
FROM returns
GROUP BY regime
ORDER BY regime;
