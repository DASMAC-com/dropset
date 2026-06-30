-- All legs of one take (the (signature, txn_index) group), for a full
-- idempotent recompute.
SELECT *
FROM fill_events
WHERE signature = $1 AND txn_index = $2
ORDER BY event_ordinal;
