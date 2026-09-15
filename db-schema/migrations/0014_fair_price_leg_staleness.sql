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
-- writing to it, and the estimator that writes these columns arrives in the
-- same change as this migration. If a row somehow exists, this fails loudly
-- rather than back-filling a bound the composition never used — a fabricated
-- calibration figure is worse than a failed migration, because it would be
-- indistinguishable from a recorded one forever after.
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
ALTER TABLE fair_price
    -- Bound for a continuously-publishing source, whose age is a statement
    -- about now. Whole seconds, matching every other duration column here.
    ADD COLUMN leg_stale_tape_secs      BIGINT NOT NULL,
    -- Bound for a source published on a slow schedule, authoritative for the
    -- moment it names. Must exceed the longest gap between publications,
    -- holidays included, or that class drops out of the roster on a closure
    -- nobody is watching — which is why it is recorded separately and not
    -- assumed equal to the tape bound.
    ADD COLUMN leg_stale_reference_secs BIGINT NOT NULL,
    -- Both halves of what `FairValueConfig::validate` rejects, restated so a
    -- hand-written row cannot record a configuration the engine would have
    -- refused: a zero bound (which would age every source of that class out
    -- instantly), and an inverted pair (which would let a daily fix age out
    -- faster than a live tape — the exact inversion the class split exists to
    -- remove). Equal bounds are degenerate rather than wrong, so `>=` accepts
    -- them, matching the validator's own `reference < tape` test.
    ADD CONSTRAINT fair_price_leg_stale_bounds_ordered
        CHECK (leg_stale_tape_secs > 0
           AND leg_stale_reference_secs >= leg_stale_tape_secs);

COMMENT ON COLUMN fair_price.leg_stale_tape_secs IS
    'Configured age bound for a tape-class source on this tick, in seconds. '
    'A bound the composition was judged at, not an observed age.';

COMMENT ON COLUMN fair_price.leg_stale_reference_secs IS
    'Configured age bound for a reference-class source on this tick, in '
    'seconds. A bound the composition was judged at, not an observed age.';
