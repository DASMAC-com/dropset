-- The FX session fence: which wall-clock spans the FX market is open for.
--
-- This view is the intended **session authority**: the one place entitled to
-- decide whether the FX market is trading. Note it has no reader yet — see
-- STAGING at the end of this header — so as of this migration the maker still
-- decides from a local wall-clock bracket. The reason such an authority has to
-- exist at all is that no feed can decide it:
-- a venue that self-fences goes quiet at the close, so its silence looks like
-- the market shutting, but a *grid* venue emits a bar for every interval whether
-- or not anyone traded. Measured across a full window, one roster vendor printed
-- 28.6% of its series outside the session with 99% of those bars moving, while
-- the trusted tape printed none at all. So feed liveness carries no information
-- about the session, and the state has to be imposed from a source that knows
-- the calendar.
--
-- WHY A VIEW AND NOT A TABLE. There is a rule here and no reference data: the
-- session boundary is weekly, and holidays do not affect availability. So a table
-- would be a materialization of a rule, and stored rows can drift from the rule
-- they were generated from where a view cannot. That is the whole argument, and
-- it is deliberately NOT an argument about horizons — the view below carries a
-- bounded horizon of its own, for the reason given a few paragraphs down. The
-- difference is only whether the stored form can come to disagree with the rule.
--
-- WHY THE SPANS ARE CONTIGUOUS. Every instant inside the horizon is covered by
-- exactly one row, open or closed, **read as a half-open interval**:
-- `starts_at <= t < ends_at`. That convention is load-bearing rather than
-- incidental, because each boundary instant is the `ends_at` of one row and the
-- `starts_at` of the next — so a consumer writing `BETWEEN starts_at AND ends_at`
-- gets TWO rows at every Friday and Sunday 17:00 ET, one 'open' and one 'closed'.
-- The column names carry no hint of this, so it is stated here and in the view's
-- own COMMENT: read it half-open.
--
-- Given that convention, a consumer distinguishes three states
-- rather than two: a covering row reading 'open', a covering row reading
-- 'closed', and NO covering row. That third case is what lets the fence fail
-- closed — it means the authority cannot answer, and a consumer must halt rather
-- than quote. A table of open windows alone could not express it, because a
-- closed market and an unanswerable question would both be the absence of a row.
--
-- A consequence worth stating plainly: when this horizon runs out, every
-- consumer halts. That is the intended failure and the reason the horizon is
-- generous rather than unbounded. A fence that silently resumed quoting past its
-- own coverage would be worse than one that stops.
--
-- POSTGRES IS THE DAYLIGHT-SAVING AUTHORITY, which is the substantive reason the
-- boundary lives here at all rather than in application code. The anchors below
-- are stated in America/New_York and converted, so the UTC instant moves with US
-- daylight saving automatically. The derivation this is intended to replace
-- hardcodes UTC hours and says so in its own comment; a fixed pair of hours is
-- exact for one half of the year and an hour off for the other.
--
-- THE ANCHORS ARE THE RULE, NOT A MEASUREMENT. Interbank FX is shut Friday 17:00
-- ET through Sunday 17:00 ET, and those two instants are what this view encodes.
-- Observed vendor timestamps sit slightly inside them — the trusted tape's last
-- bar lands a minute before the close and its first bar after the reopen lands a
-- few minutes late — because a tape only prints when someone trades. That jitter
-- is absorbed by the tape staleness bound, which is far wider than it. Do not
-- re-anchor these boundaries on measured tape edges: that would pin a rule to
-- one vendor's observed behavior in one sampled window, and the rule is the
-- thing that is actually true.
--
-- STAGING. This migration lands the authority; the reader that consults it lands
-- separately, so nothing in the running system reads this view yet. The split is
-- deliberate — it keeps the schema change and the behavioral change reviewable
-- apart — and is recorded on the issue rather than only here.
--
-- DISCLOSURE. 0002 grants the read-only `dropset_ro` role SELECT on everything
-- in `public` and sets default privileges for future relations, which in
-- Postgres cover views as well as tables — so this view is readable by whoever
-- holds the dashboard password and needs no grant of its own. It contains no
-- position or credential data, only a calendar.
CREATE VIEW fx_session_window AS
WITH fridays AS (
    -- 2024-01-05 is a Friday, and a 7-day step keeps every generated date one.
    -- The end bound only truncates the series, so it need not fall on a Friday.
    SELECT g::date AS friday
    FROM generate_series(
        DATE '2024-01-05',
        DATE '2036-01-01',
        INTERVAL '7 days'
    ) AS g
),
anchors AS (
    SELECT
        (friday + TIME '17:00') AT TIME ZONE 'America/New_York' AS closes_at,
        (friday + 2 + TIME '17:00')
            AT TIME ZONE 'America/New_York' AS reopens_at
    FROM fridays
),
spans AS (
    SELECT
        closes_at,
        reopens_at,
        LEAD(closes_at) OVER (ORDER BY closes_at) AS next_closes_at
    FROM anchors
)
SELECT
    closes_at AS starts_at,
    reopens_at AS ends_at,
    'closed'::text AS state
FROM spans
UNION ALL
-- The final week's open span is dropped: with no following close there is no
-- instant to end it at, and inventing one would extend the horizon past the
-- point the rule was actually generated for.
SELECT
    reopens_at AS starts_at,
    next_closes_at AS ends_at,
    'open'::text AS state
FROM spans
WHERE next_closes_at IS NOT NULL;

COMMENT ON VIEW fx_session_window IS
    'Contiguous FX open/closed spans, anchored Fri 17:00 ET to Sun 17:00 ET. '
    'Read half-open (starts_at <= t < ends_at): adjacent rows share a boundary '
    'instant, so BETWEEN matches two rows there. No covering row means the '
    'session is unestablished, which fails closed.';
