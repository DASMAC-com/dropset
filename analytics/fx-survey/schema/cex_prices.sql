-- cspell:word chrono
-- The survey's CEX reference-candle table (docs/fx-survey.md §5): one row per
-- OHLCV bucket from a centralized exchange, the Coinbase EURC/USDC feed (§4)
-- being the first and only filler for the gate. Keyed by pair so a second
-- currency is additive, and by granularity so minute and coarser series can
-- coexist. Every write is idempotent (ON CONFLICT DO NOTHING on the PK), so a
-- re-fetched backfill window — the store sink's at-least-once contract
-- (docs/data-feeds.md §3) — is absorbed.
--
-- Applied by the `fx-survey-migrate` runner as idempotent DDL, not through a
-- second `sqlx::migrate!` migrator: the framework's `PgCursorStore::migrate`
-- already owns the `_sqlx_migrations` table for `feed_cursors`, and a second
-- migrator on the same database would collide on it. `feeds/tests/
-- store_postgres.rs` is the precedent — the framework migrates its cursor
-- table, the consumer creates its own with plain `CREATE TABLE`.
--
-- `bucket_start` is the epoch-second bucket open, stored as BIGINT to match the
-- indexer's `block_time` and keep the crate free of a chrono/time dependency;
-- analyses wrap it in `to_timestamp(...)` for session / regime slicing.
-- Prices and volume are DOUBLE PRECISION: an FX-stablecoin rate sits near 1.0,
-- so f64's ~15 significant digits are exact well past the bps the analyses
-- measure (unlike the indexer's u64 atoms, which need NUMERIC to avoid i64
-- truncation).
CREATE TABLE IF NOT EXISTS cex_prices (
    source           TEXT             NOT NULL,
    product_id       TEXT             NOT NULL,
    granularity_secs INTEGER          NOT NULL,
    bucket_start     BIGINT           NOT NULL,
    low              DOUBLE PRECISION NOT NULL,
    high             DOUBLE PRECISION NOT NULL,
    open             DOUBLE PRECISION NOT NULL,
    close            DOUBLE PRECISION NOT NULL,
    volume           DOUBLE PRECISION NOT NULL,
    PRIMARY KEY (source, product_id, granularity_secs, bucket_start)
);

-- The dominant analysis access pattern is a time-ordered scan of one series
-- (lead-lag, dislocation overlay); index the pair by bucket for it.
CREATE INDEX IF NOT EXISTS cex_prices_series_idx
    ON cex_prices (source, product_id, granularity_secs, bucket_start);
