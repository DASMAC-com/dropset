-- Per-batch ingestion counts for every store-sink feed: how many records a
-- batch offered, how many rows it actually wrote, and which of three outcomes
-- that pair represents.
--
-- **The gap this closes.** A collector that stores zero rows reports success
-- exactly like a full one. The store sink commits whatever survived intake and
-- the cursor advances regardless, so a batch that persisted nothing leaves no
-- durable trace — the count existed only as a `tracing::debug!` field. A gap in
-- a candle series also reads as legitimate on its own, because venues omit
-- buckets on quiet stretches, so a consumer cannot tell attrition from a still
-- market by looking at the series.
--
-- **Why now rather than earlier.** The candle intake guard drops a bad bar with
-- a warning instead of failing the batch — the right trade, and what let
-- `0012_candle_price_checks` be added VALIDATED — but it converts a loud stop
-- into a quiet gap by construction. The conversion was deliberate; the quiet gap
-- had no detector.
--
-- **What this DOES and DOES NOT see, stated precisely, because the obvious
-- reading is too generous.** `requested` is the count of records that reached
-- the sink, so it is measured *after* intake has already dropped whatever it
-- rejected. That means a batch which arrives EMPTY is visible here, and a batch
-- which merely arrives SHORT is not: a guard that drops some bars and keeps
-- others yields `requested = written` and an outcome of `stored`, identical to a
-- healthy batch. Partial attrition therefore has no detector either, and this
-- table does not claim one — the per-bar warning remains its only trace. What is
-- closed is the total case, which is the filed symptom: a collector that keeps
-- polling and stores nothing.
--
-- **Three outcomes, and the middle one is why this table is not just a row
-- count.** Delivery is deliberately at-least-once: the cursor is saved after
-- the batch's transaction commits, so a crash in between re-fetches the last
-- window and the writer's idempotent upsert absorbs the duplicates. That means
-- `written = 0` is the EXPECTED state after a crash-restart, not a fault, and a
-- naive "zero rows written" alarm would fire on every legitimate redelivery.
-- The discriminator is whether anything reached the writer at all:
--
--   * `empty_intake`  — nothing was offered. Either the venue returned an empty
--                       response or intake rejected every record.
--   * `all_duplicate` — records were offered and every one deduped away. The
--                       ordinary shape of a resumed window; routine.
--   * `stored`        — at least one row was written.
--
-- **A single `empty_intake` is NOT a fault, and reading it as one would make
-- this table worse than useless.** A polling collector legitimately returns an
-- empty batch whenever the venue has not published the next bucket yet, so on a
-- short poll interval most batches are empty in normal operation. What the
-- filed symptom actually describes is a *sustained run* of them — a feed that
-- was storing rows and now stores none while its market is open. That is a
-- question about a rate over a window, which is why this table records every
-- batch and leaves the judgement to the panel, rather than trying to classify a
-- single row as healthy or not.
--
-- **`outcome` is GENERATED rather than written by the caller**, so the
-- discriminator is a property of the schema instead of a convention each writer
-- and each dashboard query re-derives. A stored column cannot drift from the
-- counts it describes, and a future writer cannot mislabel a batch — the same
-- reasoning `0012_candle_price_checks` gives for putting price sanity in a
-- constraint rather than in each adapter.
--
-- **A partial dedup is deliberately NOT its own outcome.** `written < requested`
-- with `written > 0` is the normal result of an overlapping window and says
-- nothing a reader should act on, so it folds into `stored`. The counts are both
-- retained, so a query that wants the ratio still has it.
--
-- **No upper-bound CHECK relating `written` to `requested`.** It is tempting to
-- assert `written <= requested`, and it holds for every writer today, but the
-- `StoreWriter` contract does not promise a one-row-per-record mapping: a
-- consumer may legitimately expand one record into several rows. A CHECK would
-- convert that design freedom into a migration-time failure for a writer that
-- has done nothing wrong.
--
-- That freedom is why `empty_intake` tests BOTH counts rather than `requested`
-- alone. Leaving the CHECK out makes `(requested = 0, written > 0)` schema-legal,
-- and a `requested`-only test would label such a row `empty_intake` — reporting a
-- feed as silent in the very row that proves it wrote. It folds into `stored`
-- instead, which is what actually happened.
--
-- **Keyed `(feed, observed_at)`, and the insert is idempotent.** A single feed's
-- sink is sequential, and `now()` is transaction-START time, so a collision needs
-- one feed to BEGIN two batch transactions inside the same microsecond. A commit
-- and a client round trip separate them by far more than that in practice —
-- though "in practice" is the honest strength of the claim, not "impossible".
-- The writer therefore inserts `ON CONFLICT DO NOTHING`, because the failure mode
-- of being wrong about it must not be an aborted data batch: silently losing one
-- telemetry row is the correct price, per docs/data-feeds.md §8's
-- idempotent-write rule.
--
-- **Grafana reads this; nothing surfaces it in the TUI.** `0002_reader_role`
-- already grants `SELECT` on future tables in `public` to `dropset_ro`, so this
-- needs no accompanying grant.
--
-- Retention is deliberately out of scope here — one row per batch per feed
-- accumulates, and pruning it is its own decision rather than a rider on the
-- detector. Nothing here should be read as having settled it.
CREATE TABLE feed_batch_ingestion (
    feed        TEXT        NOT NULL,
    observed_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    requested   BIGINT      NOT NULL,
    written     BIGINT      NOT NULL,
    outcome     TEXT        GENERATED ALWAYS AS (
        CASE
            WHEN requested = 0 AND written = 0 THEN 'empty_intake'
            WHEN written = 0 THEN 'all_duplicate'
            ELSE 'stored'
        END
    ) STORED,
    PRIMARY KEY (feed, observed_at),
    CONSTRAINT counts_are_non_negative
        CHECK (requested >= 0 AND written >= 0)
);

-- The primary key already serves a per-feed time range. This covers the
-- cross-feed panel query — "which feeds stored nothing recently" — which scans
-- by time first and does not name a feed.
CREATE INDEX feed_batch_ingestion_observed_at_idx
    ON feed_batch_ingestion (observed_at DESC);
