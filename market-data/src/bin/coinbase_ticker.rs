//! The Coinbase spot-tick collector: poll each roster product's ticker and
//! persist the print into `spot_ticks` (docs/data-feeds.md §9).
//!
//! **This is the collector that makes the overlay move.** The candles feed on
//! the same venue cannot: 60s is the finest bucket the candles endpoint offers,
//! so no polling cadence makes a candle series show anything between closes.
//! These are the prints in between.
//!
//! Unlike the Kraken and Pyth tick feeds, the Coinbase ticker endpoint is keyed
//! by a single product, so a roster needs one source per product — several
//! feeds in one process rather than one batched request. They share a transport
//! so the venue still sees one request budget, and each is supervised together
//! so a dead pair takes the process down instead of going quiet.
//!
//! No silence watch here, unlike the batched collectors, and the difference is
//! structural rather than an omission: a batched venue answers for its whole
//! roster in one response and simply leaves out what it could not price, so a
//! misconfigured symbol is invisible without tracking which ones ever appeared.
//! Here each product has its own named feed, so one that never yields a record
//! is already identifiable by that name in the logs.

use dropset_feeds::{connect, run, venues::CoinbaseTicker, HttpClient, RunConfig, Sink, StoreSink};
use dropset_market_data::{
    roster::roster_from_env,
    supervise::run_all,
    ticks::{Tick, TickConfig, TickDefaults, TickSource, TickWriter},
};
use std::time::Duration;

/// The value written to `spot_ticks.source`. The same label the candles
/// collector writes to `cex_prices.source`: it is the same venue, and the table
/// is what distinguishes a print from a bucket.
const SOURCE: &str = "coinbase";

/// Coinbase lists only the one demo-roster token, so the default is a roster of
/// one — but it is a roster, and adding a product is now a config change rather
/// than a second service.
const DEFAULT_PRODUCTS: &str = "EURC-USDC";

const DEFAULTS: TickDefaults = TickDefaults {
    base_url: "https://api.exchange.coinbase.com",
    poll_interval_secs: 15,
};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let cfg = TickConfig::from_env(&DEFAULTS)?;
    let products = roster_from_env(DEFAULT_PRODUCTS)?;
    let pool = connect(&cfg.database_url).await?;
    dropset_db_schema::require_schema(&pool).await?;

    // One transport, cloned per product: these sources all reach the same host
    // from the same egress IP, which is what the public tier limits on, so a
    // client each would multiply the venue's budget by the roster size.
    let http = HttpClient::new(&cfg.base_url)?;

    let ids: Vec<String> = products
        .iter()
        .map(|entry| entry.product_id.clone())
        .collect();
    tracing::info!(
        products = %ids.join(","),
        poll_secs = cfg.poll_interval_secs,
        "coinbase tick collector starting"
    );

    let run_cfg = RunConfig {
        poll_interval: Duration::from_secs(cfg.poll_interval_secs),
        ..RunConfig::default()
    };
    let mut feeds = Vec::with_capacity(ids.len());
    for product_id in ids {
        // Coinbase names its products the canonical way already, so there is no
        // spelling to derive here.
        let ticker = CoinbaseTicker::from_client(http.clone(), product_id.clone());
        let feed = format!("ticks:coinbase:{product_id}");
        let source = TickSource::new(ticker, |record: &(String, f64), poll_secs| {
            let (product_id, price) = record;
            vec![Tick {
                product_id: product_id.clone(),
                // The ticker response carries a timestamp, but the adapter
                // decodes only the price — so the poll second is what this
                // collector can honestly attribute the reading to.
                observed_at: poll_secs,
                price: *price,
                confidence: None,
            }]
        });
        let sinks: Vec<Box<dyn Sink<Tick>>> = vec![Box::new(StoreSink::new(
            pool.clone(),
            feed.clone(),
            TickWriter::new(SOURCE),
        ))];
        feeds.push((feed, run(source, sinks, run_cfg.clone())));
    }
    run_all(feeds).await
}
