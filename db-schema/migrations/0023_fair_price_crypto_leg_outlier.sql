-- Record which source disagreed on each published composition's crypto
-- (basis) leg, so a corroborator's dispersion is visible on the row rather
-- than only in the estimator's log.
--
-- A market whose basis leg pairs a designated tape with an untrusted
-- corroborator — tGBP's Coinbase candle beside its Orca pool — composes on the
-- designated tape when the two disagree, and the regime does not move: a pool
-- anyone can walk past the band must not be able to demote the market. That
-- leaves `regime` and `degrade` silent about the disagreement by design, so
-- without this column the flag would exist only in the composition the
-- estimator held and threw away.
--
-- **An observation, not a verdict.** A value names the source the consensus
-- judged the outlier on this tick's crypto leg, and is set whenever that leg
-- was dispersed — including on a leg with no designation, where `degrade`
-- already reads `leg_dispersed`. It says nothing about whether the market
-- degraded; `regime` and `degrade` say that.
--
-- **Nullable, and NULL means two things a consumer need not tell apart:** the
-- leg was not dispersed, or the row predates this column. Both read as "no
-- disagreement recorded". No DEFAULT, for 0019's reason: one would fabricate
-- an observation for ticks that recorded none.
--
-- **Disclosure.** 0002 grants `dropset_ro` SELECT on every table in `public`,
-- and 0011 requires a column added here to extend its disclosure argument
-- rather than inherit it. A source label is a venue name from the published
-- roster, already disclosed by the collectors' own tables, and the `fair`
-- beside it dominates anything this adds.
ALTER TABLE fair_price
    ADD COLUMN crypto_leg_outlier TEXT;

COMMENT ON COLUMN fair_price.crypto_leg_outlier IS
    'The source judged the outlier on this tick''s crypto (basis) leg when '
    'that leg was dispersed; NULL when it was not, or when not recorded.';
