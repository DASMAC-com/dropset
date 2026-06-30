//! Environment-driven configuration. Nothing is hard-coded: the program
//! id defaults to the SDK's `DROPSET_ID` but every value is overridable so
//! the same binary runs against localnet or a deployed cluster.

use solana_pubkey::Pubkey;
use std::str::FromStr;

#[derive(Clone, Debug)]
pub struct Config {
    pub database_url: String,
    pub rpc_url: String,
    pub program_id: Pubkey,
    pub poll_interval_ms: u64,
    pub api_bind: String,
    pub signature_batch_limit: usize,
}

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

impl Config {
    pub fn from_env() -> anyhow::Result<Self> {
        let database_url = std::env::var("DATABASE_URL")
            .map_err(|_| anyhow::anyhow!("DATABASE_URL is required"))?;
        let program_id = Pubkey::from_str(&env_or(
            "DROPSET_PROGRAM_ID",
            &dropset_sdk::DROPSET_ID.to_string(),
        ))?;
        Ok(Self {
            database_url,
            rpc_url: env_or("RPC_URL", "http://127.0.0.1:8899"),
            program_id,
            poll_interval_ms: env_or("POLL_INTERVAL_MS", "1000").parse().unwrap_or(1000),
            api_bind: env_or("API_BIND", "0.0.0.0:8080"),
            signature_batch_limit: env_or("SIGNATURE_BATCH_LIMIT", "200")
                .parse()
                .unwrap_or(200),
        })
    }
}
