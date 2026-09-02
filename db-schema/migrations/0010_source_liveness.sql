-- Per-`(source, product)` ingestion liveness, and the health tables' contract.
--
-- WHAT THIS FIXES, WHICH IS NOT WHAT IT LOOKS LIKE AT FIRST.
--
-- `instrument_liveness` (0009) already answers "is this PRODUCT collecting",
-- by seeking each `(source, product_id)` series on the measurement tables'
-- primary-key prefix and then aggregating with `GROUP BY r.product_id`. That
-- aggregation is right for a dimension — "what instrument is this" has the
-- same answer whoever measured it — but it discards the per-source reading its
-- own CTE just computed, and with it the one question an operator asks when a
-- venue misbehaves: did THIS collector stop delivering THIS pair?
--
-- The consequence is a masking bug one axis over from the one that motivated
-- the dimension. EUR-USD is polled by four collectors (OANDA, Twelve Data,
-- Alpha Vantage, Pyth), so `instrument_liveness` reports it live while three
-- of them are dark. That is the same shape as a venue-level `feed_health` row
-- reading fresh while a single pair on a batched venue is dead — the error the
-- registry carries a `source` column to make answerable.
--
-- So this view exposes the reading rather than computing a new one: same
-- prefix seeks, same class-aware thresholds, one row per registry row instead
-- of one per product. `instrument_liveness` is then re-derived from it below,
-- which is what keeps the two from drifting.
--
-- WHAT IT DELIBERATELY DOES NOT ANSWER.
--
-- This is INGESTION liveness — "when did a reading for this pair last reach
-- us" — and never price age. The distinction is load-bearing because
-- `spot_ticks.observed_at` is the venue's publish time only where the venue
-- publishes one (Pyth Hermes does); everywhere else it is the collector's poll
-- second, which is the honest attribution for *arrival* and says nothing about
-- how old the quote was when it arrived.
--
-- Two blind spots follow, and both are better named here than rediscovered:
--
--   * A venue answering `200 OK` with a FROZEN quote reads perfectly live. No
--     arrangement of receipt stamps can catch that; it needs a publish
--     timestamp the venue does not send. Anything alerting on price staleness
--     rather than delivery staleness needs that work, not this view.
--   * A pegged pair legitimately sits still, so "price unchanged" is not a
--     fault signal here either — `USDC-USD`, the peg leg the Kraken collector
--     exists to capture, is the case in point.
--   * Two collector PROCESSES sharing one source label collapse into one row,
--     and `coinbase` is a live instance of it (the `last_seen` note below has
--     the detail). If its candle collector goes dark while its ticker keeps
--     running, that row still reads live — the same masking shape this view
--     exists to remove, one level further in. Closing it needs a distinct
--     `source` per collector, which is a change to the collectors and not to
--     this view.
--
-- WHERE THE THRESHOLDS LIVE NOW.
--
-- 0009 introduced the two constants with the note that they are "defined
-- nowhere else. Change them here and every consumer follows." That sentence
-- now points at the wrong file: an applied migration is immutable, so the note
-- cannot be corrected in place, and this replaces the view that carried it.
-- The constants are defined ONCE, in `thresholds` below, and
-- `instrument_liveness` inherits them by construction because it is an
-- aggregate of this view rather than a second copy of its logic. A future
-- change edits one CTE here.
CREATE VIEW instrument_source_liveness AS
WITH thresholds AS (
    -- Unchanged from 0009, and unchanged deliberately: the weekend gap they
    -- clear is a measured property of the venues (Friday 16:59 to Sunday 17:04
    -- New York, 47-49 hours across the daylight-saving transition), not a
    -- tuning knob. See 0009 for the full derivation.
    SELECT
        72 * 3600 AS session_bound_stale_secs,
        48 * 3600 AS always_open_stale_secs
),
last_seen AS (
    -- One row per registry row — the `GROUP BY` that 0009 applies here is the
    -- whole difference between the two views, so there is no aggregate around
    -- `GREATEST` and no grouping key.
    --
    -- The seek costs are inherited unchanged: the ticks side is a `Limit 1`
    -- backward seek on `(source, product_id)`, and the bars side range-scans,
    -- because `cex_prices` puts `granularity_secs` between `product_id` and
    -- `bucket_start` in its key. 0009 states the argument in full; measure
    -- before putting either view on a per-load path.
    --
    -- `GREATEST` ignores NULL arguments and returns NULL only when every
    -- argument is NULL, so a registered pair whose venue has never answered
    -- yields NULL — distinguishable from one that answered and stopped, with
    -- no 0 sentinel to manufacture and then undo.
    --
    -- Usually only one side is populated, because most collectors write bars
    -- or ticks and not both. `coinbase` is the exception, and it is not
    -- hypothetical: `market-data/src/bin/coinbase.rs` writes `cex_prices`
    -- while `market-data/src/bin/coinbase_ticker.rs` writes `spot_ticks`,
    -- both under the source label `coinbase`, and deliberately so — "it is
    -- the same venue, and the table is what distinguishes a print from a
    -- bucket". For that source both arguments are populated and `GREATEST`
    -- returns the later of the two, which is the right reading of "when did
    -- anything last arrive for this pair".
    --
    -- 0009 states the never-both case as an invariant. That was wrong when it
    -- was written and it is immutable now, so this is the corrected
    -- statement; do not propagate the invariant form into a later migration.
    SELECT
        r.source,
        r.product_id,
        GREATEST(bars.last_at, ticks.last_at) AS last_at
    FROM instrument_registry r
    LEFT JOIN LATERAL (
        SELECT max(c.bucket_start) AS last_at
        FROM cex_prices c
        WHERE c.source = r.source AND c.product_id = r.product_id
    ) bars ON true
    LEFT JOIN LATERAL (
        SELECT max(s.observed_at) AS last_at
        FROM spot_ticks s
        WHERE s.source = r.source AND s.product_id = r.product_id
    ) ticks ON true
),
bounded AS (
    SELECT
        l.source,
        l.product_id,
        i.asset_class,
        l.last_at AS last_data_at,
        -- The always-open classes are named explicitly and the ELSE carries
        -- the LOOSE bound, for the reason 0009 gives: written the other way
        -- round, 'unclassified' would fall to the tight bound and read quiet
        -- across every weekend. The two errors are not symmetric — a spurious
        -- "quiet" hides data, a slow "still live" costs a stale row on a panel
        -- — so an unknown class errs loose.
        CASE
            WHEN i.asset_class IN ('stablecoin-pair', 'peg-pair', 'crypto')
                THEN t.always_open_stale_secs
            ELSE t.session_bound_stale_secs
        END       AS stale_after_secs
    FROM last_seen l
    JOIN instruments i ON i.product_id = l.product_id
    CROSS JOIN thresholds t
)
SELECT
    source,
    product_id,
    asset_class,
    last_data_at,
    stale_after_secs,
    -- The NULL guard is spelled out rather than left to the comparison, so a
    -- never-collected pair is answered on purpose instead of by accident.
    last_data_at IS NOT NULL
        AND last_data_at > EXTRACT(EPOCH FROM now())::BIGINT - stale_after_secs
        AS is_live
