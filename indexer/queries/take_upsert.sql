-- Upsert one take (a (signature, txn_index) group). Recomputed from all of
-- the take's legs each pass, so re-folding is idempotent.
INSERT INTO takes (
    signature,
    txn_index,
    slot,
    block_time,
    market,
    taker,
    side,
    leg_count,
    total_fill_base,
    total_fill_quote,
    total_taker_fee,
    avg_price
)
VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)
ON CONFLICT (signature, txn_index) DO UPDATE SET
    slot = excluded.slot,
    block_time = excluded.block_time,
    leg_count = excluded.leg_count,
    total_fill_base = excluded.total_fill_base,
    total_fill_quote = excluded.total_fill_quote,
    total_taker_fee = excluded.total_taker_fee,
    avg_price = excluded.avg_price;
