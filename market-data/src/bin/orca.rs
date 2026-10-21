//! The Orca Whirlpool spot-tick collector: read each roster pool's price over
//! JSON-RPC and persist it into `spot_ticks` (docs/data-feeds.md §9).
//!
//! The on-chain basis tape. It is the price Jupiter routes through, which is
//! why the estimator offers it beside the Coinbase candle — as an untrusted
//! tape that corroborates, never one believable alone (see
//! `dropset_feeds::venues::orca` for why a pool price cannot be).
//!
//! **The roster pins each pool by address**: `PRODUCT=POOL_ADDRESS`, through
//! the same `PRODUCT_IDS` grammar every collector reads. Here the pin is not an
//! optional spelling override but the whole entry — a pool cannot be derived
//! from its pair without trusting whoever created the newest pool for it — so
//! an unpinned entry fails startup.
//!
//! **RPC goes through an ordered endpoint list**: the keyed primary from the
//! `helius/rpc-url` secret when one is configured, then `BASE_URL` (the public
//! mainnet-beta endpoint by default). With no secret configured the list is the
//! public endpoint alone, so the collector runs keyless. The collector reads
//! its own secret rather than the TUI's `DROPSET_MAINNET_RPC_URL`: the two run
//! independently, and the TUI's variable deliberately has no fallback.

use anyhow::{anyhow, Context, Result};
use dropset_feeds::{
    connect, run,
    venues::{OrcaSpot, RpcFailover},
    RunConfig, Sink, StoreSink,
};
use dropset_market_data::{
    fx::optional_secret,
    instruments::register as register_instruments,
    parked_mirror::mirror as mirror_parked_sources,
    roster::{roster_from_env, RosterEntry},
    supervise::run_all,
    tick_store::SOURCE_ORCA,
    ticks::{Tick, TickConfig, TickDefaults, TickSource, TickWriter},
};
use solana_pubkey::Pubkey;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

/// The canonical name of the keyed RPC endpoint. The key is part of the URL, so
/// the whole URL is the secret: `HELIUS_RPC_URL` in the environment,
/// `op://<vault>/helius/rpc-url` in the local enclave.
const RPC_SECRET_NAME: &str = "helius/rpc-url";

/// The tGBP-USDC Whirlpool — the only Orca pool for the pair, holding about
/// $220k on 2026-10-21, with tGBP as its token A and USDC as token B.
const DEFAULT_PRODUCTS: &str = "TGBP-USDC=9aJrewHLYMjXiZK5u2UU4aWi4R3jFGtoKJJo4AaHYyjt";

/// The quote legs a pool may be priced in, and their mainnet mints — which is
/// what lets the adapter check a pinned pool's orientation rather than assume
/// it. A roster entry quoting anything else fails startup.
const QUOTE_MINTS: &[(&str, &str)] = &[("USDC", "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v")];

const DEFAULTS: TickDefaults = TickDefaults {
    base_url: dropset_feeds::venues::orca::PUBLIC_MAINNET_RPC,
    poll_interval_secs: 15,
};

/// One roster entry resolved: the product stored, the pool read, and the mint
/// its price is quoted in.
#[derive(Debug, PartialEq, Eq)]
struct OrcaPool {
    product_id: String,
    pool: Pubkey,
    quote_mint: Pubkey,
}

/// Resolve every roster entry to its pinned pool and its quote leg's mint.
fn orca_pools(entries: &[RosterEntry]) -> Result<Vec<OrcaPool>> {
    let mut out: Vec<OrcaPool> = Vec::with_capacity(entries.len());
    for entry in entries {
        let address = entry.venue_symbol.as_deref().ok_or_else(|| {
            anyhow!(
                "roster entry {:?} names no pool — an Orca entry is \
                 `PRODUCT=POOL_ADDRESS`, since a pool cannot be derived from \
                 its pair",
                entry.product_id
            )
        })?;
        let pool = Pubkey::from_str(address).with_context(|| {
            format!(
                "roster entry {:?} pins {address:?}, which is not a pool address",
                entry.product_id
            )
        })?;
        if let Some(clash) = out.iter().find(|p| p.pool == pool) {
            return Err(anyhow!(
                "{:?} and {:?} pin the same pool; one pool prices one pair",
                clash.product_id,
                entry.product_id
            ));
        }
        let quote = entry
            .product_id
            .split_once('-')
            .map(|(_, quote)| quote)
            .unwrap_or_default();
        let quote_mint = QUOTE_MINTS
            .iter()
            .find(|(leg, _)| *leg == quote)
            .map(|(_, mint)| Pubkey::from_str(mint))
            .transpose()?
            .ok_or_else(|| {
                anyhow!(
                    "roster entry {:?} quotes {quote:?}, which has no known mint",
                    entry.product_id
                )
            })?;
        out.push(OrcaPool {
            product_id: entry.product_id.clone(),
            pool,
            quote_mint,
        });
    }
    Ok(out)
}