FROM bounded;

COMMENT ON VIEW instrument_source_liveness IS
    'Per-(source, product) ingestion liveness: when a reading for this pair '
    'last reached us from this collector, and a live/quiet verdict against an '
    'asset-class bound. The two class bounds are defined here and inherited '
    'by instrument_liveness, which aggregates this view. Read this rather '
    'than feed_health to ask whether one collector stopped delivering one '
    'pair, noting the keys do NOT join (a bare venue token here, a framework '
    'source name there). Delivery staleness, never price age: a venue '
    'answering with a frozen quote reads live here.';

-- The per-product dimension, re-derived as a strict aggregate of the view
-- above rather than a second traversal of the measurement tables.
--
-- Every column keeps the name, type, position and meaning 0009 gave it —
-- `CREATE OR REPLACE VIEW` requires the first three, and the fourth is what
-- makes this a refactor rather than a change: `max(last_data_at)` over a
-- product's sources is exactly the `max(GREATEST(bars, ticks))` the replaced
-- body computed, and `is_live` is recomputed from that aggregate with the same
-- expression. Existing consumers — the dashboards' variable queries among
-- them — see no difference.
--
-- `stale_after_secs` is a grouping key rather than an aggregate because it is
-- a pure function of `asset_class`, which is per-product: every row of a
-- product carries the same bound, so grouping by it is exact and needs no
-- `min`/`max` that would imply a choice was being made between differing
-- values.
--
-- **A future migration that changes this view's input incompatibly must drop
-- this view first.** Postgres refuses to `CREATE OR REPLACE` a view whose
-- column list another view depends on, so altering
-- `instrument_source_liveness`'s columns or types means
-- `DROP VIEW instrument_liveness`, then the replacement, then recreating this
-- one — all in the same migration. Appending a column to the source view
-- needs none of that, since this one selects by name. That is the price of
-- deriving rather than duplicating, and it is worth naming here because the
-- error arrives at migration time on somebody else's change.
CREATE OR REPLACE VIEW instrument_liveness AS
WITH per_product AS (
    -- The aggregate named once, so the projection below reads a value rather
    -- than repeating the call three times.
    SELECT
        product_id,
        asset_class,
        stale_after_secs,
        max(last_data_at) AS last_data_at
    FROM instrument_source_liveness
    GROUP BY product_id, asset_class, stale_after_secs
)
SELECT
    product_id,
    asset_class,
    last_data_at,
    stale_after_secs,
    -- Same NULL guard as the per-source view, and spelled out for the same
    -- reason: a product no collector has ever answered for is not live, and
    -- that should be answered on purpose rather than by the comparison.
    last_data_at IS NOT NULL
        AND last_data_at > EXTRACT(EPOCH FROM now())::BIGINT - stale_after_secs
        AS is_live
