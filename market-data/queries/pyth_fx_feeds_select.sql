-- The enabled Pyth FX roster, read once at collector startup. Ordered so the
-- startup log that names the loaded roster is stable between restarts and two
-- runs can be diffed.
--
-- Ordered by `product_id` rather than `currency`: since `0006_pyth_fx_crosses`
-- the roster carries non-USD crosses, so one currency names several rows and
-- ordering by it is no longer deterministic.
SELECT currency, product_id, feed_id, invert
FROM pyth_fx_feeds
WHERE enabled
ORDER BY product_id;
