-- The Pyth Hermes FX roster: which currency crosses the FX collector polls,
-- and the venue coordinates each one needs (docs/data-feeds.md §9, §12).
--
-- WHY THIS IS A TABLE AND NOT A CONSTANT.
--
-- A Pyth feed is addressed by a 32-byte hex id that nothing derives — unlike
-- every other venue coordinate in this system, where the canonical `BASE-QUOTE`
-- product id yields the venue's spelling by rule (OANDA `AUD_USD`, Twelve Data
-- `AUD/USD`, Kraken `AUDUSD`). So the ids have to be configured somewhere, and
-- the deployment target decides where: post-gate these collectors run on ECS,
-- which offers environment variables but no way to mount a configuration file.
-- The options were therefore a compiled constant (a rebuild and a redeploy to
-- add a currency), a base64 blob in an environment variable, or reference data
-- in the store the collectors are already required to reach. This is the third.
-- Adding an eighth cross is an INSERT plus a service restart.
--
-- Scope is deliberately narrow: **slowly-changing venue reference data, seeded
-- by this migration, with no runtime writer.** It is explicitly not a general
-- parameter channel — operator-tunable runtime parameters with desired/applied
-- semantics are a separate design with its own issue, and shaping this table to
-- anticipate it would be designing that here.
--
-- The collector reads this once, at startup, and logs every row it loaded.
-- Restart to apply a change; there is no live reload. That keeps the effective
-- roster of a running process legible in one log line, which matters more than
-- avoiding a restart for data that changes a few times a year.
--
-- INTEGRITY. The Pyth adapter *omits* a feed Hermes did not answer for rather
-- than erroring — deliberately, so one unpublished cross cannot take the whole
-- roster down. The cost of that choice is that a mistyped id is
-- indistinguishable from a venue outage: both are silently missing data. A
-- compiled constant is unit-tested for shape; a table row gets nothing for
-- free, so the checks below are the replacement and are not optional. The
-- collector covers the other half by warning distinctly when a *configured*
-- feed yields no reading.
CREATE TABLE pyth_fx_feeds (
    -- ISO 4217 code of the fiat leg. The natural key: Pyth publishes one feed
    -- per cross against USD, so a currency names a feed exactly.
    currency     TEXT    PRIMARY KEY,
    -- The canonical id the readings are stored under, e.g. `EUR-USD`. Held
    -- rather than derived from `currency` so the stored key stays explicit at
    -- the point the roster is edited.
    product_id   TEXT    NOT NULL UNIQUE,
    -- Hermes' 32-byte feed id, lowercase hex, no `0x` prefix.
    feed_id      TEXT    NOT NULL UNIQUE,
    -- TRUE when Hermes publishes the cross as `USD/<ccy>` and the reading has
    -- to be reciprocated into USD per `<ccy>`. Five of the seven seeded
    -- currencies are quoted that way.
    invert       BOOLEAN NOT NULL,
    -- Lets a cross be parked without deleting its coordinates, so turning one
    -- off is not a lossy edit.
    --
    -- **Parking a cross needs a matching maker-bot change.** This is the one
    -- edit that breaks the seed-versus-constant agreement without touching
    -- either file, so the test that pins them cannot see it: setting this FALSE
    -- stops the collector recording the cross while the maker keeps quoting
    -- from its compiled roster, leaving it quoting a cross with no stored
    -- history to compare against. Nothing enforces that pairing — it is a
    -- deliberately manual affordance, so it is written down here.
    enabled      BOOLEAN NOT NULL DEFAULT true,
    CONSTRAINT currency_is_iso_4217
        CHECK (currency ~ '^[A-Z]{3}$'),
    CONSTRAINT product_id_is_canonical
        CHECK (product_id ~ '^[A-Z]{3}-[A-Z]{3}$'),
    -- The check that matters most: a 32-byte hex id, so a truncated or
    -- `0x`-prefixed paste fails at the INSERT instead of becoming a feed that
    -- silently never reports.
    CONSTRAINT feed_id_is_32_bytes_of_hex
        CHECK (feed_id ~ '^[0-9a-f]{64}$')
);

-- The seven crosses the demo roster covers, from the Hermes FX catalogue
-- (`/v2/price_feeds?asset_type=fx`). Only EUR and GBP are published as
-- `<ccy>/USD`; the rest are `USD/<ccy>` and carry `invert = true`.
--
-- These same ids also exist as a compiled constant in the maker bot, which
-- cannot depend on this table: Postgres is a *soft* dependency there by design
-- (an unreachable database means degraded quoting, never a refusal to start),
-- so its roster must survive with no store at all. That duplication is
-- deliberate and is held honest by a test asserting this seed and that constant
-- agree.
INSERT INTO pyth_fx_feeds (currency, product_id, feed_id, invert) VALUES
    ('EUR', 'EUR-USD', 'a995d00bb36a63cef7fd2c287dc105fc8f3d93779f062f09551b0af3e81ec30b', false),
    ('GBP', 'GBP-USD', '84c2dde9633d93d1bcad84e7dc41c9d56578b7ec52fabedc1f335d673df0a7c1', false),
    ('CHF', 'CHF-USD', '0b1e3297e69f162877b577b0d6a47a0d63b2392bc8499e6540da4187a63e28f8', true),
    ('ZAR', 'ZAR-USD', '389d889017db82bf42141f23b61b8de938a4e2d156e36312175bebf797f493f1', true),
    ('MXN', 'MXN-USD', 'e13b1c1ffb32f34e1be9545583f01ef385fde7f42ee66049d30570dc866b77ca', true),
    ('SGD', 'SGD-USD', '396a969a9c1480fa15ed50bc59149e2c0075a72fe8f458ed941ddec48bdb4918', true),
    ('IDR', 'IDR-USD', '6693afcd49878bbd622e46bd805e7177932cf6ab0b1c91b135d71151b9207433', true);
