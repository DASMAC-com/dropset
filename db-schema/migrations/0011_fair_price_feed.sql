-- The fair-price estimator's own output, per pair per tick: the seam between
-- the estimator process and everything that consumes a fair value
-- (docs/market-making.md §1 fair-price estimation).
--
-- **Why this is a new table when 0007 said the series needed none.** That note
-- was correct for its moment and this does not contradict it: while the maker
-- was the only thing computing a fair value, `maker_telemetry.fair` was the
-- series, and adding a second home for it would have been duplication. What
-- this table anticipates is not a change in the arithmetic but a change of
-- *owner*: it exists for an estimator that will own the computation as its own
-- process, whose output has to outlive any one consumer's telemetry.
--
-- That process does **not** exist as of this migration. This table is the
-- output half of the seam, landing first so the contract is fixed before
-- anything writes to it; the statements below about two makers and about
-- independent tick clocks describe the arrangement it is built for, not the
-- one running today.
--
-- So the two columns answer different questions and both are kept:
--
--   * `fair_price.fair` (here) — what the **estimator published** for this pair.
--   * `maker_telemetry.fair` — what a **maker actually priced off** for one
--     market. It stays maker-keyed and maker-owned.
--
-- They are not redundant and neither is derivable from the other. A maker that
-- is halted, has no inventory, or declines a `Paused` estimate writes no
-- composition while the estimator keeps publishing; two makers on one pair would
-- write two telemetry rows against this table's one. **A reader asking "what
-- was the fair price" wants this table; one asking "what did the bot quote
-- against" wants the telemetry.** Joining them on `ts` will not line up either —
-- the two processes tick on their own clocks, so a consumer correlating them
-- must join on a window, not on equality.
--
-- **Keyed by `product_id`, not by market**, because the estimator is a per-pair
-- service and a market is a consumer's notion: several Dropset markets can quote
-- one pair, and the estimator has no opinion about how many. The id is the
-- canonical `BASE-QUOTE` of the instruments dimension (0009), so this table
-- joins to `instrument_registry`, `spot_ticks` and `cex_prices` without a
-- translation step. The engine carries a per-market basis EMA, so an estimator
-- holds one engine instance per key it publishes under — the same constraint,
-- read at the level this table keys.
--
-- **`ts` is the ESTIMATOR's stamp, and that is the point of the column.** A
-- consumer ages the value from this, never from its own read time — a fair value
-- fetched promptly from a stalled estimator is stale in substance and would
-- otherwise read as fresh. Publishing the stamp is what lets the honest-ageing
-- discipline survive the extra process hop.
--
-- **Nullability means unknown, never zero**, as it does throughout 0003 and
-- 0007. A panel rendering a NULL `fair` as 0 would draw a pause as a collapse to
-- zero, which is the most misleading reading available.
--
-- **Deliberately not here: per-leg and per-source attribution.** This table
-- carries the composition and its guard flags, and no breakdown of which
-- source contributed what. Nothing here discharges that.
--
-- Be precise about what that costs, because "it lives in the maker's telemetry
-- tables" would be too generous: for exactly the ticks this table exists to
-- capture — the estimator publishing while no maker composed — attribution is
-- recorded nowhere at all, not elsewhere. Per-leg staleness is in the same
-- position. The mutable docs carry the current plan for both.
--
-- **Disclosure.** 0002 grants the read-only `dropset_ro` role SELECT on every
-- table in `public`, so a dashboard reader sees this.
--
-- What licenses it is **precedent, not derivation**: 0003 already exposes
-- `fair`, `anchor`, `regime`, `health`, `uncertain`, `basis`, `basis_breach`
-- and `usdc_breach` to that same role on `maker_telemetry`. This table
-- discloses no vocabulary that is not already disclosed, which is the whole of
-- the argument.
--
-- Stated that way on purpose, because the tempting argument is weaker than it
-- looks. "Every number here is a deterministic function of public venue quotes
-- and calibration constants committed in the repository" is true of the
-- derivation and does **not** establish non-disclosure: the role is held by
-- whoever has the dashboard password, who need not have repo access, and to
-- that reader a guard flag flipping against quotes they also hold is an
-- observation of where the band sits. `uncertain` says "quote, but widen".
-- Precedent covers these columns; determinism would not have.
--
-- No credential, inventory figure, or venue-produced free text appears —
-- `anchor`, `regime`, `degrade` and `health` are compile-time constants, the
-- rest are numbers — and none may be added without extending this argument.
CREATE TABLE fair_price (
    -- Unix seconds, the estimator's own tick stamp. See the note above.
    ts             BIGINT           NOT NULL,
    -- Canonical `BASE-QUOTE`, matching `instrument_registry.product_id`.
    product_id     TEXT             NOT NULL,
    -- The quoting mid in quote units per base unit, NULL exactly when `regime`
    -- is `paused` — the estimator had no usable leg and published no mid.
    fair           DOUBLE PRECISION,
    -- Which leg anchored the mid: `fx`, `crypto_reference`, `static`, `none`.
    anchor         TEXT             NOT NULL,
    -- The composition regime: `normal`, `crypto_only`, `fx_pinned`,
    -- `uncorroborated`, `degraded`, `paused`.
    --
    -- TEXT rather than an enum type or a smallint, for the reason every enum-ish
    -- column in 0003 is TEXT: a new variant must not turn a telemetry write into
    -- a constraint violation that fails the very write reporting the problem.
    --
    -- Note `degraded` collapses a variant that carries a payload in the engine;
    -- `degrade` below is that payload, split out rather than encoded into this
    -- string so a reader can filter on the regime without parsing it.
    regime         TEXT             NOT NULL,
    -- Which degrade, when `regime` is `degraded`. NULL in every other regime —
    -- meaning "not degraded", not "degraded for an unknown reason".
    degrade        TEXT,
    -- The kill-switch health gate: `ok`, `unverified`, `degraded`, `pause`.
    --
    -- A total function of `regime` in the engine, and stored anyway: it is the
    -- documented kill-switch axis, a consumer reading it should not have to
    -- reimplement the mapping, and a stored copy makes a future divergence
    -- visible in the data rather than silent.
    health         TEXT             NOT NULL,
    -- The smoothed basis, set only in an FX-anchored regime — there is no basis
    -- without an FX anchor to divide the crypto reference by.
    basis          DOUBLE PRECISION,
    -- How long ago the value in `basis` was last *observed*, floored to whole
    -- seconds — so 0 means "observed less than a second ago" rather than
    -- "folded on this exact tick", and the first tick or two after a fold can
    -- still read 0. That resolution is deliberate: the age is read against
    -- multi-second freshness bounds, and finer precision would imply the
    -- estimator ticks faster than it does.
    --
    -- The point of the column is the coarse distinction: without it a basis
    -- smoothed six seconds ago and one smoothed five days ago are
    -- indistinguishable, so the operator cannot tell a live correction from a
    -- carried one.
    --
    -- NULL whenever `basis` is NULL, and also for a pinned basis, which is a
    -- constant and has no observation age.
    basis_age_secs BIGINT,
    -- The basis leg answered, but its reading was refused as an outlier rather
    -- than folded. Distinct from `basis_breach`: that says the *smoothed* basis
    -- has left its sane band, this says a *single* reading was too far from the
    -- running estimate to be credible. A run of these is what a sick source
    -- looks like before it has moved anything.
    basis_outlier  BOOLEAN          NOT NULL,
    -- The FX anchor is fresh but too uncertain (§1 fm6): quote, but widen. Never
    -- set in a non-FX regime.
    uncertain      BOOLEAN          NOT NULL,
    -- The smoothed basis is outside its sane band — a peg event (§4). Only
    -- meaningful when `basis` is non-NULL.
    basis_breach   BOOLEAN          NOT NULL,
    -- The USDC/USD reading is outside its common-mode band — a portfolio-wide
    -- event (§1 fm1, §4). Evaluated whenever a USDC/USD reading is live, in any
    -- regime, so it is meaningful on every row.
    usdc_breach    BOOLEAN          NOT NULL,
    PRIMARY KEY (product_id, ts),
    -- The same shape 0009 enforces on `instrument_registry.product_id`. Declared
    -- again rather than inherited: a check constraint is per-table, and a pair
    -- id that cannot join to the dimension is a silently orphaned series.
    CONSTRAINT fair_price_product_id_is_canonical
        CHECK (product_id ~ '^[A-Z0-9]{2,10}-[A-Z0-9]{2,10}$')
);

-- The dashboards scan recent rows across all pairs, which the primary key cannot
-- serve — it orders by pair first, so "the last 15 minutes over every pair" is a
-- full scan. Same reasoning, and same shape, as the index 0007 added for the
-- same access pattern.
CREATE INDEX fair_price_ts_idx ON fair_price (ts);

COMMENT ON TABLE fair_price IS
    'The fair-price estimator''s published output, one row per pair per tick. '
    'Distinct from maker_telemetry.fair, which is what a maker priced off.';
