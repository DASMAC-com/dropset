-- Replace the `feed_health` names each park silences. Driven by
-- `market-data/src/parked_mirror.rs`, and run after
-- `parked_sources_mirror.sql` in the same transaction — the parent rows have to
-- exist before these reference them.
--
-- The prune here is narrower than the parent one and has to be: a venue that
-- stays parked while its health-feed list changes leaves the parent row in
-- place, so nothing cascades and the stale pair would survive. Deleting every
-- pair not in the incoming set covers both that and a venue dropping one name
-- of several.
--
-- A venue that leaves `PARKED_SOURCES` entirely is already handled by the
-- parent statement's ON DELETE CASCADE, so this prune is not the only thing
-- standing between an un-parked venue and a stale exclusion.
--
-- $1 — the venue token of each pair (TEXT[])
-- $2 — the `feed_health.feed` name of each pair (TEXT[])
--
-- The arrays are parallel: one element per (venue, feed) pair, flattened from
-- the nested constant by the Rust side, so a venue naming two feeds appears
-- twice in $1.
WITH incoming AS (
    SELECT
        venue,
        feed
    FROM unnest($1::TEXT[], $2::TEXT[]) AS t (venue, feed)
),

pruned AS (
    DELETE FROM parked_source_feeds AS p
    WHERE NOT EXISTS (
        SELECT 1
        FROM incoming AS i
        WHERE i.venue = p.venue AND i.feed = p.feed
    )
)

INSERT INTO parked_source_feeds (venue, feed)
SELECT
    i.venue,
    i.feed
FROM incoming AS i
ON CONFLICT (venue, feed) DO NOTHING;
