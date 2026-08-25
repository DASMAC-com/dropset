-- cspell:word pkey
--
-- Widen the Pyth FX roster from one-feed-per-currency to arbitrary crosses,
-- and seed every cross Hermes actually publishes for the roster currencies
-- (docs/data-feeds.md §9).
--
-- WHY THE KEY HAS TO MOVE.
--
-- `0005_pyth_fx_feeds.sql` keyed the roster on `currency`, on the stated
-- assumption that "Pyth publishes one feed per cross against USD, so a currency
-- names a feed exactly". That held while the roster was seven USD pairs. It
-- stops holding the moment a genuine cross is wanted: Hermes publishes EUR/GBP,
-- EUR/JPY and GBP/CAD alongside the USD pairs, so EUR alone names six feeds.
--
-- The natural key is therefore the canonical `product_id`, which was already
-- UNIQUE — this migration only promotes it. `currency` is kept and is now
-- documented for what it always was in practice: the **base** leg. Every
-- existing row satisfies that reading already (`CHF-USD` carries
-- `currency = 'CHF'`), so nothing is rewritten, and a `quote` column is added
-- beside it to make the pair explicit rather than something callers re-derive
-- by splitting a string.
--
-- WHY THE COLLECTOR AND THE MAKER NO LONGER HOLD THE SAME SET.
--
-- They were equal because the collector stored exactly what the maker quoted.
-- That is no longer the intent: the collector should ingest every rate we can
-- lawfully obtain, because history cannot be backfilled into a market that did
-- not exist yet, while the maker quotes only its configured markets and keeps a
-- compiled constant so an unreachable Postgres degrades quoting rather than
-- preventing startup. The invariant that replaces equality is **containment** —
-- every cross the maker quotes must be one the collector records — and
-- `market-data/tests/pyth_roster_agreement.rs` now asserts that instead.
--
-- ONLY FEEDS THAT ACTUALLY PUBLISH ARE SEEDED, AND THIS IS THE WHOLE CARE.
--
-- Presence in the Hermes FX catalogue does not mean a feed has ever carried a
-- price. Of the 53 catalogued feeds whose legs are both roster currencies, 27
-- publish and 26 report `publish_time = 0` — they have never published at all.
-- The adapter omits a feed Hermes did not answer for rather than erroring, so a
-- seeded dead feed is indistinguishable from an outage: silently missing data,
-- forever. Every id below was confirmed live on 2026-08-24 by reading
-- `/v2/updates/price/latest` and requiring a non-zero publish time.
--
-- The 26 that are catalogued but silent are deliberately NOT seeded, and are
-- listed here so the check is not repeated: AUD/SGD, BRL/JPY, EUR/BRL, EUR/IDR,
-- EUR/MXN, EUR/MYR, EUR/NGN, EUR/SGD, EUR/TRY, EUR/ZAR, GBP/BRL, GBP/IDR,
-- GBP/MXN, GBP/MYR, GBP/NGN, GBP/SGD, GBP/TRY, GBP/ZAR, IDR/JPY, MXN/JPY,
-- MYR/JPY, SGD/JPY, TRY/JPY, USD/MYR, USD/NGN, ZAR/JPY. Note USD/MYR and
-- USD/NGN among them: those are the two roster currencies with no live Pyth
-- feed at all, which is exactly why they depend on other vendors.
ALTER TABLE pyth_fx_feeds DROP CONSTRAINT pyth_fx_feeds_pkey;

-- Dropped rather than left in place: `product_id` was already declared UNIQUE
-- in 0005, and a primary key builds its own unique index, so keeping both would
-- leave two identical indexes on one column to be maintained on every write.
ALTER TABLE pyth_fx_feeds DROP CONSTRAINT pyth_fx_feeds_product_id_key;

ALTER TABLE pyth_fx_feeds ADD PRIMARY KEY (product_id);

-- The quote leg. Backfilled from the canonical id rather than typed twice, so
-- the existing seven rows cannot disagree with the ids they already carry.
ALTER TABLE pyth_fx_feeds ADD COLUMN quote TEXT;

UPDATE pyth_fx_feeds SET quote = split_part(product_id, '-', 2);

ALTER TABLE pyth_fx_feeds ALTER COLUMN quote SET NOT NULL;

ALTER TABLE pyth_fx_feeds
    ADD CONSTRAINT quote_is_iso_4217 CHECK (quote ~ '^[A-Z]{3}$');

