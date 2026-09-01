-- The instruments dimension: what each stored product IS, as opposed to what
-- was measured about it (docs/data-feeds.md §9).
--
-- WHY THIS EXISTS.
--
-- Nothing in the schema carries an asset class. `cex_prices` and `spot_ticks`
-- hold a `product_id` and nothing else about the instrument, so every question
-- of the form "show me the FX pairs" or "group these by class" has had to be
-- answered by hardcoding a product list into a dashboard panel. That is the
-- defect this closes: a hardcoded list is wrong the moment the roster changes,
-- and wrong silently — the panel keeps rendering, just without the new pair.
--
-- It is also the hard prerequisite for selecting by CURRENCY rather than by
-- pair. A canonical `BASE-QUOTE` id can be split, but knowing that `EURC` and
-- `EUR` are the stablecoin and the fiat of the same currency — or that `EURC`
-- is a stablecoin at all — is not derivable from the string. That fact has to
-- be stated somewhere, and this is the somewhere.
--
-- TWO OBJECTS, BECAUSE THERE ARE TWO KINDS OF FACT HERE.
--
-- `currency_kinds` is reference data: irreducible, slowly-changing, and
-- genuinely hand-maintained, because no rule tells you what kind of thing a
-- currency symbol names. It follows the precedent `pyth_fx_feeds` set in
-- `0005` — narrow scope, seeded by the migration, no runtime writer.
--
-- `instrument_registry` is the opposite: it is written by the collectors from
-- their own rosters at startup, so it needs no hand edits on a roster change
-- at all. Adding a pair to `PRODUCT_IDS` and restarting the collector is what
-- populates it.
--
-- WHY THE COLLECTORS WRITE IT, RATHER THAN A VIEW DERIVING IT FROM THE DATA.
--
-- The obvious alternative — `SELECT DISTINCT product_id` over `cex_prices` and
-- `spot_ticks` — was rejected on cost, not on taste. Both tables key on
-- `(source, product_id, …)`, so `product_id` does not lead any index, and both
-- migrations deliberately declined a secondary index on the grounds that the
-- dominant read is a time-ordered scan of one series. A DISTINCT over a
-- non-leading column is therefore a full scan of a table that grows without
-- bound, and the primary consumer of this dimension is a Grafana template
-- variable that fires on every dashboard load. Adding an index to serve it
-- would mean paying write overhead on the hottest tables in the store to
-- answer a question about a set that changes a few times a year.
--
-- The registry inverts that: the set of products is written once per collector
-- start — a handful of rows — and read from a table small enough that no index
-- beyond the primary key is warranted.
--
-- The cost of the choice, stated plainly: this records the CONFIGURED roster,
-- not the observed data. A pair can appear here before its first row lands
-- (a collector that started but whose venue has not answered yet), and a
-- retired pair keeps its row until someone deletes it. For the dimension's
-- purpose that is the right direction to err — a configured pair with no data
-- is a fact an operator wants to see, and it is precisely what the ingestion
-- dashboard's coverage guarantee is about. Where a panel needs "has this
-- actually produced data", it must join to the measurement tables and say so,
-- rather than assuming a registry row implies a series.
CREATE TABLE currency_kinds (
    -- The currency symbol as it appears as a leg of a canonical product id:
    -- upper-case, which is what `parse_roster` normalizes to. Note that a
    -- mixed-case ticker (MXNe) is stored here in the upper-cased spelling the
    -- roster produces (MXNE), because this has to join against `product_id`
    -- and that is the form the product id carries.
    currency TEXT PRIMARY KEY,
    -- What kind of thing the symbol names. The class of a PAIR is derived
    -- from its two legs' kinds by the view below, rather than declared per
    -- pair: a per-pair declaration would be O(pairs) hand edits and could
    -- disagree with the legs it claims, while this is O(currencies) and
    -- cannot.
    --
    -- 'fiat'       — a sovereign currency, ISO 4217.
    -- 'stablecoin' — a token intended to track one of them.
    -- 'crypto'     — anything else: a token with no peg.
    kind     TEXT NOT NULL,
    CONSTRAINT currency_is_a_product_leg
        CHECK (currency ~ '^[A-Z0-9]{2,10}$'),
    -- Spelled out rather than an enum type: adding a kind to a CHECK is one
    -- migration, while adding a value to an enum is a migration plus a
    -- transaction-visibility caveat, and this vocabulary is not expected to
    -- grow.
    CONSTRAINT kind_is_known
        CHECK (kind IN ('fiat', 'stablecoin', 'crypto'))
);

