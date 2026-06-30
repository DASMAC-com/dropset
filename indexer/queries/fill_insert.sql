-- Idempotent insert of one fill leg into the typed raw tier. u64 atoms are
-- NUMERIC; the event PK dedups a replayed slot via ON CONFLICT DO NOTHING.
INSERT INTO fill_events (
    slot,
    txn_index,
    signature,
    event_ordinal,
    block_time,
    market,
    taker,
    leader,
    quote_authority,
    side,
    sector_idx,
    level_idx,
    fill_base,
    fill_quote,
    fill_price,
    base_atoms_after,
    quote_atoms_after,
    nonce_after,
    taker_fee_atoms
)
VALUES (
    $1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15,
    $16, $17, $18, $19
)
ON CONFLICT DO NOTHING;
