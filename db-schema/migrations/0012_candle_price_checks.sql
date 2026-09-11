-- Price sanity CHECKs on `cex_prices`, the CEX reference candle table 0001
-- created: every price positive, and a bucket's high never below its low.
--
-- **The gap these close is a silent one, which is what makes it worth a
-- migration.** A venue adapter validates what it parses, and that is precisely
-- the reach of that guard — the bytes one adapter received. Nothing downstream
-- narrows it: the store sink's write is idempotent, not validating, and every
-- consumer treats a stored bar as already-true. So a non-positive price or an
-- inverted high/low arriving by any path an adapter does not cover stores
-- without a sound, and surfaces much later and somewhere else as an impossible
-- spread or a nonsense return in an analysis — wrong information rather than
-- missing information, which is the failure shape this schema spends effort on
-- elsewhere (0009's LEFT JOIN note is the same argument about a different
-- column). A constraint is the one place the check cannot be bypassed by
-- adding a writer, which a collector-side guard cannot promise.
--
-- **Two constraints rather than one**, because they fail for unrelated reasons
-- and the constraint name is the whole diagnostic a violation carries: a
-- non-positive price means a parse or a unit went wrong, while an inverted
-- high/low means a bar was assembled or transformed wrongly. The OANDA
-- direction flip is the live instance of the second — inverting a bar has to
-- swap high and low, since `x -> 1/x` reverses their order, so this is exactly
-- the corruption an inversion bug would produce.
--
-- **`> 0`, not `>= 0`.** A zero price is not a cheap quote, it is a missing
-- one, and a bar carrying it would compute a zero or infinite return rather
-- than declining to answer. Volume is the deliberate opposite and is left
-- unconstrained here: zero volume is legitimate and routine — two wired
-- sources publish no volume at all and their rows carry `0.0` — so the column
-- has no positivity invariant to assert.
--
-- **Deliberately NOT here: the full OHLC ordering**, that `open` and `close`
-- each sit within `[low, high]`. It is a coherent stronger invariant and a
-- plausible next step, but it is a wider claim about how every present and
-- future adapter assembles a bar, so it is its own decision rather than a
-- rider on this one. Nothing here should be read as having settled it.
--
-- **Added VALIDATED — the default — rather than `NOT VALID`.** `NOT VALID`
-- would exempt the rows already stored, permanently: Postgres would enforce
-- the invariant on new writes only, and the exemption is not something a later
-- migration can retract for rows that have since been read and reported on. It
-- is the historical series that the pricing-verification work reads, so an
-- exemption would leave the guarantee off exactly where it is being relied on.
-- The cost of the validating form is that this migration fails if any stored
-- bar violates it — which is the correct outcome, because such a bar is the
-- corruption this constraint exists to make loud, and it should be adjudicated
-- rather than grandfathered.
ALTER TABLE cex_prices
    ADD CONSTRAINT cex_prices_prices_positive
        CHECK (low > 0 AND high > 0 AND open > 0 AND close > 0);

ALTER TABLE cex_prices
    ADD CONSTRAINT cex_prices_high_at_least_low
        CHECK (high >= low);
