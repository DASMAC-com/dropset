//! The `/v1` REST server over the indexer store.

use dropset_indexer::{api, config::Config, store::Store};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let cfg = Config::from_env()?;
    let store = Store::connect(&cfg.database_url).await?;
    let app = api::router(store);

    let listener = tokio::net::TcpListener::bind(&cfg.api_bind).await?;
    tracing::info!(bind = %cfg.api_bind, "api listening");
    axum::serve(listener, app).await?;
    Ok(())
}
