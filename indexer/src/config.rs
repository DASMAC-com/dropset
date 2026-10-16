//! Environment-driven configuration, so the same binary runs against
//! localnet or a deployed cluster.
//!
//! The cluster is required (`DROPSET_CLUSTER`, `localnet` or `mainnet`) and
//! fixes the program id; there is deliberately no default, since a mainnet
//! indexer that fell back to the localnet id would trust events from a
//! program anyone can deploy. `DROPSET_PROGRAM_ID` may override the id, but
//! never with the localnet one off localnet.

use dropset_sdk::Deployment;
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
        let cluster = std::env::var("DROPSET_CLUSTER")
            .map_err(|_| anyhow::anyhow!("DROPSET_CLUSTER is required (localnet or mainnet)"))?;
        let program_id = program_id(
            cluster.parse().map_err(anyhow::Error::msg)?,
            std::env::var("DROPSET_PROGRAM_ID").ok().as_deref(),
        )?;
        Ok(Self {
            database_url,
            rpc_url: env_or("RPC_URL", "http://127.0.0.1:8899"),
            program_id,
            poll_interval_ms: env_or("POLL_INTERVAL_MS", "1000").parse().unwrap_or(1000),
            api_bind: env_or("API_BIND", "0.0.0.0:8080"),
            // Floored here rather than at each use site: this one value sizes
            // both the ingest page and the aggregator's fold batch, and a `0`
            // is silently fatal to the latter — `run_once` would fold nothing,
            // forever, while ingest kept writing legs that never became takes.
            signature_batch_limit: env_or("SIGNATURE_BATCH_LIMIT", "200")
                .parse()
                .unwrap_or(200)
                .max(1),
        })
    }
}

/// The program id to index on `deployment`: the cluster's own, or the
/// `DROPSET_PROGRAM_ID` override, refused when it is the localnet id off
/// localnet.
fn program_id(deployment: Deployment, override_id: Option<&str>) -> anyhow::Result<Pubkey> {
    let Some(raw) = override_id else {
        return Ok(deployment.program_id());
    };
    Ok(deployment.check_program_id(Pubkey::from_str(raw)?)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use dropset_sdk::{DROPSET_ID, DROPSET_MAINNET_ID};

    #[test]
    fn the_cluster_fixes_the_id_and_the_localnet_override_is_refused_off_localnet() {
        assert_eq!(program_id(Deployment::Localnet, None).unwrap(), DROPSET_ID);
        assert_eq!(
            program_id(Deployment::Mainnet, None).unwrap(),
            DROPSET_MAINNET_ID
        );
        let localnet = DROPSET_ID.to_string();
        assert!(program_id(Deployment::Mainnet, Some(&localnet)).is_err());
        assert_eq!(
            program_id(Deployment::Localnet, Some(&localnet)).unwrap(),
            DROPSET_ID
        );
        let fresh = Pubkey::new_unique();
        assert_eq!(
            program_id(Deployment::Mainnet, Some(&fresh.to_string())).unwrap(),
            fresh
        );
    }
}
