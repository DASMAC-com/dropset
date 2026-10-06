-- The maker's quote ladder as it was actually ARMED: one row per level per
-- side, written when the shape changes rather than on every tick.
--
-- **What this closes.** `0003_maker_telemetry` records the *touch* — `best_bid`
-- and `best_ask`, derived from the ladder's tightest level — and deliberately
-- nothing about the levels behind it. So a dashboard can show where the book
-- starts and has no way to show its shape: how far out the deeper levels sit,
-- how much size each carries, or whether a reshape actually took effect. That
-- is the spread *profile*, and it is what an operator tuning liquidity needs to
-- see beside the fair price.
--
-- **Why per-change and NOT per-tick, which is the whole design.** The naive
-- shape is one row per level per side per tick, and it is unaffordable for what
-- it buys: at the 5 s tick that is 8 rows/tick/market, ~138k rows/day/market,
-- or ~415k/day across the three-market roster. `0007_fair_price_fusion` already
-- names `maker_leg_contributions` at ~100k rows/day/market as the
-- fastest-growing of the maker's tables and the first that will want a
-- retention policy; a per-tick ladder would land ~1.4x that and become the
-- largest table in the schema, in a schema that has no retention policy
-- anywhere yet.
--
-- It would also be almost entirely redundant, because **the per-level price is
-- already derivable from columns `0003` stores**. A level's price is a pure
-- function of the reference and the level's offset:
--
--     bid = on_chain_reference * (1 - offset_ppm / 1e6)
--     ask = on_chain_reference * (1 + offset_ppm / 1e6)
--
-- which is the arithmetic `telemetry::SampleBuilder::touch` applies to the
-- tightest level, applied to each. `on_chain_reference` is on every telemetry
-- row, so the only thing missing was the ladder's *shape* — and the shape is
-- near-constant: it changes on a re-arm, which is a restart, a reshape, a
-- freeze-side, a halt, or the daily heartbeat. Recording the shape per epoch
-- and deriving the price per tick is therefore not an approximation of the
-- per-tick table; it yields the same numbers for ~4 orders of magnitude fewer
-- rows. Retention is not needed and is not deferred-with-a-note: at tens of
-- rows/day/market there is nothing to prune.
--
-- **`size_bps` is what was ARMED, not what was configured**, and that is the
-- reason this table stores sizes at all rather than leaving them to be read
-- from the bot's config. `model::ladder::scale_side` (the > 30% reshape) and
-- `zero_side` (the freeze) rewrite per-level sizes at runtime — flooring, then
-- renormalizing if a side would exceed BPS — so the resting sizes routinely
-- differ from `DEFAULT_LADDER`. Reading sizes from config would show the
-- operator the shape the bot intended rather than the one it armed, which is
-- precisely the divergence a tuning session is looking for.
--
-- **Offsets, by contrast, are never rewritten.** Both reshape primitives touch
-- `size_bps` only, leaving `price_offset` as `build_profile` set it, so the
-- touch is unchanged by a reshape. The offsets are stored anyway and for the
-- same reason the sizes are: a reader deriving prices must not have to also
-- know which mutations preserve which field.
--
-- **This table says nothing about whether a side was LIVE.** Darkness is a
-- property of the tick, not of the armed shape: a side goes dark through a
-- frozen vault, an invalid reference, an armed halt, or a freeze-side, and all
-- four are already discriminated on the telemetry row (`frozen`,
-- `reference_valid`, `profile_kind`). A consumer joins to that row and applies
-- the same gates `touch` does. Encoding liveness here would duplicate a
-- four-valued decision in the one place that cannot observe it, and the
-- duplicate would be the copy that goes stale.
--
-- **Only the levels the ladder DEFINES get rows.** The on-chain profile has
-- `N_LEVELS` (8) slots and `build_profile` fills the first `ladder.len()` (4
-- today), leaving the tail zeroed. A zeroed slot is `offset_ppm = 0` with
-- `size_bps = 0` — inert on-chain, but a row for it would plot a level sitting
-- exactly at the reference, which is a line at the fair price that no one is
-- quoting. The writer therefore emits rows only for defined levels, so the
-- level count is a property of the epoch and a future longer ladder needs no
-- migration.
--
-- **`side` takes a CHECK where every other enum-ish column in this schema does
-- not**, which is a deliberate departure worth stating. `0003`'s rule is that a
-- new variant must never turn a telemetry write into a constraint violation
-- that fails the very write reporting the problem — a rule about *evolving*
-- vocabularies like `regime` and `health`. A book side is not one: the program
-- has exactly two, fixed by the on-chain layout. And the failure a CHECK
-- prevents here is the kind that cannot be noticed — a mislabelled side plots
-- bids among the asks and reads as a crossed book rather than as a bad write.
-- `profile_kind` keeps the TEXT-without-CHECK treatment, matching `0003`,
-- because that one really does evolve.
--
-- **Keyed `(market, armed_at, side, level_idx)`.** The cold path sends at most
-- one `SetLiquidityProfile` per market per cycle and every arm path returns
-- immediately after it, so two re-arms cannot share a market and a wall-clock
-- second at the 5 s tick. No extra index: the consuming query asks for the
-- epoch in force at a tick ("latest `armed_at` <= ts, for this market"), which
-- the key's `(market, armed_at)` prefix already serves.
--
-- **Grafana reads this; nothing surfaces it in the TUI**, per the standing
-- surface split. `0002_reader_role` already grants `SELECT` on future tables in
-- `public` to `dropset_ro`, so this needs no accompanying grant.
CREATE TABLE maker_ladder_epoch (
    -- Unix seconds, the tick that armed this shape. Shares a clock and a unit
    -- with `maker_telemetry.ts` so the two join without conversion.
    armed_at     BIGINT   NOT NULL,
    -- The market's symbol (`EURC`), matching `maker_telemetry.market` rather
    -- than the pubkey — this table is joined to telemetry, never to the
    -- indexer's pubkey-keyed tables.
    market       TEXT     NOT NULL,
    -- `bid` or `ask`.
    side         TEXT     NOT NULL,
    -- 0 is the tightest level. The ladder is validated monotonic in
    -- `offset_ppm`, so level order is also distance order.
    level_idx    SMALLINT NOT NULL,
    -- Offset from the reference, in ppm — bids subtract, asks add.
    offset_ppm   BIGINT   NOT NULL,
    -- The level's share of its side's committed size, in bps, as armed.
    size_bps     INTEGER  NOT NULL,
    -- Which shape this epoch is, in the same `Debug`-rendered vocabulary
    -- `maker_telemetry.profile_kind` uses (`Standard`, `Reshaped(Bid)`,
    -- `FrozenSide(Ask)`, `Halted`) so the two columns can be correlated
    -- directly.
    profile_kind TEXT     NOT NULL,
    PRIMARY KEY (market, armed_at, side, level_idx),
    CONSTRAINT side_is_a_book_side
        CHECK (side IN ('bid', 'ask')),
    CONSTRAINT ladder_fields_are_non_negative
        CHECK (offset_ppm >= 0 AND size_bps >= 0)
);
