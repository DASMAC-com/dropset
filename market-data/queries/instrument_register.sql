-- Register every product in this collector's roster in the instruments
-- dimension. Driven by `market-data/src/instruments.rs`.
--
-- One statement for the whole roster rather than one per product: the roster is
-- handed over as a TEXT[] and unnested, so a collector polling twenty crosses
-- pays one round trip at startup instead of twenty.
--
-- `first_registered_at` is written only by the insert, never by the update, so
-- it keeps the first time this product was ever seen across every later
-- restart. `last_registered_at` moves every time, which is what makes a row
-- left behind by a pair that was dropped from a roster distinguishable from one
-- a collector is still confirming.
--
-- $1 — the canonical product ids (TEXT[])
-- $2 — the epoch second of this registration (BIGINT)
INSERT INTO instrument_registry (product_id, first_registered_at, last_registered_at)
SELECT unnest($1::TEXT[]), $2, $2
ON CONFLICT (product_id) DO UPDATE
    SET last_registered_at = EXCLUDED.last_registered_at;
