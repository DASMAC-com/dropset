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
-- column is unread by decision, not by oversight.
--
-- `DISTINCT ON` over the primary key's leading columns, so this walks per series
-- and stops at the first row: one row per series.
--
-- Unlike the candle reader there is no bucket-width hazard to order around — a
-- tick's stamp *is* its publication instant, so `observed_at DESC` is both the
-- newest row and the newest print. The consumer still refuses a
-- future-stamped one and floors its age on the read.
SELECT DISTINCT ON (source, product_id)
    source,
    product_id,
    observed_at,
    price
FROM spot_ticks
WHERE source = ANY($1)
  AND product_id = ANY($2)
ORDER BY source, product_id, observed_at DESC
