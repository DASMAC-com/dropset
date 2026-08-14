-- Deviation from the FX anchor, in basis points: the basis made visible.
--
-- For one on-chain / CEX venue product (say AUDD-USDC) and the FX rate it is
-- supposed to track (AUD-USD), this is the series
--
--     deviation_bps = (venue_close / anchor_close - 1) * 10000
--
-- Sign convention: POSITIVE means the venue product is trading RICH to the
-- FX anchor (a buyer of AUDD on the venue pays more USD-equivalent than the
-- interbank rate), NEGATIVE means CHEAP. That is the direction a maker cares
-- about, so it is the direction the panel plots.
--
-- Both legs are quoted the same way round, which is what makes the ratio
-- meaningful: Coinbase's AUDD-USDC is USDC-per-AUDD, and the canonical
-- AUD-USD FX pair is USD-per-AUD. Both are "dollars per Australian unit", so
-- their ratio is dimensionless and the peg deviation falls straight out. If
-- you point this at a pair where that is NOT true (an inverted venue symbol,
-- or an FX source that quotes USD/AUD rather than AUD/USD), the series will
-- be wrong by roughly a factor of the rate squared rather than obviously
-- broken -- so check the quote direction before trusting a reading.
--
-- The two legs are aligned onto a common `align_secs` grid rather than joined
-- directly on `bucket_start`. That is deliberate: the collectors do NOT all
-- run at one granularity (the FX free tiers are call-budget capped and will
-- likely run coarser than Coinbase's 60s bars), so a naive equi-join on
-- bucket_start silently returns ZERO rows the moment the two sides disagree.
-- Aligning takes the last close in each window on each side independently,
-- which degrades gracefully instead. Set `align_secs` to the COARSER of the
-- two feeds' granularities.
--
-- Parameters (psql):
--   venue_source, venue_product   -- e.g. 'coinbase', 'AUDD-USDC'
--   anchor_source, anchor_product -- e.g. 'oanda', 'AUD-USD'
--   align_secs                    -- common grid, >= max(granularity) of both
--
-- Example:
--   psql "$DROPSET_DB_URL" \
--     -v venue_source=coinbase -v venue_product=AUDD-USDC \
--     -v anchor_source=oanda   -v anchor_product=AUD-USD \
--     -v align_secs=300 \
--     -f market-data/analytics/deviation_from_anchor.sql

WITH venue AS (
    SELECT DISTINCT ON (bucket_start / :align_secs)
           (bucket_start / :align_secs) * :align_secs AS aligned_start,
           close                                      AS venue_close
    FROM cex_prices
    WHERE source = :'venue_source'
      AND product_id = :'venue_product'
    ORDER BY bucket_start / :align_secs, bucket_start DESC
),
anchor AS (
    SELECT DISTINCT ON (bucket_start / :align_secs)
           (bucket_start / :align_secs) * :align_secs AS aligned_start,
           close                                      AS anchor_close
    FROM cex_prices
    WHERE source = :'anchor_source'
      AND product_id = :'anchor_product'
    ORDER BY bucket_start / :align_secs, bucket_start DESC
)
SELECT
    to_timestamp(v.aligned_start)                             AS bucket_ts,
    v.venue_close,
    a.anchor_close,
    (v.venue_close / a.anchor_close - 1) * 10000              AS deviation_bps
FROM venue v
JOIN anchor a USING (aligned_start)
-- A zero or negative anchor is not a real rate; it would also make the ratio
-- explode or flip sign. Drop those rows rather than plot a spike.
WHERE a.anchor_close > 0
ORDER BY v.aligned_start;
