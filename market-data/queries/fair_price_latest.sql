-- The newest published composition per requested product.
--
-- `DISTINCT ON` over the primary key's own order, so this is one index descent
-- per product rather than a scan of the series. A product with no row at all is
-- simply absent from the result; the consumer tells that apart from a failed
-- read by the read succeeding.
--
-- `$2` is a stamp ceiling: rows stamped past it are skipped, so one
-- future-stamped row cannot shadow the honest rows written after it. See
-- `FairPriceSource::latest_at`.
SELECT DISTINCT ON (product_id)
    ts, product_id, fair, anchor, regime, degrade, health,
    basis, basis_age_secs, basis_outlier, uncertain, basis_breach, usdc_breach,
    fx_tape_live
FROM fair_price
WHERE product_id = ANY($1) AND ts <= $2
ORDER BY product_id, ts DESC;
