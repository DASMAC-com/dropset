-- Replace the parked-source parent rows with this build's whole set. Driven by
-- `market-data/src/parked_mirror.rs`, which runs it inside a transaction with
-- `parked_source_feeds_mirror.sql`.
--
-- REPLACE, not upsert-only: the prune is what makes un-parking work. Removing an
-- entry from `PARKED_SOURCES` has to delete its row, or the mirror would
-- accumulate every source ever parked and a panel would keep rendering a
-- restored source as quiet-by-decision. The prune also cascades to
-- `parked_source_feeds`, so a whole venue leaves in one statement.
--
-- The prune and the insert are one statement so no reader ever sees the set
-- empty. `<> ALL` over the incoming array is deliberately the whole test: it
-- deletes exactly the rows this build does not declare, and when the array is
-- empty — every source un-parked — `<> ALL('{}')` is TRUE for every row, so the
-- mirror correctly empties. That case is why the Rust side does not skip the
-- write on an empty set, and it is the opposite choice from
-- `instrument_register.sql`, whose empty-roster guard returns early.
--
-- The two CTEs cannot conflict: a venue is either in the incoming set or not, so
-- the rows the prune deletes and the rows the insert touches are disjoint by
-- construction, and both see the same snapshot.
--
-- $1 — the bare venue tokens (TEXT[])
-- $2 — the park dates, `YYYY-MM-DD` (TEXT[], cast to DATE here)
-- $3 — the reasons (TEXT[])
-- $4 — the epoch second of this mirror write (BIGINT)
--
-- The three arrays are parallel and are zipped POSITIONALLY by `unnest`. Do not
-- rely on it to police their lengths — a multi-argument `unnest` does not
-- require them to match. What keeps them aligned is that the Rust side builds
-- all three in one pass over the constant (`flatten_parked_set`, pinned by its
-- own test), and what would catch a divergence here is the `NOT NULL` on
-- `since` and `reason`.
WITH incoming AS (
    SELECT
        venue,
        since,
        reason
    FROM unnest($1::TEXT[], $2::TEXT[], $3::TEXT[])
        AS t (venue, since, reason)
),

pruned AS (
    DELETE FROM parked_sources
    WHERE venue <> ALL ($1::TEXT[])
)

INSERT INTO parked_sources (venue, since, reason, mirrored_at)
SELECT
    i.venue,
    i.since::DATE,
    i.reason,
    $4
FROM incoming AS i
ON CONFLICT (venue) DO UPDATE
SET
    since = EXCLUDED.since,
    reason = EXCLUDED.reason,
    mirrored_at = EXCLUDED.mirrored_at;
