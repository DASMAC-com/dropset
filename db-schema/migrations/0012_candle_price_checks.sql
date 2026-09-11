-- Price sanity CHECKs on `cex_prices`, the CEX reference candle table 0001
-- created: every price finite and positive, and a bucket's high never below
-- its low.
--
-- **Why the schema rather than only the collectors.** An adapter validates
-- what it parses, and the strength of that guard is per-adapter rather than
-- uniform. Nothing downstream reliably narrows it either. The insert is
-- idempotent rather than validating, and while some analytics queries do
-- carry a defensive `close > 0` — a log-return divides by it, so they have
-- to — that guard is per-query and partial: it covers the one column the
-- arithmetic forces and says nothing about `low`, `high`, `open`, or the
-- ordering. Absent a constraint every consumer has to remember its own, and
-- a malformed bucket reaches the ones that forgot as an impossible spread or
-- a nonsense return — wrong information rather than missing information. A
-- constraint is the one point a new writer cannot bypass.
--
-- **Three constraints rather than one**, because they fail for unrelated
-- reasons and the constraint name is the whole diagnostic a violation
-- carries: a non-positive price means a parse or a unit went wrong; a
-- non-finite one means an upstream sentinel or a division reached the column
-- intact; an inverted high/low means a bar was assembled or transformed
-- wrongly. A direction-flipping adapter is the obvious source of the last,
-- since inverting a bar must swap high and low — `x -> 1/x` reverses order.
--
-- **`> 0`, not `>= 0`.** A zero price is not a cheap quote, it is a missing
-- one, and a bar carrying it would compute a zero or infinite return rather
-- than declining to answer.
--
-- **`>=` for high against low, not `>`.** A flat bucket — an interval in
-- which the rate did not move — is a routine outcome on an illiquid pair or
-- in a quiet session, not a defect, and `>` would reject it. This is the
-- relation most at risk of being "tightened" later by a reader who takes
-- `high = low` for a symptom, so the choice is stated rather than inferred.
--
-- **Why an explicit upper bound of infinity, which looks redundant beside
-- `> 0` and is not.** Postgres orders `NaN` as GREATER than every other
-- float, so `NaN > 0` holds and a positivity test alone admits it. Nor does
-- the IEEE trick of comparing a value with itself help, because this engine
-- treats `NaN = NaN` as true. `x < 'Infinity'` is false for both `NaN` and
-- `Infinity` and true for every finite value, so pairing it with `x > 0` is
-- what makes these columns mean "a real price". Without it these constraint
-- names would assert more than they check.
--
-- Read those two as a PAIR, because neither is exhaustive alone and the
-- boundary between them is not where the names suggest. A negative infinity
-- satisfies `x < 'Infinity'` and is refused by the positivity check instead
-- — which is not a gap, since a negative infinity is genuinely not positive,
-- but it does mean a division artefact can surface under either name. Only
-- the conjunction says "finite and positive".
--
-- **Volume is deliberately unconstrained.** Zero volume is legitimate and
-- routine — some sources publish none at all — so the column carries no
-- positivity invariant. Which sources, and why, is a property of the feed
-- roster and changes with it, so it lives in docs/data-feeds.md rather than
-- here. Named without a section number deliberately: headings renumber, and
-- this file cannot be corrected when they do.
--
-- **Deliberately NOT here: the full OHLC ordering**, that `open` and `close`
-- each sit within `[low, high]`. It is a coherent stronger invariant and a
-- plausible next step, but it is a wider claim about how every present and
-- future adapter assembles a bar, so it is its own decision rather than a
-- rider on this one. Nothing here should be read as having settled it.
--
-- **Added VALIDATED — the default — rather than `NOT VALID`.** `NOT VALID`
-- would enforce the invariant on new writes only, leaving the rows already
-- stored exempt. That exemption is retractable in principle — it is what
-- `ALTER TABLE ... VALIDATE CONSTRAINT` exists for — but only prospectively:
-- rows read and reported on while it stood were read without the guarantee,
-- and the historical series is what any backfilled analysis reads. So the
-- cost of the validating form is that this migration fails if a stored bar
-- violates it, and that is the correct outcome: such a bar is the corruption
-- these constraints exist to make loud, and it wants adjudicating rather
-- than grandfathering.
ALTER TABLE cex_prices
    ADD CONSTRAINT prices_are_positive
        CHECK (low > 0 AND high > 0 AND open > 0 AND close > 0);

ALTER TABLE cex_prices
    ADD CONSTRAINT prices_are_finite
        CHECK (low < 'Infinity'::double precision
            AND high < 'Infinity'::double precision
            AND open < 'Infinity'::double precision
            AND close < 'Infinity'::double precision);

ALTER TABLE cex_prices
    ADD CONSTRAINT high_at_least_low
        CHECK (high >= low);
