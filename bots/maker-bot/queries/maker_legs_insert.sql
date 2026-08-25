-- One resolved leg reading for one market for one tick: the fast consensus and
-- its diagnostics, plus the fused estimate the composition actually priced off.
-- Per-source attribution now exists and lives in `maker_leg_contributions`.
-- Idempotent on `(market, leg, ts)` for the same reason as the sample insert.
INSERT INTO maker_legs (
    ts,
    market,
    leg,
    value,
    age_secs,
    confidence,
    fresh,
    consensus_state,
    contributor_count,
    dispersion_outlier,
    fused_value,
    fused_sigma,
    fusion_step,
    fused_count
)
VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14)
ON CONFLICT DO NOTHING;
