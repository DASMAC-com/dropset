-- The enabled Pyth FX roster, read once at collector startup. Ordered so the
-- startup log that names the loaded roster is stable between restarts and two
-- runs can be diffed.
SELECT currency, product_id, feed_id, invert
FROM pyth_fx_feeds
WHERE enabled
ORDER BY currency;
