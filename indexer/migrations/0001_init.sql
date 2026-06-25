-- Dropset indexer schema (docs/indexer.md §5). Two tiers: raw, immutable,
-- append-only event tables keyed on the frozen primary key
-- (slot, txn_index, signature, event_ordinal); and derived rollup tables
-- the watermarked aggregator owns. Every raw write is idempotent
-- (ON CONFLICT DO NOTHING), so a replayed slot is a no-op — the PK is the
-- dedup contract end to end.

-- Raw tier ────────────────────────────────────────────────────────────

-- Fill legs: the typed, high-cardinality, rollup-critical event. One row
-- per matched (sector_idx, level_idx) leg. u64 atoms are NUMERIC (a BIGINT
-- bind would truncate values above i64::MAX); the Price key is its u32
-- bits.
CREATE TABLE IF NOT EXISTS fill_events (
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

CREATE INDEX IF NOT EXISTS fill_events_market_idx ON fill_events (market);
CREATE INDEX IF NOT EXISTS fill_events_txn_idx
    ON fill_events (signature, txn_index);

-- Every other event, kept at full fidelity as the decoded JSON payload.
-- The lifecycle events (Deposit / Withdraw / CreateVault / CloseVault /
-- FreezeVault / Realize) and the admin retuning events (SetMarketFeeConfig
-- &c., which teardown reconstructs from history) all land here, keyed and
-- queryable by `kind` / `market`.
CREATE TABLE IF NOT EXISTS events (
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

CREATE INDEX IF NOT EXISTS events_kind_idx ON events (kind);
CREATE INDEX IF NOT EXISTS events_market_idx ON events (market);

-- Derived tier (owned by the aggregator) ───────────────────────────────

-- One row per take: the (signature, txn_index) group of fill legs. This is
-- the take-level view interface.md §1 calls "derived, not emitted". Recomputed
-- from all of a take's legs on each pass, so re-folding is idempotent.
CREATE TABLE IF NOT EXISTS takes (
    signature         TEXT     NOT NULL,
    txn_index         BIGINT   NOT NULL,
    slot              BIGINT   NOT NULL,
    block_time        BIGINT,
    market            TEXT     NOT NULL,
    taker             TEXT     NOT NULL,
    side              SMALLINT NOT NULL,
    leg_count         INTEGER  NOT NULL,
    total_fill_base   NUMERIC  NOT NULL,
    total_fill_quote  NUMERIC  NOT NULL,
    total_taker_fee   NUMERIC  NOT NULL,
    avg_price         DOUBLE PRECISION,
    PRIMARY KEY (signature, txn_index)
);

CREATE INDEX IF NOT EXISTS takes_market_idx ON takes (market, slot);

-- Per-market rollup: last price + raw and self-trade-adjusted volume. The
-- prototype populates the raw figures; the self-trade-adjusted columns wait
-- on the off-chain wash-clustering pipeline (interface.md §1, volume
-- integrity — never silently net).
CREATE TABLE IF NOT EXISTS market_stats (
    market               TEXT   PRIMARY KEY,
    last_price           DOUBLE PRECISION,
    last_slot            BIGINT NOT NULL DEFAULT 0,
    take_count           BIGINT NOT NULL DEFAULT 0,
    volume_base          NUMERIC NOT NULL DEFAULT 0,
    volume_quote         NUMERIC NOT NULL DEFAULT 0,
    volume_base_adjusted  NUMERIC,
    volume_quote_adjusted NUMERIC
);

-- Singleton watermark: the last event coordinate the aggregator folded.
CREATE TABLE IF NOT EXISTS indexer_cursor (
    id                 SMALLINT PRIMARY KEY DEFAULT 1,
    last_slot          BIGINT NOT NULL DEFAULT 0,
    last_txn_index     BIGINT NOT NULL DEFAULT 0,
    last_event_ordinal BIGINT NOT NULL DEFAULT 0,
    CONSTRAINT indexer_cursor_singleton CHECK (id = 1)
);

INSERT INTO indexer_cursor (id) VALUES (1) ON CONFLICT (id) DO NOTHING;
