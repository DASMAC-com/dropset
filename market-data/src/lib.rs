//! The market-data collection app (`docs/data-feeds.md` §7) — the first
//! consumer of the shared `feeds` ingestion framework, and so the end-to-end
//! proof of its source → store-sink path.
//!
//! This crate owns the collector's **deployment shape** — its configuration and
//! its record → row mapping. The venue adapters it polls are not its own: they
//! live in `dropset_feeds::venues`, written once and shared with the bots
//! (`data-feeds.md` §4), so this crate names a source rather than implementing
//! one. The framework owns the drive loop, cursor persistence, and fan-out, and
//! `dropset-db-schema` owns every table either of them touches.
//!
//! **It is no longer write-only, and it is no longer only collectors.** Three
//! modules sit on the read side of the same tables: [`fx_store`] reads the newest
//! closed bucket per source and product back out of `cex_prices`, [`tick_store`]
//! does the same for the newest print in `spot_ticks`, and [`fair_price`]
//! serializes a composed fair value into the estimator's own output table. They
//! live here for the same reason — a reader of these tables belongs beside their
//! writers, so a column rename breaks one crate rather than two — and all three
//! are on the **price** path rather than the collection path, which is why their
//! failure postures differ from a collector's.
//!
//! [`estimator`] is the process that drives all three: it reads the legs,
//! composes one fair value per market, and publishes the tick. It is the one
//! binary here that is **not** a collector — it polls no venue — and the one that
//! does not run on the feeds runner, because that runner retries any source error
//! forever, which is the right policy for a gap in a series and the wrong one for
//! a publisher. Two readers rather than one because the peg leg is in the other
//! table: see [`tick_store`] for the writer split.
//!
//! The first feed was the shared Coinbase EURC/USDC reference price
//! (`data-feeds.md` §9). Two row shapes come out of the collectors, and the
//! distinction is the spine of this crate: the [`store`] module maps every
//! **candle** source onto `cex_prices`, and [`ticks`] maps every **spot print**
//! onto `spot_ticks`. A candle is an aggregate over a window; a tick is one
//! observation. The finest bucket any candle endpoint offers is 60s, so no
//! polling cadence makes a candle series show movement between closes — which
//! is what the tick tier exists for.
//!
//! **One collector process serves a venue, not a pair.** [`roster`] parses the
//! product list and owns the canonical ↔ venue *resolution* — including the
//! rule that two entries may never resolve to one venue symbol, and that a
//! venue deriving no spelling rejects a pinned one rather than ignoring it.
//! (The individual FX vendors' spelling rules live in [`fx`], beside the
//! credential handling those collectors share.) [`supervise`] runs the
//! resulting feeds concurrently and ends the process when the first of them
//! finishes, either way. Each feed keeps its own per-product cursor key, so
//! consolidating N per-pair services into one per-venue service resets
//! nothing.
//!
//! The FX collectors share the [`fx`] module: their configuration, the single
//! place a credential is resolved, and the canonical ↔ venue symbol mapping.
//! That last part is load-bearing rather than cosmetic — the three FX vendors
//! spell one currency pair three different ways, so the **stored** `product_id`
//! is canonical (`AUD-USD`) and each adapter is handed the spelling it wants.
//! Storing venue-native symbols would put one pair under three keys and make
//! the cross-source comparison these feeds exist for impossible.
//!
//! [`pyth_roster`] is the one venue whose coordinates cannot be derived from a
//! canonical id, so they are read from the store at startup instead of compiled
//! in — see that module for why the deployment target decides this.
//!
//! [`instruments`] is its mirror: where `pyth_roster` reads roster reference
//! data the store owns, `instruments` writes the roster the *environment* owns
//! into the store, so a dashboard can ask what kind of thing a product is
//! without hardcoding a product list into a panel. Both run once, at startup.
//!
//! Every binary asserts the shared schema at startup rather than provisioning
//! one (`data-feeds.md` §8), and then registers its roster in the instruments
//! dimension — the two startup writes that make a collector's configuration
//! legible from the store rather than only from its logs.

pub mod config;
pub mod estimator;
pub mod fair_price;
pub mod fx;
pub mod fx_store;
pub mod instruments;
pub mod pyth_roster;
pub mod roster;
pub mod store;
pub mod supervise;
pub mod tick_store;
pub mod ticks;
