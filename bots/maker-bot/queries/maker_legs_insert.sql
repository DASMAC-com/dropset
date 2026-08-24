-- One resolved leg reading for one market for one tick, with the consensus
-- diagnostics that replace per-source attribution (see the migration for why
-- there is no answering-feed column). Idempotent on `(market, leg, ts)` for
-- the same reason as the sample insert.
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
    dispersion_outlier
)
VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
ON CONFLICT DO NOTHING;
