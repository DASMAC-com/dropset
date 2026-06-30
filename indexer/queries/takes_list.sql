-- /v1 read: the most recent takes, optionally filtered to one market.
SELECT *
FROM takes
WHERE ($1::text IS NULL OR market = $1)
ORDER BY slot DESC, txn_index DESC
LIMIT $2;