/// The endpoint list, primary first, labelled for logging — never by URL.
fn endpoints(primary: Option<String>, fallback: String) -> Vec<(String, String)> {
    let mut out = Vec::with_capacity(2);
    if let Some(url) = primary {
        out.push(("helius".to_string(), url));
    }
    out.push(("public".to_string(), fallback));
    out
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let cfg = TickConfig::from_env(&DEFAULTS)?;
    let pools = orca_pools(&roster_from_env(DEFAULT_PRODUCTS)?)?;
    let endpoints = endpoints(optional_secret(RPC_SECRET_NAME)?, cfg.base_url.clone());
    let labels: Vec<&str> = endpoints.iter().map(|(label, _)| label.as_str()).collect();
    tracing::info!(endpoints = %labels.join(","), "orca RPC endpoint list");
    // One list shared by every pool, so the roster draws on one endpoint
    // budget and logs one active endpoint.
    let rpc = Arc::new(RpcFailover::new(endpoints)?);

    let pool = connect(&cfg.database_url).await?;
    dropset_db_schema::require_schema(&pool).await?;
    let ids: Vec<String> = pools.iter().map(|p| p.product_id.clone()).collect();
    register_instruments(&pool, SOURCE_ORCA, &ids).await?;
    mirror_parked_sources(&pool).await?;
    tracing::info!(
        products = %ids.join(","),
        poll_secs = cfg.poll_interval_secs,
        "orca tick collector starting"
    );

    let run_cfg = RunConfig {
        poll_interval: Duration::from_secs(cfg.poll_interval_secs),
        ..RunConfig::default()
    };
    let mut feeds = Vec::with_capacity(pools.len());
    for entry in pools {
        let spot = OrcaSpot::new(
            rpc.clone(),
            entry.product_id.clone(),
            entry.pool,
            entry.quote_mint,
        );
        let feed = format!("ticks:orca:{}", entry.product_id);
        let source = TickSource::new(spot, |record: &(String, f64), poll_secs| {
            let (product_id, price) = record;
            vec![Tick {
                product_id: product_id.clone(),
                // A pool has no publish time of its own beyond the slot it was
                // read at, so the poll second is the honest attribution.
                observed_at: poll_secs,
                price: *price,
                confidence: None,
            }]
        });
        let sinks: Vec<Box<dyn Sink<Tick>>> = vec![Box::new(StoreSink::new(
            pool.clone(),
            feed.clone(),
            TickWriter::new(SOURCE_ORCA),
        ))];
        feeds.push((feed, run(source, sinks, run_cfg.clone())));
    }
    run_all(feeds).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use dropset_market_data::roster::parse_roster;

    #[test]
    fn the_default_roster_resolves_to_the_pinned_pool_quoted_in_usdc() {
        let pools = orca_pools(&parse_roster(DEFAULT_PRODUCTS).unwrap()).unwrap();
        assert_eq!(
            pools,
            vec![OrcaPool {
                product_id: "TGBP-USDC".to_string(),
                pool: Pubkey::from_str("9aJrewHLYMjXiZK5u2UU4aWi4R3jFGtoKJJo4AaHYyjt").unwrap(),
                quote_mint: Pubkey::from_str(QUOTE_MINTS[0].1).unwrap(),
            }]
        );
    }

    #[test]
    fn an_unpinned_entry_fails_startup() {
        let err = orca_pools(&parse_roster("TGBP-USDC").unwrap()).unwrap_err();
        assert!(err.to_string().contains("names no pool"), "{err}");
    }

    #[test]
    fn a_pin_that_is_not_an_address_fails_startup() {
        assert!(orca_pools(&parse_roster("TGBP-USDC=not-a-pool").unwrap()).is_err());
    }

    #[test]
    fn a_quote_leg_with_no_known_mint_fails_startup() {
        let spec = "TGBP-USDT=9aJrewHLYMjXiZK5u2UU4aWi4R3jFGtoKJJo4AaHYyjt";
        let err = orca_pools(&parse_roster(spec).unwrap()).unwrap_err();
        assert!(err.to_string().contains("no known mint"), "{err}");
    }

    #[test]
    fn two_entries_pinning_one_pool_fail_startup() {
        let spec = "TGBP-USDC=9aJrewHLYMjXiZK5u2UU4aWi4R3jFGtoKJJo4AaHYyjt,\
                    EURC-USDC=9aJrewHLYMjXiZK5u2UU4aWi4R3jFGtoKJJo4AaHYyjt";
        let err = orca_pools(&parse_roster(spec).unwrap()).unwrap_err();
        assert!(err.to_string().contains("same pool"), "{err}");
    }

    #[test]
    fn the_keyed_primary_comes_first_and_the_public_endpoint_always_last() {
        let public = "https://public.example".to_string();
        let both = endpoints(Some("https://keyed.example".to_string()), public.clone());
        let labels: Vec<&str> = both.iter().map(|(l, _)| l.as_str()).collect();
        assert_eq!(labels, vec!["helius", "public"]);
        // No secret configured: the public endpoint alone, so nothing breaks.
        assert_eq!(
            endpoints(None, public.clone()),
            vec![("public".to_string(), public)]
        );
    }
}
