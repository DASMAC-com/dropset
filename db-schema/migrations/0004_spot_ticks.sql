-- Spot ticks: point-in-time prints from a venue's ticker, as opposed to
-- `cex_prices`' closed OHLCV buckets (docs/data-feeds.md §9).
--
-- Why a second table rather than a finer granularity in `cex_prices`. A candle
-- is an aggregate over a window and is keyed by that window's start; a tick is
-- one observation and has no window at all. Storing ticks as one-second
-- "candles" would put a fabricated bucket width in the key, make `open`/`high`/
-- `low`/`close` four copies of one number, and leave no honest place for a
-- confidence half-width. It would also silently corrupt every query that reads
-- `cex_prices` as bars.
--
-- What this buys, concretely: the finest bucket the candle endpoints offer is
-- 60s, so a candle series cannot show movement *between* closes no matter how
-- often it is polled. The overlay that makes a dislocation visible needs prints
-- at the cadence the collector polls, which is what lands here.
--
-- `observed_at` is the epoch second the reading is attributed to: **the venue's
-- own publish time where the venue publishes one** (Pyth Hermes does), else the
-- collector's poll second. That choice is what makes a re-poll idempotent
-- rather than a duplicate — a venue-timestamped print re-fetched after a crash
-- carries the same `observed_at` and lands on the primary key. Where the venue
-- publishes no timestamp, a re-poll inside the same second dedups and one in
-- the next second is a genuinely new observation, which is the honest reading.
--
-- Every write is idempotent (ON CONFLICT DO NOTHING on the PK), matching the
-- store sink's at-least-once contract (docs/data-feeds.md §3).
--
-- Volume: at a 15s cadence this is ~4 rows/minute/pair/source — a roster of
-- seven pairs across three sources is well under 10^5 rows/day, so no
-- partitioning or retention policy is warranted yet. Note it here because the
-- growth rate is an order of magnitude above `cex_prices`', so that stops being
-- true sooner.
CREATE TABLE spot_ticks (
    source      TEXT             NOT NULL,
    product_id  TEXT             NOT NULL,
    observed_at BIGINT           NOT NULL,
    price       DOUBLE PRECISION NOT NULL,
    -- Symmetric confidence half-width in the same units as `price`, for a
    -- venue that publishes one (Pyth). NULL means **no confidence notion this
    -- tick**, never "zero half-width": a zero reads as perfect certainty and
    -- would silently satisfy a fresh-but-uncertain gate that a missing value
    -- correctly fails. The constraint below makes that distinction structural
    -- rather than a convention a writer has to remember, because the adapter
    -- that decodes this field treats a published zero as malformed for exactly
    -- the same reason.
    confidence  DOUBLE PRECISION,
    PRIMARY KEY (source, product_id, observed_at),
    CONSTRAINT confidence_is_absent_or_positive
        CHECK (confidence IS NULL OR confidence > 0)
);

-- No secondary index, for the same reason `cex_prices` has none: the dominant
-- read is a time-ordered scan of one (source, product_id) series for the
-- overlay, and the primary key's implicit index — leading equality, trailing
-- `observed_at` ordered — already serves it.
