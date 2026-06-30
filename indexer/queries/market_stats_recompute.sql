-- Recompute one market's rollup from its takes — idempotent. last_price is
-- the most recent take's avg_price; volumes are raw sums (the self-trade-
-- adjusted columns wait on the off-chain wash-clustering pipeline).
INSERT INTO market_stats (
    market,
    last_price,
    last_slot,
    take_count,
    volume_base,
    volume_quote
)
SELECT
    t.market,
    (
        SELECT tt.avg_price
        FROM takes AS tt
        WHERE tt.market = t.market
        ORDER BY tt.slot DESC, tt.txn_index DESC, tt.signature DESC
        LIMIT 1
    ) AS last_price,
    COALESCE(MAX(t.slot), 0) AS last_slot,
    COUNT(*) AS take_count,
    COALESCE(SUM(t.total_fill_base), 0) AS volume_base,
    COALESCE(SUM(t.total_fill_quote), 0) AS volume_quote
FROM takes AS t
WHERE t.market = $1
GROUP BY t.market
ON CONFLICT (market) DO UPDATE SET
    last_price = excluded.last_price,
    last_slot = excluded.last_slot,
    take_count = excluded.take_count,
    volume_base = excluded.volume_base,
    volume_quote = excluded.volume_quote;