FROM per_product;

COMMENT ON VIEW instrument_liveness IS
    'Per-product data freshness and a live/quiet verdict, with the staleness '
    'bound chosen by asset class so an FX weekend is not mistaken for a dead '
    'collector. An aggregate of instrument_source_liveness across a product''s '
    'collectors, so a product reads live while individual collectors are dark '
    '— ask that view for the per-collector answer. Not a market calendar: it '
    'knows how long silence has run, not when a session is expected closed.';

-- THE HEALTH TABLES' CONTRACT, STATED WHERE ITS READERS ARE.
--
-- `feeds/src/health.rs` and `feeds/src/liveness.rs` already state this at the
-- framework level, and state it well: the runner hands a recorder a feed
-- *name* and batch statistics, never the records, so a health row reports
-- whether the poller is alive and cannot report what the feed said. The
-- module doc is explicit that per-instrument values belong to the consumer
-- that resolved the instrument and that this is "not a gap to close here".
--
-- The reason to restate it in the catalog is that the readers who most need it
-- never open those files. A dashboard or alert author works from the schema
-- and the query files, and `feed_health` looks per-instrument the moment a
-- per-product source name appears in it — which it does, because the candle
-- collectors name themselves per product (`cex:coinbase:EURC-USDC`) while the
-- batched venues return one constant venue-level name for the whole venue.
-- Reading a batched venue's row as per-pair is the silent-green failure this
-- comment exists to prevent: the row stays fresh because the REQUEST
-- answered, whatever it contained — so one pair that stopped arriving, or
-- every pair, still leaves it looking healthy.
--
-- AND THE TWO KEYS DO NOT JOIN, which is the part a panel author most needs.
-- `feed_health.feed` is the framework's `Source::name`: prefixed and
-- per-product for the candle collectors (`cex:coinbase:EURC-USDC`), a venue
-- constant for the batched ones. `instrument_source_liveness.source` is the
-- bare venue token the collector registers with `register_instruments`
-- (`coinbase`, `oanda`, `twelvedata`, `alphavantage`, `kraken`, `pyth`). The
-- two coincide only where the framework name happens to be a bare venue
-- token, so a dashboard variable populated from one will silently match
-- nothing in the other. Map them deliberately; never equi-join them.
COMMENT ON TABLE feed_health IS
    'Per-FEED poll liveness: whether a poller is alive, when it last '
    'succeeded, and what it last failed with. The key is a framework source '
    'name, which is venue-level for a batched venue (one request prices many '
    'products), so this row is NOT per-product: it records that the request '
    'answered, not that any pair was in the response. For a per-pair answer '
    'read instrument_source_liveness — but note the keys do NOT join, since '
    'this column is a framework name (cex:coinbase:EURC-USDC) while that '
    'view''s source is a bare venue token (coinbase).';

COMMENT ON TABLE push_health IS
    'Per-CONNECTION transport state for a push source: subscribed and able to '
    'deliver, never message recency — silence is a push source''s healthy '
    'state. Like feed_health this is keyed per feed, not per product, so it '
    'cannot answer whether one pair stopped arriving; read '
    'instrument_source_liveness for that, keeping in mind the two keys do NOT '
    'join (a framework source name here, a bare venue token there).';
