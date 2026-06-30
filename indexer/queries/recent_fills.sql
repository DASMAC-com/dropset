-- /v1 read: the most recent fill legs, optionally filtered to one market.
SELECT *
FROM fill_events
WHERE ($1::text IS NULL OR market = $1)
ORDER BY slot DESC, txn_index DESC, event_ordinal DESC
LIMIT $2;
