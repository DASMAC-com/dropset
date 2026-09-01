//! The Pyth Hermes FX tick collector: one batched request prices every cross on
//! the roster, and each reading lands in `spot_ticks` with the publisher's
//! confidence half-width (docs/data-feeds.md §9).
//!
//! This is the FX tier that publishes a **confidence** alongside its rate, which
//! is why it outranks the daily fallbacks in the maker's model — and why storing
//! it matters: a rate whose uncertainty is not recorded cannot later be
//! distinguished from one that was firm.
//!
//! Its roster comes from the store rather than the environment or a constant —
//! see [`dropset_market_data::pyth_roster`] for why the deployment target
//! decides that. Read once, at startup, and logged; restart to apply a change.
//!
//! `observed_at` is Hermes' own `publish_time`, not the poll second. That is
//! what makes a re-poll idempotent: the publishers agreed on that instant, so
//! re-fetching the same reading lands on the primary key instead of writing a
//! second row for one observation.

use dropset_feeds::{
    connect, run,
    venues::{pyth::FxQuote, PythHermesSource},
    RunConfig, Sink, StoreSink,
};
use dropset_market_data::{
    instruments::register as register_instruments,
    pyth_roster,
    ticks::{SilenceWatch, Tick, TickConfig, TickDefaults, TickSource, TickWriter},
};
use std::collections::HashMap;
use std::time::Duration;

/// The value written to `spot_ticks.source`.
const SOURCE: &str = "pyth";

/// The cursor key this collector's sink is wired with. A latest-price endpoint
/// has no resume position, so nothing is written under it.
const FEED: &str = "ticks:pyth";

/// How many polls a configured cross may stay unpriced before it is reported as
/// a roster mistake rather than a venue gap. This venue needs the check most:
/// its feed ids are opaque hex, so a typo cannot be caught by eye.
const SILENCE_THRESHOLD: u32 = 4;

const DEFAULTS: TickDefaults = TickDefaults {
    base_url: "https://hermes.pyth.network",
    poll_interval_secs: 15,
};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let cfg = TickConfig::from_env(&DEFAULTS)?;
    let pool = connect(&cfg.database_url).await?;
    dropset_db_schema::require_schema(&pool).await?;

    // The roster is reference data in the store, read once here. An empty one
    // is a startup failure rather than an empty run — see `pyth_roster::load`.
    let roster = pyth_roster::load(&pool).await?;
    let feeds = pyth_roster::to_feeds(&roster);
    let products = pyth_roster::product_ids(&roster);
    // This venue's roster comes from the store rather than the environment, so
    // the round trip is store-to-store — but the dimension is about what is
    // being polled, not where the instruction came from, and Pyth's crosses
    // belong in it like any other.
    register_instruments(&pool, &products).await?;

    // Name every loaded row, so the effective roster of a running process is
    // legible without querying the database it came from. This is the log line
    // that makes startup-read-and-restart an acceptable substitute for live
    // reload.
    for cross in &roster {
        tracing::info!(
            currency = %cross.currency,
            product = %cross.product_id,
            feed_id = %cross.feed_id,
            invert = cross.invert,
            "roster cross loaded"
        );
    }
    tracing::info!(
        products = %products.join(","),
        poll_secs = cfg.poll_interval_secs,
        "pyth tick collector starting"
    );

    let source = PythHermesSource::new(&cfg.base_url, feeds)?;
    let source = TickSource::new(
        source,
        move |quotes: &HashMap<String, FxQuote>, poll_secs| {
            quotes
                .iter()
                .map(|(product_id, quote)| Tick {
                    product_id: product_id.clone(),
                    // Prefer the venue's publish time; fall back to the poll
                    // second only if it is missing, since a zero would attribute
                    // the reading to the epoch.
                    observed_at: if quote.publish_time > 0 {
                        quote.publish_time
                    } else {
                        poll_secs
                    },
                    price: quote.value,
                    confidence: quote.confidence,
                })
                .collect()
        },
    )
    // Driven once per poll by the adapter, not from the closure above — see
    // `TickSource::watching`.
    .watching(SilenceWatch::new(products, SILENCE_THRESHOLD));

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
