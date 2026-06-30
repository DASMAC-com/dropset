-- Fill legs strictly after the watermark, in PK order. `signature` joins the
-- compare/order tuple so a strict `>` never skips a leg when two takes share
-- (slot, txn_index, event_ordinal) (the RPC path pins txn_index to 0).
SELECT *
FROM fill_events
WHERE (slot, txn_index, event_ordinal, signature) > ($1, $2, $3, $4)
ORDER BY slot, txn_index, event_ordinal, signature
LIMIT $5;
