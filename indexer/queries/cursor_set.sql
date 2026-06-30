-- Advance the singleton aggregator watermark.
UPDATE indexer_cursor
SET
    last_slot = $1,
    last_txn_index = $2,
    last_event_ordinal = $3,
    last_signature = $4
WHERE id = 1;
