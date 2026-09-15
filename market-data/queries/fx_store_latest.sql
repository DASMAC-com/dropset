-- The newest closed bucket for each (venue, pair) asked for — read from the
-- shared market-data store rather than polled from the venues directly.
--
-- The venue and pair lists are both the caller's, so this is not FX-specific
-- despite the file name: the maker reads its intraday FX anchor through it, and
-- the fair-value estimator reads the crypto reference the same way. Nothing here
-- constrains which sources may be asked for.
--
-- **But it does constrain which TABLE**, and that bounds the sentence above.
-- This statement reads `cex_prices` only, and the USDC/USD peg series is written
-- to `spot_ticks` by `market-data-kraken` and to `cex_prices` by nobody — so the
-- peg leg is NOT reachable through this reader, however unconstrained the source
-- list is. `queries/spot_ticks_latest.sql` is the counterpart that reaches it.
-- (An earlier version of this comment claimed the peg was readable here. It was
-- not; the corrected claim lives here rather than only in the other file, so a
-- reader of this one is not misled.)
--
-- Why a consumer reads this rather than holding its own OANDA / Twelve Data
-- clients: those venues are metered and keyed, the collectors already poll
-- them on a budget sized to the free tier, and a second consumer on the same
-- key is a self-inflicted rate-limit on the anchor. Reading the collectors'
-- rows also means the consumers and the Grafana dashboards price off the exact
-- same numbers, so a green dashboard is evidence about a consumer's own inputs.
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
-- `DISTINCT ON` over the primary key's leading columns, so this walks per
-- series and stops at the first row: one row per series. Granularity is
-- deliberately not constrained — a venue whose bucket width is reconfigured
-- should still yield its newest print rather than nothing at all.
--
-- **Order by the projected close, not by `bucket_start`.** Those differ
-- exactly when one source holds two granularities, which the schema permits
-- and the previous sentence invites: a daily bucket opened today outranks a
-- minute bucket opened an hour ago on `bucket_start`, while its close is
-- twenty-three hours in the future. Picking it would hand the maker a
-- future-stamped row as its freshest print — and a future stamp is the one
-- input the consumer's age arithmetic cannot make safe, since the receipt
-- floor resets on every poll. Ordering by the value actually projected keeps
-- the winner the row whose close is genuinely newest, whatever bucket widths
-- coexist.
SELECT DISTINCT ON (source, product_id)
    source,
    product_id,
    bucket_start + granularity_secs AS published_at,
    close
FROM cex_prices
WHERE source = ANY($1)
  AND product_id = ANY($2)
ORDER BY source, product_id, bucket_start + granularity_secs DESC
