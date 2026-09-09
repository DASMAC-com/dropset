-- The newest closed bucket for each (venue, pair) the maker prices off — the
-- intraday FX anchor, read from the shared market-data store rather than
-- polled from the venues directly.
--
-- Why the maker reads this rather than holding its own OANDA / Twelve Data
-- clients: those venues are metered and keyed, the collectors already poll
-- them on a budget sized to the free tier, and a second consumer on the same
-- key is a self-inflicted rate-limit on the anchor. Reading the collectors'
-- rows also means the maker and the Grafana dashboards price off the exact
-- same numbers, so a green dashboard is evidence about the maker's own inputs.
--
-- `bucket_start + granularity_secs` is the bucket **close**, and that is the
-- honest publication instant of `close`: the row's key is the bucket open, but
-- the closing price is not a statement about that moment, it is a statement
-- about the end of the bucket. The consumer still floors this age on when it
-- read the row, so a collector that stalls ages its own leg out rather than
-- resting on a stamp that stopped moving.
--
-- Only closed buckets are ever written, so no `bucket_start < now()` guard is
-- needed here — see the collectors' `closed_boundary`.
--
-- `DISTINCT ON` over the primary key's leading columns, so this walks the
-- implicit index backwards per series and stops at the first row: no scan, no
-- secondary index, one row per series. Granularity is deliberately not
-- constrained — a venue whose bucket width is reconfigured should still yield
-- its newest print rather than nothing at all.
SELECT DISTINCT ON (source, product_id)
    source,
    product_id,
    bucket_start + granularity_secs AS published_at,
    close
FROM cex_prices
WHERE source = ANY($1)
  AND product_id = ANY($2)
ORDER BY source, product_id, bucket_start DESC
