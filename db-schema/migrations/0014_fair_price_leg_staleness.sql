-- Record the staleness bounds each published composition resolved at, closing
-- the gap 0011 named in its own header: it carried the composition and its
-- guard flags, noted that per-leg staleness was recorded nowhere at all for
-- exactly the ticks it exists to capture, and left the plan to the mutable
-- docs (docs/market-making.md §1 fair-price estimation).
--
-- **These are BOUNDS, not observations.** Each is the configured age past which
-- a source of that class stopped being credited on this tick — the input to the
-- resolution, not a measurement of how old anything actually was. A reader
-- asking "how stale was this row's FX leg" is not answered here; a reader
-- asking "at what bound was it judged" is.
--
-- **Keyed by source CLASS, not by leg**, which is why the column names say
-- `tape` and `reference` rather than `fx` and `crypto`. The split cannot be per
-- leg: a daily reference fix and a live tape sit on the same FX leg, so the leg
-- does not identify the freshness convention while the class does. Both legs
-- are judged against both bounds, according to what each of their sources is.
--
-- **Why store a value that is configuration.** It is constant across a run, so
-- every row in a run repeats it — the same objection that could be raised
-- against 0011 storing `health`, which is a total function of `regime` there
-- and stored anyway. The answer is the same one: a consumer should not have to
-- reimplement or guess the estimator's calibration to read its output, and a
-- stored copy makes a recalibration visible in the data rather than silent. A
-- series whose bounds change mid-history is the case this exists to show; with
-- the bounds unrecorded, a composition that changed because the bounds moved is
-- indistinguishable from one that changed because the market did.
--
-- **NOT NULL with no DEFAULT, deliberately.** That succeeds only against an
-- empty table, which `fair_price` is: 0011 landed the contract with no process
-- writing to it, and none writes to it yet — the estimator that will write these
-- columns does not exist as of this migration. If a row somehow exists, this
-- fails loudly rather than back-filling a bound the composition never used — a
-- fabricated calibration figure is worse than a failed migration, because it
-- would be indistinguishable from a recorded one forever after.
--
-- **Disclosure.** 0002 grants `dropset_ro` SELECT on every table in `public`,
-- so a dashboard reader sees these, and 0011 requires that any column added
-- here extend its disclosure argument rather than inherit it. Doing that
-- explicitly: these two are **not** covered by 0011's precedent ground, since
-- 0003 exposes no staleness bound on `maker_telemetry`. They rest on a
-- different and narrower ground — a bound is a static configured constant that
-- says nothing about market state. Where `uncertain` tells that reader where a
-- band sits relative to quotes they also hold, a bound tells them only the age
-- at which the estimator stops trusting a class of source. Note this is NOT
-- the rejected "it is deterministic from public data" argument, which 0011
-- correctly refused: the ground here is that the value is not an observation of
-- the market at all, in any regime, on any row.
--
-- **And the decisive point, which is stronger than the one above.** The same
-- grant already exposes `fair_price.fair` itself — the estimator's published
-- output on every tick — beside `regime`, `degrade`, `health` and `uncertain`. A
-- reader holding the *result* of the trust judgment on every row cannot be
-- materially advantaged by also holding the *threshold* that produced it; for
-- the tape class the threshold is in any case recoverable to within one tick
-- from the disclosed series, by watching `basis_age_secs` at the row where
-- `health` transitions. So these columns reduce the effort of a derivation the
-- existing disclosure already permits, which is exactly the shape 0011's
-- precedent covers.
--
-- The strongest objection, recorded because it was raised and judged rather than
-- missed: `leg_stale_reference_secs` is the one figure the observed series may
-- never reveal, since a daily fix rarely ages out during a run — so it names the
-- minimum feed-outage duration needed to force that class out of the roster,
-- which is actionable against a degraded-mode spread. It fails against the point
-- above a fortiori: a party who can SELECT this table can read the published
-- fair price directly, which dominates any forcing threshold.
--
-- One honest limit on all of the above: a bound stated beside the *observed*
-- age 0011 already discloses (`basis_age_secs`) yields remaining headroom, a
-- derivation neither column gives alone. That is a real increment, and it is
-- covered by the dominance argument rather than denied by it.
ALTER TABLE fair_price
    -- Bound for a continuously-publishing source, whose age is a statement
    -- about now. Whole seconds, matching every other duration column here.
    --
    -- The writer TRUNCATES to get here, and that is part of the contract rather
    -- than an accident: a sub-second bound serializes to 0 and is then refused
    -- by the CHECK below, so it surfaces as a failed publish rather than as a
    -- row claiming a bound of zero. Whole seconds is the resolution these
    -- bounds are calibrated in, so nothing legitimate is lost.
    ADD COLUMN leg_stale_tape_secs      BIGINT NOT NULL,
    -- Bound for a source published on a slow schedule, authoritative for the
    -- moment it names. Must exceed the longest gap between publications,
    -- holidays included, or that class drops out of the roster on a closure
    -- nobody is watching — which is why it is recorded separately and not
    -- assumed equal to the tape bound.
    ADD COLUMN leg_stale_reference_secs BIGINT NOT NULL,
    -- Stated in this table's own terms, so nothing here depends on code that
    -- can change after this file becomes immutable:
    --
    --   * a ZERO bound would age every source of that class out instantly, so a
    --     row carrying one describes a composition with no live inputs at all;
    --   * an INVERTED pair would let a daily reference fix age out faster than a
    --     live tape, which is the inversion the source-class split exists to
    --     remove;
    --   * EQUAL bounds are degenerate rather than wrong, so `>=` admits them.
    --
    -- The engine refuses the same three shapes at startup today. That is a
    -- pointer, not an assertion: this CHECK is the database's own floor and does
    -- not depend on that staying true. Deliberately no claim that the two
    -- *mirror* each other — the equivalence is the part that would rot here,
    -- and it is already false in one direction (see the seconds note above:
    -- truncation makes this CHECK stricter on a sub-second bound).
    -- Named for what it enforces rather than for one half of it: a violation's
    -- constraint name is the whole diagnostic it carries, so `..._ordered` would
    -- name the wrong thing when the zero case is what failed.
    ADD CONSTRAINT fair_price_leg_stale_bounds_valid
        CHECK (leg_stale_tape_secs > 0
           AND leg_stale_reference_secs >= leg_stale_tape_secs);

COMMENT ON COLUMN fair_price.leg_stale_tape_secs IS
    'Configured age bound for a tape-class source on this tick, in seconds. '
    'A bound the composition was judged at, not an observed age.';

COMMENT ON COLUMN fair_price.leg_stale_reference_secs IS
    'Configured age bound for a reference-class source on this tick, in '
    'seconds. A bound the composition was judged at, not an observed age.';
