//! `dropset-maker-bot` entrypoint.
//!
//! Default mode supervises every demo market live against a localnet validator:
//! discover the markets, fund the leader, and drive the tick loop, one batched
//! feed poll shared across them. `--dry-run` instead polls the tiered feeds
//! once and prints the reference each market *would* stamp — the wiring check
//! for feed credentials, with no validator and no writes. Pass `--drop <tier>`
//! (repeatable: `coingecko`, `cmc`, `fx`) in a dry run to suppress a tier and
//! watch the cascade fall through to the next one.
//!
//! Flags:
//!   --rpc <url>            RPC endpoint (default http://127.0.0.1:8899)
//!   --ws <url>             PubSub websocket (default: derived from --rpc)
//!   --leader-key <path>    leader/quote-authority keypair (default keys/EEEE.json)
//!   --market <symbol>      quote only this market (repeatable); default: all
//!   --dry-run              poll feeds and print the intended quotes, then exit
//!   --drop <tier>          dry-run only: suppress coingecko | cmc | fx

use anyhow::{anyhow, Context, Result};
use dropset_fair_value::{FairValueEngine, Reading};
use dropset_localnet_support::ws_url_from_rpc;
use dropset_maker_bot::config::{
    BotConfig, MarketConfig, DEFAULT_LEADER_KEY, MARKETS, QUOTE_KEYPAIR_FILE, USDC_COINGECKO_ID,
};
use dropset_maker_bot::context::Context as BotContext;
use dropset_maker_bot::model::fair_mid::build_legs;
use dropset_maker_bot::model::feeds::Feeds;
use dropset_maker_bot::{chain, fills, tasks};
use solana_pubkey::Pubkey;
use solana_signer::Signer;
use std::time::Duration;

/// Lamports per SOL.
const LAMPORTS_PER_SOL: u64 = 1_000_000_000;
/// Below this leader balance, airdrop on startup (localnet).
const MIN_LEADER_LAMPORTS: u64 = LAMPORTS_PER_SOL / 2;
/// Airdrop size when topping up the leader.
const AIRDROP_LAMPORTS: u64 = 2 * LAMPORTS_PER_SOL;

struct Args {
    leader_key: String,
    dry_run: bool,
    /// Tiers to suppress in a dry run (to exercise the cascade).
    drop: Vec<String>,
    /// Symbols to restrict this instance to (empty = every market). The TUI
    /// runs one instance per market by passing a single `--market`, so an
    /// operator can start / stop each market's bot independently.
    markets: Vec<String>,
}

impl Args {
    /// The roster this instance quotes: every [`MarketConfig`] whose symbol was
    /// named with `--market` (case-insensitive), or all of them when none was.
    fn selected(&self) -> Vec<&'static MarketConfig> {
        MARKETS
            .iter()
            .filter(|m| {
                self.markets.is_empty()
                    || self
                        .markets
                        .iter()
                        .any(|s| s.eq_ignore_ascii_case(m.symbol))
            })
            .collect()
    }
}

fn main() -> Result<()> {
    let mut cfg = BotConfig::default();
    let args = parse_args(&mut cfg);
    if args.dry_run {
        dry_run(&cfg, &args)
    } else {
        run_live(&cfg, &args)
    }
}

/// Parse flags, mutating `cfg` and returning the run options.
fn parse_args(cfg: &mut BotConfig) -> Args {
    let mut leader_key = DEFAULT_LEADER_KEY.to_string();
    let mut dry_run = false;
    let mut drop = Vec::new();
    let mut markets = Vec::new();
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--rpc" => {
                if let Some(url) = it.next() {
                    cfg.rpc_url = url;
                }
            }
            "--ws" => {
                if let Some(url) = it.next() {
                    cfg.ws_url = Some(url);
                }
            }
            "--leader-key" => {
                if let Some(path) = it.next() {
                    leader_key = path;
                }
            }
            "--market" => {
                if let Some(symbol) = it.next() {
                    markets.push(symbol);
                }
            }
            "--drop" => {
                if let Some(tier) = it.next() {
                    drop.push(tier);
                }
            }
            "--dry-run" => dry_run = true,
            _ => {}
        }
    }
    Args {
        leader_key,
        dry_run,
        drop,
        markets,
    }
}

