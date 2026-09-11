-- Publish one tick of the fair-price estimator's output.
--
-- Idempotent on the primary key, like every other writer here: the estimator is
-- restartable and a replayed tick must not fail the write that reports it. That
-- alone settles `DO NOTHING` against letting the conflict raise.
--
-- **The signal for a conflict is `publish`'s return value, not the shape of the
-- series.** An earlier version of this comment argued that `DO NOTHING` leaves
-- two estimators on one pair "visible as a flat series", and that does not hold:
-- two deterministic estimators write agreeing rows and nothing looks flat; two
-- disagreeing ones have the second value silently discarded, which is the case
-- most needing to be visible; and two independently-clocked processes need not
-- collide on a whole-second `ts` at all, in which case both rows land and the
-- series doubles rather than flattens. A flat series also alerts nobody.
--
-- So the detection contract is the `rows_affected() = 0` that `publish` returns
-- as `false`. A caller must log or count it.
INSERT INTO fair_price (
    ts, product_id, fair, anchor, regime, degrade, health,
    basis, basis_age_secs, basis_outlier, uncertain, basis_breach, usdc_breach
)
VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)
ON CONFLICT (product_id, ts) DO NOTHING;
