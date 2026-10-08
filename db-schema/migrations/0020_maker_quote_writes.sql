-- cspell:word trunc
--
-- Quote-burn telemetry: one row per confirmed quote write the maker sent, with
-- what it cost the leader, rolled up per hour and per day.
--
-- **What this is for.** Every quote write is a transaction the leader pays
-- for, and on mainnet that is real SOL. The ladder's rungs expire on a
-- 36 / 120 / 480 / 2880-second wall-clock TIF, so the bot must re-stamp often
-- enough to keep them live — and the tradeoff between a longer TIF (fewer
-- writes, more exposure on an ungraceful death) and a shorter one (more
-- writes) is priced in exactly these rows. Neither `maker_telemetry` nor
-- `maker_ladder_epoch` can answer it: the first is per tick whether or not
-- anything was sent, the second per re-arm of the cold path only.
--
-- **Per write, at the cadence the bot actually sent.** The hot path's drift,
-- skew and heartbeat triggers fire irregularly, so the write rate is not
-- derivable from the trigger config; counting rows is the honest measure. The
-- volume is small — a re-stamp every ~30 s heartbeat is ~2.9k rows/day/market
-- at the floor — so no retention policy is needed yet.
--
-- **The fee columns are computed from the fee schedule, not read back.** The
-- bot knows both inputs at send time — one signer, and the compute-unit price
-- it attached — and the runtime charges a priority fee on the *requested*
-- compute-unit limit rather than the units consumed, so the computed figure is
-- what the runtime charged. Reading it back with `getTransaction` would add a
-- round trip to every write on the quote path. `base_fee_lamports` is the
-- per-signature fee; `priority_fee_lamports` is zero except on the kill
-- stamp, the one write sent with a compute-unit price.
--
-- **`kind` is TEXT without a CHECK**, following `0003`'s rule for evolving
-- vocabularies: a new write kind must never turn the row reporting it into a
-- constraint violation. Today it is `reference`, `kill` or `profile`.
--
-- **Keyed on `signature`.** A signature is unique on chain, so the key
-- deduplicates a redelivered write and nothing else.
--
-- The views bucket in UTC explicitly, so the daily rollup's day boundary does
-- not move with the session `TimeZone` a reader happens to connect with.
-- `0002_reader_role`'s default privileges already grant `dropset_ro` `SELECT`
-- on new relations in `public`, views included, so no grant accompanies them.
CREATE TABLE maker_quote_writes (
    -- Unix seconds, the tick that sent the write. Shares a clock and a unit
    -- with `maker_telemetry.ts`.
    ts                    BIGINT NOT NULL,
    -- The market's symbol (`EURC`), matching `maker_telemetry.market`.
    market                TEXT   NOT NULL,
    -- `reference`, `kill` or `profile`.
    kind                  TEXT   NOT NULL,
    -- The transaction signature, base58.
    signature             TEXT   PRIMARY KEY,
    base_fee_lamports     BIGINT NOT NULL,
    priority_fee_lamports BIGINT NOT NULL,
    CONSTRAINT fees_are_non_negative
        CHECK (base_fee_lamports >= 0 AND priority_fee_lamports >= 0)
);

-- The rollups and the dashboard read one market's writes over a time range.
CREATE INDEX maker_quote_writes_market_ts ON maker_quote_writes (market, ts);

CREATE VIEW maker_quote_burn_hourly AS
SELECT
    date_trunc('hour', to_timestamp(ts), 'UTC') AS bucket,
    market,
    kind,
    count(*) AS writes,
    sum(base_fee_lamports) AS base_fee_lamports,
    sum(priority_fee_lamports) AS priority_fee_lamports,
    sum(base_fee_lamports + priority_fee_lamports) AS total_fee_lamports
FROM maker_quote_writes
GROUP BY 1, 2, 3;

CREATE VIEW maker_quote_burn_daily AS
SELECT
    date_trunc('day', to_timestamp(ts), 'UTC') AS bucket,
    market,
    kind,
    count(*) AS writes,
    sum(base_fee_lamports) AS base_fee_lamports,
    sum(priority_fee_lamports) AS priority_fee_lamports,
    sum(base_fee_lamports + priority_fee_lamports) AS total_fee_lamports
FROM maker_quote_writes
GROUP BY 1, 2, 3;