-- And the pair has to be the id it is stored under, which is what stops a row
-- claiming EUR-GBP while carrying the legs of EUR-JPY.
ALTER TABLE pyth_fx_feeds
    ADD CONSTRAINT legs_match_product_id
        CHECK (product_id = currency || '-' || quote);

COMMENT ON COLUMN pyth_fx_feeds.currency IS
    'ISO 4217 code of the base leg. Named `currency` because this table began '
    'as one row per currency against USD; it is the base leg of `product_id`.';

COMMENT ON COLUMN pyth_fx_feeds.quote IS
    'ISO 4217 code of the quote leg — the second half of `product_id`.';

-- The twenty crosses Hermes publishes for roster currencies that 0005 did not
-- seed: five USD pairs, and fifteen crosses with no USD leg at all. `invert` is
-- TRUE only where Hermes publishes the reciprocal of the canonical id, which
-- for a non-USD cross is never — those are stored the way they are published.
INSERT INTO pyth_fx_feeds (currency, quote, product_id, feed_id, invert) VALUES
    ('AUD', 'USD', 'AUD-USD', '67a6f93030420c1c9e3fe37c1ab6b77966af82f995944a9fefce357a22854a80', false),
    ('BRL', 'USD', 'BRL-USD', 'd2db4dbf1aea74e0f666b0e8f73b9580d407f5e5cf931940b06dc633d7a95906', true),
    ('CAD', 'USD', 'CAD-USD', '3112b03a41c910ed446852aacf67118cb1bec67b2cd0b9a214c58cc0eaa2ecca', true),
    ('JPY', 'USD', 'JPY-USD', 'ef2c98c804ba503c6a707e38be4dfbb16683775f195b091252bf24693042fd52', true),
    ('TRY', 'USD', 'TRY-USD', '032a2eba1c2635bf973e95fb62b2c0705c1be2603b9572cc8d5edeaf8744e058', true),
    ('AUD', 'CAD', 'AUD-CAD', '95330ad1bcac1bd79179fe59000bfe199ba3fe7f03254220548ef2d034bdf4d6', false),
    ('AUD', 'CHF', 'AUD-CHF', '56e94c0381e42a81a15a46daf35f59f391c074ef1770ef33829475c9b797b420', false),
    ('AUD', 'JPY', 'AUD-JPY', '8dbbb66dff44114f0bfc34a1d19f0fe6fc3906dcc72f7668d3ea936e1d6544ce', false),
    ('CAD', 'CHF', 'CAD-CHF', '4db9de9866f63172964e8fd048241253c62d50b73c6cba98ce65dd634c8fc6de', false),
    ('CAD', 'JPY', 'CAD-JPY', '9e19cbf0b363b3ce3fa8533e171f449f605a7ca5bb272a9b80df4264591c4cbb', false),
    ('CHF', 'JPY', 'CHF-JPY', 'e9f0f24d8828dc49e1d7aa6b82373dfaf671f8e28cbf9600b14008670c82a462', false),
    ('EUR', 'AUD', 'EUR-AUD', 'f51e4e46f00cb9153ddb379ea26672084c0263126c56102af148402b7a6d11d3', false),
    ('EUR', 'CAD', 'EUR-CAD', 'fec44951e54a606cbbca6fc7fb721c33bb54e4ae641a8a12d5df94313d635a12', false),
    ('EUR', 'CHF', 'EUR-CHF', '6194ee9b4ae25932ae69e6574871801f0f30b4a3317877c55301a45902aa0c1a', false),
    ('EUR', 'GBP', 'EUR-GBP', 'c349ff6087acab1c0c5442a9de0ea804239cc9fd09be8b1a93ffa0ed7f366d9c', false),
    ('EUR', 'JPY', 'EUR-JPY', 'd8c874fa511b9838d094109f996890642421e462c3b29501a2560cecf82c2eb4', false),
    ('GBP', 'AUD', 'GBP-AUD', 'bbcf32c739841d1170ae2dfaf7c1bd2483df5cf241e2ecf5bce5d14cf09982b1', false),
    ('GBP', 'CAD', 'GBP-CAD', 'ff940d31a543df4485af8f08e81c638cab5af80e399d9928d34f73838a8a106b', false),
    ('GBP', 'CHF', 'GBP-CHF', 'ae95ee182ff568100d09257956a01d6bd663072e62fe108bae42ecca4400f527', false),
    ('GBP', 'JPY', 'GBP-JPY', 'cfa65905787703c692c3cac2b8a009a1db51ce68b54f5b206ce6a55bfa2c3cd1', false);
