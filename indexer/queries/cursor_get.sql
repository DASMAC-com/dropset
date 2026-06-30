-- Read the singleton aggregator watermark (the last event coordinate folded).
SELECT
    last_slot,
    last_txn_index,
    last_event_ordinal,
    last_signature
FROM indexer_cursor
WHERE id = 1;
