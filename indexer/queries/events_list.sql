-- /v1 read: the most recent fidelity-tier events, optionally filtered by
-- kind and / or market.
SELECT
    slot,
    txn_index,
    signature,
    event_ordinal,
    block_time,
    kind,
    market,
    payload
FROM events
WHERE
    ($1::text IS NULL OR kind = $1)
    AND ($2::text IS NULL OR market = $2)
ORDER BY slot DESC, txn_index DESC, event_ordinal DESC
LIMIT $3;
