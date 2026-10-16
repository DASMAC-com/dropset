-- Carry the three fields the on-chain `FillEvent` gained when it was laid
-- out again by concern, so a fill row says which counter a leg consumed,
-- what the level's flip price was, and how much of that counter is left —
-- without the reader re-deriving any of it from the vault's book.
--
-- `leg_kind` is 0 for a leg that consumed the level's own side and 1 for one
-- that consumed its flipped side. `flip_price` is the level's flip price at
-- emit, as raw `Price` bits like `fill_price`, and 0 when the level does not
-- flip; against `fill_price` it is the spread a flipping level captures.
-- `remaining_after` is the allowance left on the consumed side, after the leg,
-- in that side's atoms.
--
-- **Nullable, deliberately.** The table already holds legs indexed from the
-- previous event layout, which carried none of these, so a NOT NULL without a
-- DEFAULT would fail against those rows, and any DEFAULT would fabricate a
-- value: a zero `leg_kind` would mislabel a flipped leg as primary. NULL
-- therefore means "indexed before the event carried this field".
--
-- **Disclosure.** 0002 grants `dropset_ro` SELECT on every table in `public`.
-- All three are copied verbatim from an event the program publishes on chain,
-- so they disclose nothing a public RPC does not.
ALTER TABLE fill_events
    ADD COLUMN leg_kind SMALLINT,
    ADD COLUMN flip_price BIGINT,
    ADD COLUMN remaining_after NUMERIC;

COMMENT ON COLUMN fill_events.leg_kind IS
    '0 when the leg consumed the level''s own side, 1 when it consumed the '
    'flipped side. NULL for a leg indexed before the event carried it.';

COMMENT ON COLUMN fill_events.flip_price IS
    'The level''s flip price at emit, as raw Price bits; 0 when the level does '
    'not flip. NULL for a leg indexed before the event carried it.';

COMMENT ON COLUMN fill_events.remaining_after IS
    'Atoms left on the consumed side after the leg, in that side''s units. '
    'NULL for a leg indexed before the event carried it.';
