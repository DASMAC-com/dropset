-- The parked-source set, mirrored from code so a panel can join against it.
--
-- A source parked **by decision** is not a fault, and separating the two is
-- what `docs/dashboards.md` §4 requires of every liveness surface. The decision
-- itself lives in `PARKED_SOURCES` (`feeds/src/parked.rs`) and cannot live
-- here: parking is a statement about deployment, made about a collector that is
-- deliberately not running, and every table in this schema that could hold it
-- is written by a *running* collector. `instrument_registry` is the specific
-- one — a parked source can never write the row that would say it is parked.
--
-- **These tables are a MIRROR, and nothing else may treat them as an
-- authority.** The market-data collectors replace the whole set at startup
-- (`market-data/src/parked_mirror.rs`), so the rows say what the last-started
-- collector's build believed. Nothing reads them to decide whether to spawn a
-- tier; the constant is what gates that, and a reader here who wants "is it
-- running" wants `instrument_source_liveness` instead.
--
-- WHY NOT SEED THE LIST HERE, which is the obvious shape and was rejected on
-- the record. A seed would pin the parked set to migration time: every later
-- parking decision would cost another migration, and between them the code
-- constant and this table could disagree with nothing detecting it. The runner
-- checksums this file's raw bytes once applied, so the seeded list could never
-- be corrected either — only superseded. A mirror written at bring-up keeps one
-- source of truth and makes parking and un-parking cost no migration at all.
--
-- The cost paid instead, stated so no consumer is surprised by it: the mirror
-- is only as fresh as the last collector start. A park decided since then is
-- real in code and absent here. `mirrored_at` is what lets a reader see that,
-- and it is why a stale mirror is a legible state rather than a wrong answer.
--
-- **Disclosure.** 0002 grants the read-only `dropset_ro` role SELECT on every
-- table in `public`, present and future, so a dashboard reader sees these. That
-- is the intent rather than a tolerated consequence — the whole purpose is to
-- be read by a panel. Nothing here is venue-produced or credential-bearing:
-- both tables hold compile-time constants from this repository, and `reason` is
-- operator-authored prose already committed in `feeds/src/parked.rs`.
CREATE TABLE parked_sources (
    -- The bare venue token, as `instrument_source_liveness.source` spells it
    -- (`pyth`, not `pyth-hermes`). This is the join key for a coverage panel
    -- driven by the registry, and it is deliberately NOT the key the health
    -- table uses — see `parked_source_feeds` below.
    venue       TEXT   PRIMARY KEY,
    -- The date the source has been parked since: the date the condition began,
    -- not the date the entry was written.
    --
    -- DATE rather than the BIGINT unix seconds this schema uses elsewhere,
    -- because this is a calendar decision date rather than a measured instant —
    -- there is no clock reading here to preserve. The cast from the constant's
    -- `YYYY-MM-DD` string is therefore also a validation: a malformed date in a
    -- park entry fails the mirror write, and the mirror write is fatal at
    -- collector startup, so it cannot reach a dashboard as a silently wrong
    -- date. The first line of defense is in the constant's own test suite
    -- rather than here, precisely because this file cannot be corrected once
    -- applied: `feeds/src/parked.rs` validates every `since` as a real calendar
    -- date -- month lengths and the leap rule, not merely a 1-to-31 range --
    -- which turns a transposed or impossible date into a red build instead of a
    -- bring-up failure.
    since       DATE   NOT NULL,
    -- Why it is parked and what would un-park it, verbatim from the constant.
    -- Mirrored rather than summarized so an ad-hoc query can read the reason
    -- without the repository to hand. Note NO PANEL RENDERS IT: it is prose too
    -- long for a table cell, so this column is for SQL, and the reader-visible
    -- home of a park's reason stays the constant itself.
    reason      TEXT   NOT NULL,
    -- Unix seconds of the mirror write that last produced this row.
    --
    -- Not a park date and not a data-freshness signal: it tracks collector
    -- starts, exactly as `instrument_registry.last_registered_at` does. What it
    -- answers is "how current is this mirror", which is the one question a
    -- consumer of a mirrored set must be able to ask.
    mirrored_at BIGINT NOT NULL
);

-- The `feed_health.feed` names each park silences — the framework vocabulary,
-- not the bare venue token.
--
-- THIS TABLE IS THE VOCABULARY BRIDGE, and it is a table rather than a join
-- condition on purpose. A park is decided against the bare token (`pyth`),
-- while `feed_health` and the staleness alert are keyed by the framework source
-- name (`pyth-hermes`). Relating those two vocabularies in SQL is the silent
-- join the schema catalog forbids — and here it would look especially
-- reasonable, since for this one adapter the strings differ by a suffix. So the
-- mapping is declared in code, where both spellings are known
-- (`ParkedSource::health_feeds`), mirrored into these rows, and every query
-- that uses it does a single-vocabulary equality against `feed_health.feed`.
--
-- A park with NO row here is legitimate and means it silences no health row —
-- correct for a venue whose collector never wrote one. It is not a way to
-- record uncertainty.
--
-- The consumers exclude on MEMBERSHIP here, never on a NULL `last_ok_at`.
-- Never-answered is deliberately a firing state — worse than
-- stopped-answering, not exempt from it — so an exclusion keyed on NULL would
-- blind the staleness alert to every genuinely never-answered feed.
CREATE TABLE parked_source_feeds (
    -- The parked venue this name belongs to. ON DELETE CASCADE is what lets the
    -- mirror write prune a whole un-parked venue by deleting one parent row.
    venue TEXT NOT NULL REFERENCES parked_sources (venue) ON DELETE CASCADE,
    -- The `feed_health.feed` spelling. No foreign key to `feed_health`: the
    -- whole point is to name a feed whose row may be absent — a fresh database
    -- has none for a parked source, since parking removed its only writer.
    feed  TEXT NOT NULL,
    PRIMARY KEY (venue, feed)
);
