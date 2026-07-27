//! Run-once schema migrator: create the framework `feed_cursors` table and the
//! survey's `cex_prices` table, then exit. The compose stack runs this as a
//! one-shot service gating the feeds (docs/data-feeds.md §5); it is idempotent,
//! so a re-run against a provisioned database is a no-op.

use dropset_fx_survey::{config::Config, store};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let cfg = Config::from_env()?;
    let pool = dropset_feeds::connect(&cfg.database_url).await?;
    store::migrate(&pool).await?;
    tracing::info!("fx-survey migrations applied");
    Ok(())
}
