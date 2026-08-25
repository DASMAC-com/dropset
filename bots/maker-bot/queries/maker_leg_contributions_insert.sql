-- One source's share of one leg's fused estimate for one tick.
--
-- Written for **every** source that answered a fused leg, including the ones
-- the trim excluded — those land at `weight = 0` carrying the reading they
-- printed, which is the disagreement an operator needs to see. See the
-- migration for why filtering `weight > 0` discards the point of the table.
--
-- Idempotent on `(market, leg, source, ts)`, matching the store sink's
-- at-least-once contract exactly as the sibling inserts do.
INSERT INTO maker_leg_contributions (
    ts,
    market,
    leg,
    source,
    mechanism,
    value,
    variance,
    weight
)
VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
ON CONFLICT DO NOTHING;