/// Discover the markets, fund the leader, and run the supervisor loop.
fn run_live(cfg: &BotConfig, args: &Args) -> Result<()> {
    let client = chain::rpc(&cfg.rpc_url);
    // Guard before funding or signing anything: the airdrop needs the localnet
    // faucet and the leader key holds no authority on a public cluster, so an
    // off-localnet --rpc is always a misconfiguration — fail fast rather than
    // emit doomed sends.
    chain::assert_localnet(&client)?;
    let leader = solana_keypair::read_keypair_file(&args.leader_key)
        .map_err(|e| anyhow!("read leader key {}: {e}", args.leader_key))?;

    // The leader pays for its own quoting txns; top it up on localnet.
    let balance = client
        .get_balance(&leader.pubkey())
        .context("leader balance")?;
    if balance < MIN_LEADER_LAMPORTS {
        println!(
            "funding leader {} ({} SOL)…",
            leader.pubkey(),
            AIRDROP_LAMPORTS / LAMPORTS_PER_SOL
        );
        chain::airdrop(&client, &leader.pubkey(), AIRDROP_LAMPORTS)?;
    }

    // Discover every on-chain market once, then match the roster against it by
    // base mint (quote is always USDC). The roster is narrowed to any
    // `--market` symbols so one instance can quote a single market.
    let discovered = chain::discover_markets(&client)?;
    let quote_mint = mint_pubkey(QUOTE_KEYPAIR_FILE)?;
    let roster = args.selected();
    let mut contexts = Vec::new();
    for &market in &roster {
        let base_mint = match mint_pubkey(market.base_keypair_file) {
            Ok(pk) => pk,
            Err(e) => {
                eprintln!("[{}] skipped — {e}", market.symbol);
                continue;
            }
        };
        let Some(addrs) = discovered
            .iter()
            .find(|m| m.base_mint == base_mint && m.quote_mint == quote_mint)
        else {
            eprintln!(
                "[{}] no on-chain market for {base_mint}/USDC — bootstrap it first",
                market.symbol
            );
            continue;
        };
        println!(
            "[{}] market {} ({})",
            market.symbol, addrs.market, base_mint
        );
        contexts.push(BotContext::new(
            chain::rpc(&cfg.rpc_url),
            leader.insecure_clone(),
            cfg.vault_idx,
            addrs.clone(),
            *market,
            cfg.fair_value,
        ));
    }
    if contexts.is_empty() {
        return Err(anyhow!(
            "no demo markets found on-chain — is the localnet bootstrapped?"
        ));
    }

    // One fill subscription covers every market the leader quotes; the
    // supervisor routes each fill to its market by `event.market`.
    let ws_url = cfg
        .ws_url
        .clone()
        .unwrap_or_else(|| ws_url_from_rpc(&cfg.rpc_url));
    let fills = fills::spawn(ws_url, cfg.rpc_url.clone(), leader.pubkey());

    let feeds = Feeds::new(cfg.feeds.clone());
    tasks::run_supervisor(feeds, cfg.clone(), contexts, fills)
}

/// Load a checked-in mint keypair and return its public key.
fn mint_pubkey(keypair_file: &str) -> Result<Pubkey> {
    solana_keypair::read_keypair_file(keypair_file)
        .map(|kp| kp.pubkey())
        .map_err(|e| anyhow!("read mint key {keypair_file}: {e}"))
}

