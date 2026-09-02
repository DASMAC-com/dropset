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
-- `DISTINCT` is defensive, not decorative: `ON CONFLICT DO UPDATE` raises
-- `cannot affect row a second time` if the array repeats a product id, and
-- registration is fatal by design — so a duplicate would abort collector
-- startup with an opaque Postgres error. No current caller can produce one
-- (`parse_roster` rejects a duplicate canonical id, and the Pyth roster comes
-- from a table keyed on `product_id`), but the Rust side explicitly guards the
-- shape of a future caller "assembling ids some other way", and this is the
-- other half of that guard. It costs the same single round trip.
INSERT INTO instrument_registry
    (source, product_id, first_registered_at, last_registered_at)
SELECT DISTINCT $1, product_id, $3, $3
FROM unnest($2::TEXT[]) AS product_id
ON CONFLICT (source, product_id) DO UPDATE
    SET last_registered_at = EXCLUDED.last_registered_at;
