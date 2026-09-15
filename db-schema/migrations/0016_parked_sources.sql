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
    -- `YYYY-MM-DD` string is therefore also the validation: a malformed date in
    -- a park entry fails the mirror write, which fails collector startup at the
    -- next bring-up, loudly and naming the value. That is the same
    -- fatal-at-startup class as instrument registration and is the intended
    -- direction — a park entry nobody can parse should not reach a dashboard as
    -- a silently wrong date.
    since       DATE   NOT NULL,
    -- Why it is parked and what would un-park it, verbatim from the constant.
    -- Mirrored rather than summarized so the operator-visible copy and the
    -- reader-visible copy cannot drift.
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
