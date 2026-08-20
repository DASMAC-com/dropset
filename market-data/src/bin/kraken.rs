//! The Kraken spot-tick collector: one batched `/0/public/Ticker` request
//! prices the whole roster, and each reading lands in `spot_ticks`
//! (docs/data-feeds.md §9).
//!
//! This is the venue carrying the leg no other keyless source does: a real
//! market print of the `USDC/USD` peg. Coinbase Exchange lists no `USDC-USD`
//! product and Binance.US quotes an administered flat `1.00`, so collecting it
//! here is what lets a peg dislocation be read after the fact rather than only
//! watched live.
//!
//! The default roster collects the two legs the maker already subscribes to —
//! `USDC-USD` and `EURC-USD`. Kraken *also* lists `EURC/EUR`, token against its
//! own fiat, which is the closest live stand-in for an issuer redemption rate
//! and which nothing consumes yet; adding `EURC-EUR` here would cost no extra
//! request, since the poll is batched. Left out deliberately rather than by
//! oversight: what to record is a decision about the store's contents, not a
//! detail of this collector, and this PR collects what is wired.
//!
//! Keyless, batched, and cheap: one request per poll regardless of roster size,
//! so unlike the metered FX venues nothing here has to widen its cadence as
//! pairs are added.
//!
//! **Pairs are Kraken's own names, not ours.** The roster stores canonical
//! `BASE-QUOTE` ids and derives Kraken's spelling by concatenation, which is
//! right for most pairs and wrong for the legacy assets that keep an `X`/`Z`
//! prefix — those pin their spelling in the roster entry. A pair Kraken does not
//! recognize is omitted from its response rather than erroring, so the silence
//! watch is what turns a misspelling into a log line instead of a mystery.

use dropset_feeds::{
    connect, run,
    venues::{KrakenSource, Quotes},
    RunConfig, Sink, StoreSink,
};
use dropset_market_data::{
    roster::{kraken_pair, resolve_venue, roster_from_env},
    ticks::{SilenceWatch, Tick, TickConfig, TickDefaults, TickSource, TickWriter},
};
use std::collections::HashMap;
use std::time::Duration;

/// The value written to `spot_ticks.source`.
const SOURCE: &str = "kraken";

/// The cursor key this collector's sink is wired with. A ticker has no resume
/// position — every poll is the present — so nothing is ever written under it;
/// it exists because the store sink is built around a feed identity.
const FEED: &str = "ticks:kraken";

/// The two legs the maker's fair-value model needs corroborated, which are also
/// the only two of the demo roster Kraken lists.
const DEFAULT_PRODUCTS: &str = "USDC-USD,EURC-USD";

/// How many polls a configured pair may stay unpriced before it is reported as
/// a roster mistake rather than a venue gap.
const SILENCE_THRESHOLD: u32 = 4;

const DEFAULTS: TickDefaults = TickDefaults {
    base_url: "https://api.kraken.com",
    poll_interval_secs: 15,
};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let cfg = TickConfig::from_env(&DEFAULTS)?;
    let products = roster_from_env(DEFAULT_PRODUCTS)?;
    // Resolve every venue spelling before connecting: a malformed canonical id
    // must fail startup, not become a series that never appears — and two
    // entries resolving to one Kraken pair must fail here rather than have the
    // venue's single answer filed under whichever one won.
    let pairs = resolve_venue(&products, kraken_pair)?;
    let pool = connect(&cfg.database_url).await?;
    dropset_db_schema::require_schema(&pool).await?;

    let venue_pairs: Vec<String> = pairs.iter().map(|p| p.venue_symbol.clone()).collect();
    let canonical: Vec<String> = pairs.iter().map(|p| p.product_id.clone()).collect();
    // Kraken answers under the name it was asked with, so this maps its keys
    // back onto the canonical ids the rows are stored under. `resolve_venue`
    // has already rejected a duplicate venue symbol, so no entry is lost here.
    let index: HashMap<String, String> = pairs
        .into_iter()
        .map(|p| (p.venue_symbol, p.product_id))
        .collect();

    tracing::info!(
        products = %canonical.join(","),
        pairs = %venue_pairs.join(","),
        poll_secs = cfg.poll_interval_secs,
        "kraken tick collector starting"
    );

    let source = KrakenSource::new(&cfg.base_url, venue_pairs)?;
    let mut watch = SilenceWatch::new(canonical, SILENCE_THRESHOLD);
    let source = TickSource::new(source, move |quotes: &Quotes<String>, poll_secs| {
        let ticks: Vec<Tick> = quotes
            .iter()
            .filter_map(|(pair, price)| {
                index.get(pair).map(|product_id| Tick {
                    product_id: product_id.clone(),
                    // The venue publishes no per-pair timestamp in this
                    // response, so the poll second is the honest attribution.
                    observed_at: poll_secs,
                    price: *price,
                    // Kraken has no confidence notion; NULL rather than zero.
                    confidence: None,
                })
            })
            .collect();
        watch.observe(&ticks);
        ticks
    });

    let sinks: Vec<Box<dyn Sink<Tick>>> = vec![Box::new(StoreSink::new(
        pool,
        FEED,
        TickWriter::new(SOURCE),
    ))];
    let run_cfg = RunConfig {
        poll_interval: Duration::from_secs(cfg.poll_interval_secs),
        ..RunConfig::default()
    };
    run(source, sinks, run_cfg).await
}
