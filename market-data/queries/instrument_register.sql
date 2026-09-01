-- Register every product in this collector's roster in the instruments
-- dimension. Driven by `market-data/src/instruments.rs`.
--
-- One statement for the whole roster rather than one per product: the roster is
-- handed over as a TEXT[] and expanded, so a collector polling twenty crosses
-- pays one round trip at startup instead of twenty.
--
-- Keyed on `(source, product_id)` — the source is what makes the liveness
-- lookup a primary key prefix on the measurement tables, and what records
-- which collector covers which product.
--
-- `first_registered_at` is written only by the insert, never by the update, so
-- it keeps the first time this source was ever seen polling this product across
-- every later restart. `last_registered_at` moves every time, which is what
-- makes a row left behind by a pair that was dropped from a roster
-- distinguishable from one a collector is still confirming.
--
-- $1 — the value this collector writes to `cex_prices` / `spot_ticks`.`source`
-- $2 — the canonical product ids (TEXT[])
-- $3 — the epoch second of this registration (BIGINT)
INSERT INTO instrument_registry
    (source, product_id, first_registered_at, last_registered_at)
SELECT $1, unnest($2::TEXT[]), $3, $3
ON CONFLICT (source, product_id) DO UPDATE
    SET last_registered_at = EXCLUDED.last_registered_at;
