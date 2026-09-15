-- The newest spot print for each (venue, pair) asked for — the `spot_ticks`
-- counterpart to `fx_store_latest.sql`.
--
-- **Why a second statement rather than a widened first one.** The two tables
-- answer the same question about different row shapes, and neither shape can be
-- expressed as the other without lying. A candle's publication instant is
-- `bucket_start + granularity_secs`, derived; a tick's is `observed_at`,
-- recorded. Synthesizing a bucket from a tick is what migration 0004 declined
-- when it created this table, for reasons that are now immutable: it would put a
-- fabricated width in the key and make four copies of one number. A UNION here
-- would have to pick one vocabulary and mislabel the other side in it.
--
-- What is shared instead is the **age convention**, not the SQL: both readers
-- hand their rows to `fx_store::store_reading`, so publication-versus-receipt
-- ageing and the forward-skew refusal are stated exactly once.
--
-- **`confidence` is deliberately not projected.** Pyth is the only venue that
-- publishes a half-width, it is parked dark under the MVP posture, and the
-- fresh-but-uncertain regime it would feed is out of scope — so reading the
-- column here would wire a pathway nothing consumes and invite a caller to
-- treat a NULL as a zero half-width, which reads as perfect certainty. The
-- column is unread by decision, not by oversight, and a unit test in
-- `tick_store.rs` asserts that decision so it cannot lapse silently.
--
-- `DISTINCT ON` over the primary key's leading columns, so this walks per series
-- and stops at the first row: one row per series. The PK is
-- `(source, product_id, observed_at)`, so `observed_at` is unique within a
-- series and there is no tie for `DISTINCT ON` to break arbitrarily.
--
-- Unlike the candle reader there is no bucket-width hazard to order around — no
-- column here can carry a second time base, so `observed_at DESC` is both the
-- newest row and the newest print.
--
-- **Known limitation, shared with the candle reader: "newest" is the newest
-- STAMP, not the newest plausible one.** A row stamped implausibly far ahead
-- wins this ordering, and the consumer then refuses it as forward-skewed —
-- leaving no way to reach the honest row beneath it, so that series reads as
-- absent until wall-clock time catches up. `observed_at` has no upper-bound
-- CHECK, and because it is part of the PK a collector re-poll cannot overwrite
-- the bad row: a corrected stamp inserts a second row and the bad one survives
-- until someone deletes it by hand.
--
-- How exposed this actually is, today: `observed_at` is a venue-supplied instant
-- only where the venue publishes one, and the sole `spot_ticks` venue that does
-- is Pyth — parked dark. Every live writer stamps its own poll second, so
-- reaching this state on the peg leg needs the COLLECTOR's clock to be skewed
-- rather than anything venue- or attacker-supplied. The candle reader, which
-- reads three venues' own timestamps, is the more exposed of the two.
--
-- Bounding it here would fix it: `AND observed_at <= $3`, bound to
-- `now + MAX_PUBLICATION_SKEW` rather than to `now` — binding it to `now` would
-- filter out the 0-120s overshoot `fx_store::store_reading` deliberately
-- accepts, silently tightening the shared tolerance, which is the opposite of
-- the point. Deliberately not done in this change: the candle reader has the
-- identical exposure, and the two should not diverge on the convention they
-- exist to share.
SELECT DISTINCT ON (source, product_id)
    source,
    product_id,
    observed_at,
    price
FROM spot_ticks
WHERE source = ANY($1)
  AND product_id = ANY($2)
ORDER BY source, product_id, observed_at DESC
