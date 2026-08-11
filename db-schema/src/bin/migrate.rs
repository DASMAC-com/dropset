//! Run-once migration runner for the shared `dropset` database: apply every
//! outstanding migration, log where the schema landed, and exit.
//!
//! This is the only thing in the system that issues DDL (docs/data-feeds.md
//! §8). The localnet compose runs it as a one-shot service that dependent
//! services gate on (`service_completed_successfully`); the AWS deploy runs it
//! before flashing binaries. It is idempotent, so restarting the stack
//! re-runs it harmlessly, and it doubles as the dev reset story — point it at
//! a fresh database and the full history replays.

use dropset_db_schema::{connect, expected_version, migrate};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let url =
        std::env::var("DATABASE_URL").map_err(|_| anyhow::anyhow!("DATABASE_URL is required"))?;
    let pool = connect(&url).await?;
    migrate(&pool).await?;
    tracing::info!(
        version = expected_version(),
        "dropset schema migrations applied"
    );
    Ok(())
}
