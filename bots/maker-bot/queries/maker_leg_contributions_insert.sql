-- One source's share of one leg's fused estimate for one tick.
--
-- Written for every source whose reading could be **measured** — which is every
-- source that answered a fused leg with a usable value, including the ones the
-- trim then excluded. Those land at `weight = 0` carrying the reading they
-- printed, which is the disagreement an operator needs to see. See the
-- migration for why filtering `weight > 0` discards the point of the table.
--
-- A source whose reading is non-finite or non-positive gets **no row**: no
-- variance can be established for it, so there is no weight to record. Its
-- absence shows up in `feed_health`, not here.
--
-- Idempotent on `(market, leg, source, mechanism, ts)` — the table's full
-- primary key, matching the store sink's at-least-once contract exactly as the
-- sibling inserts do. `mechanism` is named here rather than elided because it is
-- part of the conflict arbiter: the day a second mechanism writes, a key stated
-- without it would describe the wrong uniqueness.
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
