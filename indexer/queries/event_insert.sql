-- Idempotent insert of a non-fill event into the JSONB fidelity tier, keyed
-- and queryable by kind / market. The event PK dedups via ON CONFLICT.
INSERT INTO events (
    slot,
    txn_index,
    signature,
    event_ordinal,
    block_time,
    kind,
    market,
    payload
)
VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
ON CONFLICT DO NOTHING;