-- The seed, taken from the canonical currency roster in
-- `frontend/lib/data/currencies.json`: its fifteen top-level fiat keys, and
-- every stablecoin listed under them. That file is the intake roster, so it is
-- the honest source for "what currencies does this system know about" — wider
-- than any single collector's product list, which is the right direction, since
-- a currency has to be classifiable before the first product using it is
-- polled.
--
-- Symbols are upper-cased to match the canonical product id: the roster spells
-- two of them in mixed case (`MXNe`, `cNGN`) and `parse_roster` normalizes, so
-- they are `MXNE` and `CNGN` here. Getting that wrong is a silent miss — the
-- join simply finds nothing.
--
-- **THIS SEED IS COUPLED TO THAT FILE AND NOTHING CHECKS IT.** A currency added
-- to the roster and not added here classifies as 'unclassified' — visible in
-- the dimension rather than silently dropped, which is the whole reason the
-- view left-joins, but still wrong. That coupling bit during this very change:
-- EUROP was added to the roster while this migration was being written, and
-- the first draft of this seed carried five stablecoins where the roster had
-- twenty-five. A mechanical coverage check is worth having and is deliberately
-- left to follow-up work rather than bolted on here.
--
-- Fifteen fiats and twenty-five stablecoins, as of this migration.
INSERT INTO currency_kinds (currency, kind) VALUES
    -- Fiat, ISO 4217.
    ('AUD', 'fiat'),
    ('BRL', 'fiat'),
    ('CAD', 'fiat'),
    ('CHF', 'fiat'),
    ('EUR', 'fiat'),
    ('GBP', 'fiat'),
    ('IDR', 'fiat'),
    ('JPY', 'fiat'),
    ('MXN', 'fiat'),
    ('MYR', 'fiat'),
    ('NGN', 'fiat'),
    ('SGD', 'fiat'),
    ('TRY', 'fiat'),
    ('USD', 'fiat'),
    ('ZAR', 'fiat'),
    -- Stablecoins, with the fiat each one tracks. That peg is what makes a
    -- stablecoin-against-its-own-fiat pair a peg measurement rather than a
    -- rate — see the class derivation below. The peg is not stored: it is not
    -- needed to classify a pair (both legs' kinds suffice) and storing it
    -- would be a second fact to keep in step with the roster.
    ('AUDD', 'stablecoin'),   -- AUD
    ('AUDM', 'stablecoin'),   -- AUD
    ('BRZ', 'stablecoin'),    -- BRL
    ('CADC', 'stablecoin'),   -- CAD
    ('CNGN', 'stablecoin'),   -- NGN, spelled `cNGN` in the roster
    ('EURAU', 'stablecoin'),  -- EUR
    ('EURC', 'stablecoin'),   -- EUR
    ('EURCV', 'stablecoin'),  -- EUR
    ('EUROP', 'stablecoin'),  -- EUR
    ('GYEN', 'stablecoin'),   -- JPY
    ('IDRX', 'stablecoin'),   -- IDR
    ('MXNE', 'stablecoin'),   -- MXN, spelled `MXNe` in the roster
    ('MYRC', 'stablecoin'),   -- MYR
    ('PYUSD', 'stablecoin'),  -- USD
    ('TGBP', 'stablecoin'),   -- GBP
    ('TRYB', 'stablecoin'),   -- TRY
    ('USD1', 'stablecoin'),   -- USD
    ('USDC', 'stablecoin'),   -- USD
    ('USDG', 'stablecoin'),   -- USD
    ('USDT', 'stablecoin'),   -- USD
    ('VCHF', 'stablecoin'),   -- CHF
    ('VGBP', 'stablecoin'),   -- GBP
    ('XSGD', 'stablecoin'),   -- SGD
    ('ZARP', 'stablecoin'),   -- ZAR
    ('ZARU', 'stablecoin');   -- ZAR

-- No 'crypto' row is seeded. Nothing in any collector's roster is an unpegged
-- token today, and a seeded currency with no product would be reference data
-- describing nothing. The kind is in the CHECK above so the class derivation
-- is total when one arrives.

-- The products the collectors are configured to poll, written by them at
-- startup — one row per (source, product), not per product.
--
-- WHY THE SOURCE IS IN THE KEY, THOUGH THE DIMENSION IS PER-PRODUCT.
--
-- "What instrument is `EURC-USDC`" has the same answer whoever measured it, so
-- the dimension view below aggregates this table to one row per product and no
-- consumer of the dimension sees the source at all. The source is in the key
-- for two other reasons, both of which need it:
--
-- 1. It makes "when did this product last produce data" a PRIMARY KEY PREFIX
--    lookup. `cex_prices` and `spot_ticks` key on `(source, product_id, …)`, so
--    `max(bucket_start) WHERE source = ? AND product_id = ?` is an index range
--    scan of one series — while the same question asked with `product_id`
--    alone is a full scan, because `product_id` leads no index. Holding the
--    source here is therefore what lets `instrument_liveness` below be cheap
--    without adding a secondary index to the two hottest tables in the store.
-- 2. It records WHICH collector polls WHICH product, which is the coverage
--    question directly: every source writing to the store should be reachable
--    on the ingestion dashboard, and that is now a query rather than a list
--    somebody maintains.
--
-- Registration is an upsert on `(source, product_id)`, so a restart is
-- idempotent, and the four collectors that poll EUR-USD — OANDA, Twelve Data,
-- Alpha Vantage and Pyth — get a row each rather than racing over one.
CREATE TABLE instrument_registry (
    -- The value this collector writes to `cex_prices.source` /
    -- `spot_ticks.source`. It must match exactly, or the liveness lookup finds
    -- no series and reports a live pair as silent.
    source              TEXT   NOT NULL,
    -- The canonical `BASE-QUOTE` id, matching `cex_prices.product_id` and
    -- `spot_ticks.product_id`. This is the join key for the whole dimension.
    product_id          TEXT   NOT NULL,
    -- When this product was first registered, and when a collector last
    -- confirmed it is still in its roster. The pair answers "is this pair
    -- still configured, or is its row a leftover from a roster it was dropped
    -- from" — a question the registry would otherwise be unable to answer,
    -- because nothing deletes a row when a pair leaves a roster.
    --
    -- Neither is a data-freshness signal, and neither may be read as one. Both
    -- track collector PROCESS starts: a collector whose venue has answered
    -- nothing for a week still refreshes `last_registered_at` every time it
    -- restarts. Freshness lives in `feed_health.last_ok_at` and in the
    -- measurement tables' own timestamps.
    first_registered_at BIGINT NOT NULL,
    last_registered_at  BIGINT NOT NULL,
    PRIMARY KEY (source, product_id),
    -- The same shape rule `parse_roster` enforces before a row can be written
    -- under this id: exactly one hyphen, non-empty upper-case legs. Stricter
    -- than `pyth_fx_feeds`' `^[A-Z]{3}-[A-Z]{3}$`, which is right for a table
    -- of fiat crosses and would reject EURC-USDC here.
    CONSTRAINT product_id_is_canonical
        CHECK (product_id ~ '^[A-Z0-9]{2,10}-[A-Z0-9]{2,10}$'),
    CONSTRAINT source_is_not_blank
        CHECK (source <> '')
);

-- The dimension as its consumers read it: a product, its two legs, and the
-- class the legs imply.
--
-- A view rather than stored columns, so the class of a pair cannot drift from
-- the kinds of its legs. Re-seeding a currency's kind reclassifies every pair
-- using it in one statement, with nothing to backfill.
--
-- WHY A LEFT JOIN, AND WHY 'unclassified' IS A VALUE RATHER THAN A NULL ROW.
--
-- An inner join would drop any product whose leg is not seeded above, which
-- would make this dimension SILENTLY INCOMPLETE: a class filter built on it
-- would hide the product's data entirely, and the panel would render a
-- perfectly plausible chart with a series missing. That is the same
-- silent-failure shape as a candlestick field map that fails to a flat line —
-- wrong information rather than missing information, and the reason this issue
-- carries dashboard-render verification at all.
--
-- So an unseeded leg yields a product that is present, visible, and labelled
-- 'unclassified'. That surfaces the missing `currency_kinds` row as a bucket
-- an operator can see, and keeps the product's series reachable while the seed
-- is fixed.
CREATE VIEW instruments AS
WITH product AS (
    -- One row per product, however many sources poll it — the source is in the
    -- registry's key for liveness and coverage, and no consumer of the
    -- dimension itself wants it. `min` and `max` rather than either alone: the
    -- earliest first sighting across sources, and the most recent confirmation
    -- from any of them.
    SELECT
        product_id,
        min(first_registered_at)          AS first_registered_at,
        max(last_registered_at)           AS last_registered_at,
        count(*)                          AS source_count,
        array_agg(source ORDER BY source) AS sources
    FROM instrument_registry
    GROUP BY product_id
)
SELECT
    p.product_id,
    split_part(p.product_id, '-', 1) AS base,
    split_part(p.product_id, '-', 2) AS quote,
    b.kind                           AS base_kind,
    q.kind                           AS quote_kind,
    CASE
        -- Unseeded legs first, so an unknown kind can never be silently
        -- absorbed into a class below by one of the broader arms.
        WHEN b.kind IS NULL OR q.kind IS NULL THEN 'unclassified'
        -- Any unpegged leg makes the pair a crypto pair whatever the other
        -- leg is: its volatility dominates, so grouping it with the FX or
        -- stablecoin pairs would put a percent-a-day series on an axis scaled
        -- for basis points.
        WHEN b.kind = 'crypto' OR q.kind = 'crypto' THEN 'crypto'
        WHEN b.kind = 'fiat' AND q.kind = 'fiat' THEN 'fx-pair'
        WHEN b.kind = 'stablecoin' AND q.kind = 'stablecoin' THEN 'stablecoin-pair'
        -- One of each: a stablecoin quoted against a sovereign currency, which
        -- is a PEG measurement rather than a rate. EURC-EUR is the case in
        -- point — it trades at ~1.0 and the only interesting thing about it is
        -- the deviation, whereas EURC-USDC is a EUR/USD rate wearing tokens.
        -- Lumping the two together as 'stablecoin-pair' would hide exactly
        -- what the peg pair exists to measure, so it gets its own class.
        ELSE 'peg-pair'
    END                              AS asset_class,
    -- Which collectors poll this product, and how many. This puts the coverage
    -- question in the dimension itself: a product nothing polls has no row at
    -- all, and one polled by a single source is visible as such rather than
    -- having to be inferred from a panel that happens to be empty.
    p.source_count,
    p.sources,
    p.first_registered_at,
    p.last_registered_at
FROM product p
LEFT JOIN currency_kinds b ON b.currency = split_part(p.product_id, '-', 1)
LEFT JOIN currency_kinds q ON q.currency = split_part(p.product_id, '-', 2);

COMMENT ON VIEW instruments IS
    'The instruments dimension: product_id, its base and quote legs, and the '
    'asset class derived from the legs'' currency kinds. Read this rather '
    'than instrument_registry, which carries no class.';

COMMENT ON COLUMN instrument_registry.last_registered_at IS
    'Epoch second a collector last confirmed this product is in its roster. A '
    'collector PROCESS-start signal, never a data-freshness one — read '
    'feed_health.last_ok_at for freshness.';

-- Which products are actually collecting, and which have gone quiet.
--
-- This is what the currency selector's default reads: the dashboard opens on
-- the pairs with data flowing now, and every other pair is opt-in. The point
-- is that it opens showing real data rather than empty panels for a pair whose
-- collector is parked.
--
-- WHY THERE ARE TWO THRESHOLDS, AND WHY THE CLASS PICKS BETWEEN THEM.
--
-- FX venues close for the weekend: a measured 48.1-hour gap, Friday 16:59 to
-- Sunday 17:04 New York, running 47 to 49 hours depending on the
-- daylight-saving transition. Any threshold under roughly 50 hours would
-- therefore mark every FX pair frozen every weekend — the precise false alarm
-- this view exists not to raise. Crypto and stablecoin venues never close, so
-- they take a tighter bound, and the loose one is reserved for the pairs that
-- legitimately go quiet.
--
-- `asset_class` already sorts one from the other at no extra cost: a
-- fiat-by-fiat pair trades only on FX venues, so session-bound *is* `fx-pair`.
--
-- **This is not a market calendar and must not grow into one.** It does not
-- know when a session is *expected* to be closed, only how long a silence has
-- to run before it stops being ordinary — which is all a default selection
-- needs. Calendar-aware liveness, with real session windows, is separate later
-- work.
CREATE VIEW instrument_liveness AS
WITH thresholds AS (
    -- The two constants, side by side and defined nowhere else. Change them
    -- here and every consumer follows.
    SELECT
        -- Clears a ~49-hour weekend with slack.
        72 * 3600 AS fx_pair_stale_secs,
        -- Still roomy enough for a daily-bar source (er-api, Alpha Vantage)
        -- that legitimately gaps 24-27 hours between publications.
        48 * 3600 AS always_open_stale_secs
),
last_seen AS (
    -- One index range scan per (source, product): a primary key PREFIX on both
    -- measurement tables, which is the whole reason the registry carries the
    -- source. A given source writes bars or ticks and never both, so one side
    -- of each pair below is NULL by construction rather than by accident.
    SELECT
        r.product_id,
        max(GREATEST(COALESCE(bars.last_at, 0), COALESCE(ticks.last_at, 0)))
            AS last_at
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
    GROUP BY r.product_id
),
bounded AS (
    SELECT
        i.product_id,
        i.asset_class,
        -- NULL rather than 0 for a product that has never produced a row. A
        -- registered pair whose venue has never answered is a genuinely
        -- different state from one that answered and stopped, and a 0 would
        -- render as 1970 on any panel that formatted it as a time.
        NULLIF(l.last_at, 0) AS last_data_at,
        CASE
            WHEN i.asset_class = 'fx-pair' THEN t.fx_pair_stale_secs
            ELSE t.always_open_stale_secs
        END                  AS stale_after_secs
    FROM instruments i
    JOIN last_seen l ON l.product_id = i.product_id
    CROSS JOIN thresholds t
)
SELECT
    product_id,
    asset_class,
    last_data_at,
    stale_after_secs,
    -- A pair that has never produced data is not live. Spelled out, because
    -- without the NULL guard the comparison would answer the question by
    -- accident rather than on purpose.
    last_data_at IS NOT NULL
        AND last_data_at > EXTRACT(EPOCH FROM now())::BIGINT - stale_after_secs
        AS is_live
FROM bounded;

COMMENT ON VIEW instrument_liveness IS
    'Per-product data freshness and a live/quiet verdict, with the staleness '
    'bound chosen by asset class so an FX weekend is not mistaken for a dead '
    'collector. Not a market calendar: it knows how long silence has run, not '
    'when a session is expected to be closed.';
