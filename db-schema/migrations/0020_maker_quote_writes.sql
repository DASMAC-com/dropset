-- cspell:word trunc
--
-- Quote-burn telemetry: one row per confirmed quote write the maker sent, with
-- what it cost the leader, plus hourly and daily rollup views.
--
-- What the rows are for, how the bot computes the two fee columns, and why
-- that figure matches what the chain charged are current claims about the
-- bot's behavior, so they live in docs/market-making.md §6 rather than here —
-- an applied migration cannot be corrected.
--
-- **Per write, not per tick.** A row exists for each confirmed send, so
-- counting rows measures the cadence the bot actually sent at. Neither
-- `maker_telemetry` (per tick, whether or not anything was sent) nor
-- `maker_ladder_epoch` (per cold-path re-arm only) can answer that.
--
-- **`kind` is TEXT without a CHECK**, following `0003`'s rule for evolving
-- vocabularies: a new write kind must never turn the row reporting it into a
-- constraint violation.
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
    -- Which write: the bot's vocabulary is in docs/market-making.md §6.
    kind                  TEXT   NOT NULL,
    -- The transaction signature, base58.
    signature             TEXT   PRIMARY KEY,
    -- The per-signature base fee, in lamports.
    base_fee_lamports     BIGINT NOT NULL,
    -- The compute-unit-price surcharge, in lamports.
    priority_fee_lamports BIGINT NOT NULL,
    CONSTRAINT fees_are_non_negative
        CHECK (base_fee_lamports >= 0 AND priority_fee_lamports >= 0)
);

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
