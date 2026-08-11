-- cspell:word chrono
-- The opening history for the shared `dropset` database
-- (docs/data-feeds.md §8). One migration, deliberately: this squashes the
-- three DDL regimes that preceded a single schema owner — the framework's
-- `feed_cursors` migration, the indexer's `0001_init`, and the market-data
-- `cex_prices` table that was applied as idempotent startup DDL — into one
-- starting point. Nothing was deployed and no instance held data worth
-- keeping, so there was no applied history to preserve. That is a one-time
-- concession to restarting: every migration from `0002` on is a separate,
-- ordered file scoped to the change it makes.
--
-- Plain `CREATE TABLE`, not `CREATE TABLE IF NOT EXISTS`. A versioned
-- migration runs exactly once against a given database, so `IF NOT EXISTS`
-- would only mask an object this migration did not create — precisely the
-- drift a single schema owner exists to make visible. A failure here means
-- the target database already carries tables from the retired per-app
-- regimes; recreate it (every local store is ephemeral) and re-run.
--
-- Table ownership — one writer per table, unrestricted readers — is the
-- matrix in docs/data-feeds.md §8, not a property of this file.

-- Framework tier ───────────────────────────────────────────────────────

-- Framework-owned cursor store (docs/data-feeds.md §3). One row per feed,
-- keyed by `Source::name`, holding that source's opaque JSON resume position
-- (a CEX feed stores `{ "next_start": <epoch> }`, an RPC feed a signature or
-- slot). The store sink upserts this after each committed batch; a poll source
-- reads it at startup to resume. A forward-only (live) feed never writes here.
--
-- This is §8's one ownership carve-out: several apps write it, but they
-- partition by feed name, and the framework — not any single app — defines
-- its shape.
CREATE TABLE feed_cursors (
    feed       TEXT        PRIMARY KEY,
    cursor     JSONB       NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Indexer raw tier ─────────────────────────────────────────────────────

-- The indexer's two tiers (docs/indexer.md §5): raw, immutable, append-only
-- event tables keyed on the frozen primary key
-- (slot, txn_index, signature, event_ordinal); and derived rollup tables the
-- watermarked aggregator owns. Every raw write is idempotent
-- (ON CONFLICT DO NOTHING), so a replayed slot is a no-op — the PK is the
-- dedup contract end to end.

-- Fill legs: the typed, high-cardinality, rollup-critical event. One row
-- per matched (sector_idx, level_idx) leg. u64 atoms are NUMERIC (a BIGINT
-- bind would truncate values above i64::MAX); the Price key is its u32
-- bits.
CREATE TABLE fill_events (
    slot              BIGINT   NOT NULL,
    txn_index         BIGINT   NOT NULL,
    signature         TEXT     NOT NULL,
    event_ordinal     BIGINT   NOT NULL,
    block_time        BIGINT,
    market            TEXT     NOT NULL,
    taker             TEXT     NOT NULL,
    leader            TEXT     NOT NULL,
    quote_authority   TEXT     NOT NULL,
    side              SMALLINT NOT NULL,
    sector_idx        BIGINT   NOT NULL,
    level_idx         BIGINT   NOT NULL,
    fill_base         NUMERIC  NOT NULL,
    fill_quote        NUMERIC  NOT NULL,
    fill_price        BIGINT   NOT NULL,
    base_atoms_after  NUMERIC  NOT NULL,
    quote_atoms_after NUMERIC  NOT NULL,
    nonce_after       NUMERIC  NOT NULL,
    taker_fee_atoms   NUMERIC  NOT NULL,
    PRIMARY KEY (slot, txn_index, signature, event_ordinal)
);

CREATE INDEX fill_events_market_idx ON fill_events (market);
CREATE INDEX fill_events_txn_idx ON fill_events (signature, txn_index);

-- Every other event, kept at full fidelity as the decoded JSON payload.
-- The lifecycle events (Deposit / Withdraw / CreateVault / CloseVault /
-- FreezeVault / Realize) and the admin retuning events (SetMarketFeeConfig
-- &c., which teardown reconstructs from history) all land here, keyed and
-- queryable by `kind` / `market`.
CREATE TABLE events (
    slot          BIGINT NOT NULL,
    txn_index     BIGINT NOT NULL,
    signature     TEXT   NOT NULL,
    event_ordinal BIGINT NOT NULL,
    block_time    BIGINT,
    kind          TEXT   NOT NULL,
    market        TEXT,
    payload       JSONB  NOT NULL,
    PRIMARY KEY (slot, txn_index, signature, event_ordinal)
);

CREATE INDEX events_kind_idx ON events (kind);
CREATE INDEX events_market_idx ON events (market);

-- Indexer derived tier (owned by the aggregator) ───────────────────────

-- One row per take: the (signature, txn_index) group of fill legs. This is
-- the take-level view interface.md §1 calls "derived, not emitted". Recomputed
-- from all of a take's legs on each pass, so re-folding is idempotent.
CREATE TABLE takes (
    signature        TEXT     NOT NULL,
    txn_index        BIGINT   NOT NULL,
    slot             BIGINT   NOT NULL,
    block_time       BIGINT,
    market           TEXT     NOT NULL,
    taker            TEXT     NOT NULL,
    side             SMALLINT NOT NULL,
    leg_count        INTEGER  NOT NULL,
    total_fill_base  NUMERIC  NOT NULL,
    total_fill_quote NUMERIC  NOT NULL,
    total_taker_fee  NUMERIC  NOT NULL,
    avg_price        DOUBLE PRECISION,
    PRIMARY KEY (signature, txn_index)
);

CREATE INDEX takes_market_idx ON takes (market, slot);

-- Per-market rollup: last price + raw and self-trade-adjusted volume. The
-- prototype populates the raw figures; the self-trade-adjusted columns wait
-- on the off-chain wash-clustering pipeline (interface.md §1, volume
-- integrity — never silently net).
CREATE TABLE market_stats (
    market                TEXT    PRIMARY KEY,
    last_price            DOUBLE PRECISION,
    last_slot             BIGINT  NOT NULL DEFAULT 0,
    take_count            BIGINT  NOT NULL DEFAULT 0,
    volume_base           NUMERIC NOT NULL DEFAULT 0,
    volume_quote          NUMERIC NOT NULL DEFAULT 0,
    volume_base_adjusted  NUMERIC,
    volume_quote_adjusted NUMERIC
);

-- Singleton watermark: the last event coordinate the aggregator folded.
-- Carries the full event PK (incl. `signature`): the RPC path leaves
-- `txn_index` at 0, so two takes in one slot share `(slot, txn_index,
-- event_ordinal)` — `signature` is what makes the watermark tuple unique,
-- so a strict `>` advance never skips an unfolded leg.
CREATE TABLE indexer_cursor (
    id                 SMALLINT PRIMARY KEY DEFAULT 1,
    last_slot          BIGINT NOT NULL DEFAULT 0,
    last_txn_index     BIGINT NOT NULL DEFAULT 0,
    last_event_ordinal BIGINT NOT NULL DEFAULT 0,
    last_signature     TEXT   NOT NULL DEFAULT '',
    CONSTRAINT indexer_cursor_singleton CHECK (id = 1)
);

INSERT INTO indexer_cursor (id) VALUES (1);

-- Market-data tier ─────────────────────────────────────────────────────

-- CEX reference candles: one row per OHLCV bucket from a centralized
-- exchange, the Coinbase feed being the first filler. Keyed by pair so a
-- second currency is additive, and by granularity so minute and coarser
-- series can coexist. Every write is idempotent (ON CONFLICT DO NOTHING on
-- the PK), so a re-fetched backfill window — the store sink's at-least-once
-- contract (docs/data-feeds.md §3) — is absorbed.
--
-- `bucket_start` is the epoch-second bucket open, stored as BIGINT to match
-- the indexer's `block_time` and keep the collector free of a chrono/time
-- dependency; analyses wrap it in `to_timestamp(...)` for session / regime
-- slicing. Prices and volume are DOUBLE PRECISION: an FX-stablecoin rate
-- sits near 1.0, so f64's ~15 significant digits are exact well past the bps
-- the analyses measure (unlike the indexer's u64 atoms, which need NUMERIC
-- to avoid i64 truncation).
CREATE TABLE cex_prices (
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

-- No secondary index: the dominant access pattern is a time-ordered scan of
-- one series (lead-lag, dislocation overlay), and the primary key's implicit
-- index — leading pair/granularity equality, trailing bucket_start ordered —
-- already serves it. A separate index on the same tuple would be pure write
-- overhead.
