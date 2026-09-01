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

-- Every currency that appears as a leg of a product any collector is
-- configured to poll, as of this migration: the three FX vendors' default
-- roster (AUD-USD, EUR-USD, GBP-USD), Kraken's (USDC-USD, EURC-USD, EURC-EUR),
-- Coinbase's (EURC-USDC), and every leg of the Pyth roster seeded by `0005`
-- and widened by `0006`.
--
-- MYR and NGN are included though they have no live Pyth feed — `0006` records
-- that they are roster currencies served by other vendors, so a product using
-- them would otherwise land unclassified.
--
-- AUDD and MXNE are included though neither currently has a market: AUDD is
-- the prospective first customer's token and appears in the FX collectors'
-- history, and MXNE is in a roster with no aggregator coverage. Both are
-- cheap to state now and would otherwise show up as an unclassified product
-- the first time one is polled.
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
    -- Stablecoins. Each tracks the fiat of the same name, which is what makes
    -- a stablecoin-against-its-own-fiat pair a peg measurement rather than a
    -- rate — see the class derivation below.
    ('AUDD', 'stablecoin'),
    ('EURC', 'stablecoin'),
    ('MXNE', 'stablecoin'),
    ('USDC', 'stablecoin'),
    ('USDT', 'stablecoin');

-- No 'crypto' row is seeded. Nothing in any collector's roster is an unpegged
-- token today, and a seeded currency with no product would be reference data
-- describing nothing. The kind is in the CHECK above so the class derivation
-- is total when one arrives.

-- The products the collectors are configured to poll, written by them at
-- startup.
--
-- Registration is an upsert keyed on the canonical id, so a restart is
-- idempotent and several collectors polling the same pair — EUR-USD is on
-- OANDA, Twelve Data, Alpha Vantage and Pyth — converge on one row rather than
-- one row per source. The dimension is deliberately per-PRODUCT, not per
-- (source, product): "what instrument is this" has the same answer whoever
-- measured it, and a per-source dimension would make every class filter fan
-- out across sources for no gain.
CREATE TABLE instrument_registry (
    -- The canonical `BASE-QUOTE` id, matching `cex_prices.product_id` and
    -- `spot_ticks.product_id`. This is the join key for the whole dimension.
    product_id          TEXT   PRIMARY KEY,
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
    -- The same shape rule `parse_roster` enforces before a row can be written
    -- under this id: exactly one hyphen, non-empty upper-case legs. Stricter
    -- than `pyth_fx_feeds`' `^[A-Z]{3}-[A-Z]{3}$`, which is right for a table
    -- of fiat crosses and would reject EURC-USDC here.
    CONSTRAINT product_id_is_canonical
        CHECK (product_id ~ '^[A-Z0-9]{2,10}-[A-Z0-9]{2,10}$')
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
SELECT
    r.product_id,
    split_part(r.product_id, '-', 1) AS base,
    split_part(r.product_id, '-', 2) AS quote,
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
    r.first_registered_at,
    r.last_registered_at
FROM instrument_registry r
LEFT JOIN currency_kinds b ON b.currency = split_part(r.product_id, '-', 1)
LEFT JOIN currency_kinds q ON q.currency = split_part(r.product_id, '-', 2);

COMMENT ON VIEW instruments IS
    'The instruments dimension: product_id, its base and quote legs, and the '
    'asset class derived from the legs'' currency kinds. Read this rather '
    'than instrument_registry, which carries no class.';

COMMENT ON COLUMN instrument_registry.last_registered_at IS
    'Epoch second a collector last confirmed this product is in its roster. A '
    'collector PROCESS-start signal, never a data-freshness one — read '
    'feed_health.last_ok_at for freshness.';
