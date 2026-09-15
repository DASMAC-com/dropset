//! The fair-value estimator: read the store, compose one fair value per MVP
//! market, publish it into `fair_price` on the estimator's own tick.
//!
//! Thin on purpose, like every other binary here — configuration, then the
//! process. The roster mapping, the per-market engines, the tick loop and the
//! classed halts all live in [`dropset_market_data::estimator`].
//!
//! **This is a publisher, not a collector, and the difference shows up in two
//! places.** It reads two tables and writes a third rather than polling a venue;
//! and it does not run on the feeds runner, because that runner continues on any
//! source error forever — see the estimator module's failure-posture section.

use dropset_feeds::connect;
use dropset_market_data::estimator::{Estimator, EstimatorConfig, MVP_MARKETS};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let cfg = EstimatorConfig::from_env()?;
    let pool = connect(&cfg.database_url).await?;
    // DB-primary in both directions: without `cex_prices` and `spot_ticks` there
    // are no legs to compose, and without `fair_price` there is nowhere to put
    // the result. Assert the schema up front rather than failing on the first
    // publish — which, being a refused row, would classify as a permanent
    // failure and halt anyway, just later and with a worse diagnosis.
    dropset_db_schema::require_schema(&pool).await?;

    // No `register_instruments` call here, deliberately. That write declares
    // which products a **collector** polls from a venue; this process polls no
    // venue and introduces no product — every id it publishes was registered by
    // the collector that feeds it. Registering them again under a second source
    // label would put a consumer in the instruments dimension.
    Estimator::new(pool, MVP_MARKETS.to_vec(), cfg.tick_interval)?
        .run()
        .await
}
