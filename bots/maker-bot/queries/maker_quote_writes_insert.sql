-- One confirmed quote write and its fee. See `0020_maker_quote_writes` for
-- what the two fee columns mean and the rollup views built on them.
--
-- Idempotent on `signature`, the primary key: a transaction signature is
-- unique on chain, so a conflict can only be this same write delivered twice,
-- and dropping the duplicate is exactly right.
INSERT INTO maker_quote_writes (
    ts,
    market,
    kind,
    signature,
    base_fee_lamports,
    priority_fee_lamports
)
VALUES ($1, $2, $3, $4, $5, $6)
ON CONFLICT DO NOTHING;
