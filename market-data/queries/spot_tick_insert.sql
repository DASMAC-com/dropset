-- Persist one spot tick, idempotently. A venue-timestamped print re-fetched
-- after a restart (the store sink's at-least-once delivery,
-- docs/data-feeds.md §3) carries the same `observed_at` and is dropped here, so
-- `rows_affected` counts only genuinely new observations.
INSERT INTO spot_ticks (source, product_id, observed_at, price, confidence)
VALUES ($1, $2, $3, $4, $5)
ON CONFLICT (source, product_id, observed_at) DO NOTHING;
