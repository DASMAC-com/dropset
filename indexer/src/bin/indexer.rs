//! The ingest + aggregate worker: poll the cluster, decode events, persist
//! them, and fold the new fill legs into takes — then sleep and repeat.

use dropset_indexer::{aggregate, config::Config, decode, ingest::RpcPollSource, store::Store};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let cfg = Config::from_env()?;
    let store = Store::connect(&cfg.database_url).await?;
    let mut source = RpcPollSource::new(
        cfg.rpc_url.clone(),
        cfg.program_id,
        cfg.signature_batch_limit,
    );
    tracing::info!(program = %cfg.program_id, rpc = %cfg.rpc_url, "indexer starting");

    let interval = std::time::Duration::from_millis(cfg.poll_interval_ms);
    loop {
        match source.poll().await {
            Ok(txs) => {
                let mut written: u64 = 0;
                for tx in &txs {
                    let decoded = decode::decode_tx(tx);
                    if !decoded.is_empty() {
                        written += store.write_events(&decoded).await?;
                    }
                }
                let folded = aggregate::run_once(&store, cfg.signature_batch_limit as i64).await?;
                if written > 0 || folded > 0 {
                    tracing::info!(written, folded, txs = txs.len(), "indexed");
                }
            }
            Err(e) => tracing::warn!(error = %e, "poll failed; retrying"),
        }
        tokio::select! {
            _ = tokio::time::sleep(interval) => {}
            _ = tokio::signal::ctrl_c() => {
                tracing::info!("shutting down");
                break;
            }
        }
    }
    Ok(())
}
