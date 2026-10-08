-- Store the fiat each stablecoin tracks, which `0009_instruments.sql` chose not
-- to.
--
-- 0009 left the peg as a trailing comment on each seed row, on the grounds that
-- classifying a pair needs only its two legs' kinds and a stored peg would be a
-- second fact to keep in step with the roster. Both halves still hold for
-- classification. What changed is that a reader now needs the peg itself: a
-- maker market is named by its stablecoin (`EURC`), while the FX leg it prices
-- off is named by the sovereign pair (`EUR-USD`), and nothing in the schema
-- joins the two. A dashboard that offers both a market and a product therefore
-- cannot narrow one by the other, and lets the operator pick a product that has
-- nothing to do with the market selected.
--
-- With the peg stored, "the products relevant to market M" is a join rather
-- than a convention: every instrument with M, or the fiat M tracks, as a leg.
--
-- WHY A COLUMN AND NOT A RULE. The peg is not derivable from the symbol, which
-- is the same reason `currency_kinds` is hand-maintained at all: `BRZ` tracks
-- BRL, `QCAD` and `CNGN` carry a prefix, and `GYEN` tracks JPY. A prefix match
-- gets several of the seed wrong and fails silently on each.
--
-- THE CONSTRAINT IS THE POINT. A stablecoin row must carry a peg and no other
-- row may, so a later seed that adds a stablecoin and forgets its peg fails at
-- migrate time rather than leaving a market whose product picker is silently
-- empty. That makes the roster coupling 0009 warns about one fact stricter: a
-- new stablecoin is now two columns, not one, and a plain `(currency, kind)`
-- insert of one is rejected. The reference ensures the peg names a seeded
-- currency; that it names a FIAT one is a cross-row fact a CHECK cannot state,
-- so the test suite asserts it instead.
--
-- The column is added nullable, backfilled, and only then constrained, because
-- the CHECK would reject every existing stablecoin row between the first two
-- steps. One migration rather than two, so no applied state ever has the
-- column without its rule.
ALTER TABLE currency_kinds
    ADD COLUMN pegged_to TEXT REFERENCES currency_kinds (currency);

-- Every stablecoin seeded by 0009 and 0013, pegs as 0009's
-- comments record them. An unmatched row is left NULL and fails the constraint
-- below, which is the intended failure mode for a seed this list missed.
UPDATE currency_kinds AS c
SET pegged_to = p.fiat
FROM (VALUES
    ('AUDD', 'AUD'),
    ('AUDM', 'AUD'),
    ('BRZ', 'BRL'),
    ('CADC', 'CAD'),
    ('CNGN', 'NGN'),
    ('EURAU', 'EUR'),
    ('EURC', 'EUR'),
    ('EURCV', 'EUR'),
    ('EUROP', 'EUR'),
    ('GYEN', 'JPY'),
    ('IDRX', 'IDR'),
    ('MXNE', 'MXN'),
    ('MYRC', 'MYR'),
    ('PYUSD', 'USD'),
    ('QCAD', 'CAD'),
    ('TGBP', 'GBP'),
    ('TRYB', 'TRY'),
    ('USD1', 'USD'),
    ('USDC', 'USD'),
    ('USDG', 'USD'),
    ('USDT', 'USD'),
    ('VCHF', 'CHF'),
    ('VGBP', 'GBP'),
    ('XSGD', 'SGD'),
    ('ZARP', 'ZAR'),
    ('ZARU', 'ZAR')
) AS p (currency, fiat)
WHERE c.currency = p.currency;

ALTER TABLE currency_kinds
    ADD CONSTRAINT stablecoin_has_a_peg
        CHECK ((kind = 'stablecoin') = (pegged_to IS NOT NULL));

COMMENT ON COLUMN currency_kinds.pegged_to IS
    'The fiat a stablecoin tracks (EURC -> EUR); NULL for every other kind, '
    'which the stablecoin_has_a_peg constraint enforces. Lets a reader join a '
    'maker market, named by its stablecoin, to the FX pair it prices off.';
