-- Publish one tick of the fair-price estimator's output.
--
-- Idempotent on the primary key, like every other writer here: the estimator is
-- restartable and a replayed tick must not fail the write that reports it. A
-- re-published tick is dropped rather than updated — the estimator is
-- deterministic for a given tick stamp, so a conflicting row would mean two
-- estimators on one pair, which `DO UPDATE` would silently paper over and
-- `DO NOTHING` leaves visible as a flat series.
INSERT INTO fair_price (
    ts, product_id, fair, anchor, regime, degrade, health,
    basis, basis_age_secs, basis_outlier, uncertain, basis_breach, usdc_breach
)
VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)
ON CONFLICT (product_id, ts) DO NOTHING;
