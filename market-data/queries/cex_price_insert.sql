-- Persist one CEX candle, idempotently. A re-fetched backfill window (the
-- store sink's at-least-once delivery, docs/data-feeds.md §3) hits the PK and
-- is dropped, so `rows_affected` counts only genuinely new buckets.
INSERT INTO cex_prices (
    source, product_id, granularity_secs, bucket_start,
    low, high, open, close, volume
)
VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
ON CONFLICT (source, product_id, granularity_secs, bucket_start) DO NOTHING;