/// Poll the tiered feeds once and print the reference each market would stamp.
/// No validator and no writes — a credentials/cascade check. `--drop` suppresses
/// a tier so the cascade to the next one is observable.
fn dry_run(cfg: &BotConfig, args: &Args) -> Result<()> {
    let feeds = Feeds::new(cfg.feeds.clone());
    let drop = |tier: &str| args.drop.iter().any(|d| d == tier);

    let roster = args.selected();
    let mut cg_ids: Vec<&str> = roster.iter().map(|m| m.coingecko_id).collect();
    // The USDC/USD common-mode leg rides the same batched CoinGecko call.
    cg_ids.push(USDC_COINGECKO_ID);
    let cmc_ids: Vec<u32> = roster.iter().filter_map(|m| m.coinmarketcap_id).collect();
    let mut currencies: Vec<&str> = roster.iter().map(|m| m.currency).collect();
    currencies.sort_unstable();
    currencies.dedup();

    let cg = if drop("coingecko") {
        Default::default()
    } else {
        feeds.poll_coingecko(&cg_ids).unwrap_or_default()
    };
    let cmc = if drop("cmc") || !feeds.coinmarketcap_enabled() {
        Default::default()
    } else {
        feeds.poll_coinmarketcap(&cmc_ids).unwrap_or_default()
    };
    let fx = if drop("fx") {
        Default::default()
    } else {
        feeds.poll_frankfurter(&currencies).unwrap_or_default()
    };

    println!(
        "Tiers live: coingecko {} ids, coinmarketcap {} ids{}, fx {} currencies",
        cg.len(),
        cmc.len(),
        if feeds.coinmarketcap_enabled() {
            ""
        } else {
            " (no CMC_API_KEY)"
        },
        fx.len()
    );
    if !args.drop.is_empty() {
        println!("Suppressed tiers: {}", args.drop.join(", "));
    }
    println!("\n  market      mid (USDC)    anchor         health     basis");

    let now = Duration::from_secs(0);
    let q = |v: Option<f64>| v.map(|v| Reading::new(v, now));
    // USDC/USD common-mode leg, shared by every market.
    let usdc_q = q(cg.get(USDC_COINGECKO_ID).copied());
    for &m in &roster {
        let cg_q = q(cg.get(m.coingecko_id).copied());
        let cmc_q = q(m.coinmarketcap_id.and_then(|id| cmc.get(&id)).copied());
        let fx_q = q(fx.get(m.currency).copied());
        // Frankfurter USD/`<ccy>` is the FX anchor; CoinGecko/CMC token-USD is
        // the (demoted) crypto basis leg — a fresh engine per row (no history).
        let legs = build_legs(fx_q, cg_q.or(cmc_q), usdc_q, m.static_usd);
        let mut engine = FairValueEngine::new(cfg.fair_value);
        let fair = engine.compose(legs, now, false);
        let anchor = format!("{:?}", fair.anchor);
        let mid = fair.fair.map_or("—".to_string(), |v| format!("{v:.8}"));
        let basis = fair.basis.map_or("—".to_string(), |b| {
            format!("{b:.4}{}", if fair.basis_breach { " BREACH" } else { "" })
        });
        println!(
            "  {:<10}  {:>12}  {:<13}  {:<9?}  {}",
            m.symbol, mid, anchor, fair.health, basis
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(markets: &[&str]) -> Args {
        Args {
            leader_key: DEFAULT_LEADER_KEY.to_string(),
            dry_run: false,
            drop: Vec::new(),
            markets: markets.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn no_market_flag_selects_the_whole_roster() {
        assert_eq!(args(&[]).selected().len(), MARKETS.len());
    }

    #[test]
    fn market_flag_narrows_to_the_named_markets_case_insensitively() {
        let selected = args(&["eurc", "MXNE"]).selected();
        let symbols: Vec<&str> = selected.iter().map(|m| m.symbol).collect();
        assert_eq!(symbols, ["EURC", "MXNe"]);
    }

    #[test]
    fn an_unknown_market_symbol_selects_nothing() {
        assert!(args(&["nope"]).selected().is_empty());
    }
}
