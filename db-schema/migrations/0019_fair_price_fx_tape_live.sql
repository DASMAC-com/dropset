-- Record whether each published composition's FX anchor rested on a live tape,
-- so the maker's fail-closed tape guard survives the move from composing inline
-- to reading this table.
--
-- The maker used to decide this itself: it held the composition's FX leg
-- report, and halted a pair that requires a live tape when every source
-- credited on that leg was a daily fix. A row in this table carries the
-- composition's result but not its contributors, so a maker reading the row
-- could no longer ask the question — and the guard would have been lost
-- silently, in exactly the move that was meant to make the published fair the
-- one authority. The estimator holds the leg report, so it answers here.
--
-- **An observation, not a policy.** TRUE means at least one intraday tape was
-- credited on this tick's FX leg; it says nothing about whether this pair is
-- *allowed* to quote without one. Whether a FALSE halts is the consumer's call,
-- which is why this is a fact about the composition rather than a verdict.
--
-- **Nullable, deliberately, and NULL is not FALSE's synonym in meaning — only
-- in handling.** Unlike 0014's bounds, this lands on a table the estimator is
-- already writing, so a NOT NULL without a DEFAULT would fail against the rows
-- already there, and any DEFAULT would fabricate an observation for ticks that
-- recorded none. NULL therefore means "not recorded": every row written before
-- this column existed, and any row from an estimator that predates it. A
-- consumer must treat it as no evidence of a tape — the fail-closed reading —
-- which is the same handling as FALSE for a different reason.
--
-- **Disclosure.** 0002 grants `dropset_ro` SELECT on every table in `public`,
-- and 0011 requires a column added here to extend its disclosure argument
-- rather than inherit it. This one rests on 0011's own precedent ground: which
-- class of source priced a composition is already recoverable from the
-- `regime` and `anchor` columns to within the daily-fix case, and the published
-- `fair` beside it dominates anything this adds.
ALTER TABLE fair_price
    ADD COLUMN fx_tape_live BOOLEAN;

COMMENT ON COLUMN fair_price.fx_tape_live IS
    'Whether an intraday tape was credited on this tick''s FX leg. NULL when '
    'not recorded; a consumer treats NULL as no evidence of a tape.';
