//! `dropset-taker-bot` entrypoint.
//!
//! Default mode runs the bot live against a localnet market: discover the
//! market, fund the taker, and drive the tick loop, submitting the stochastic
//! flow's orders as `swap`s. `--dry-run` instead samples the flow for a fixed
//! number of ticks and prints the orders it *would* submit — the wiring check
//! for the flow parameters, with no validator and no writes.
//!
//! Flags:
//!   --rpc <url>              RPC endpoint (default http://127.0.0.1:8899)
//!   --market-address <pk>    target this exact market PDA (default: first found)
//!   --taker-key <path>       taker keypair (default keys/FFFF.json)
//!   --mint-authority <path>  mock-mint authority (default keys/BBBB.json)
//!   --seed <u64>             seed the flow RNG for a reproducible run
//!   --dry-run                sample the flow and print intended swaps, then exit
//!   --ticks <n>              dry-run sample length (default 20)

use anyhow::{anyhow, Result};
use dropset_sdk::matching::SwapSide;
use dropset_taker_bot::config::{BotConfig, DEFAULT_MINT_AUTHORITY_KEY, DEFAULT_TAKER_KEY};
use dropset_taker_bot::context::Context;
use dropset_taker_bot::model::{Flow, Regime};
use dropset_taker_bot::{chain, tasks};
use solana_pubkey::Pubkey;

struct Args {
    taker_key: String,
    mint_authority_key: String,
    /// The exact market PDA to trade, when the caller scopes this instance to
    /// one market (the TUI passes the selected market so a per-market taker
    /// hits the right book). `None` ⇒ discover the first market on-chain, the
    /// single-market / container default.
    market_address: Option<Pubkey>,
    dry_run: bool,
    dry_run_ticks: u32,
}

fn main() -> Result<()> {
    let mut cfg = BotConfig::default();
    let args = parse_args(&mut cfg);
    if args.dry_run {
        dry_run(&cfg, args.dry_run_ticks)
    } else {
        run_live(&cfg, &args)
    }
}

/// Parse flags, mutating `cfg` and returning the run options.
fn parse_args(cfg: &mut BotConfig) -> Args {
    let mut taker_key = DEFAULT_TAKER_KEY.to_string();
    let mut mint_authority_key = DEFAULT_MINT_AUTHORITY_KEY.to_string();
    let mut market_address = None;
    let mut dry_run = false;
    let mut dry_run_ticks = 20u32;
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--rpc" => {
                if let Some(url) = it.next() {
                    cfg.rpc_url = url;
                }
            }
            "--market-address" => {
                if let Some(pk) = it.next().and_then(|s| s.parse().ok()) {
                    market_address = Some(pk);
                }
            }
            "--taker-key" => {
                if let Some(path) = it.next() {
                    taker_key = path;
                }
            }
            "--mint-authority" => {
                if let Some(path) = it.next() {
                    mint_authority_key = path;
                }
            }
            "--seed" => {
                if let Some(seed) = it.next().and_then(|s| s.parse().ok()) {
                    cfg.flow.seed = Some(seed);
                }
            }
            "--ticks" => {
                if let Some(n) = it.next().and_then(|s| s.parse().ok()) {
                    dry_run_ticks = n;
                }
            }
            "--dry-run" => dry_run = true,
            _ => {}
        }
    }
    Args {
        taker_key,
        mint_authority_key,
        market_address,
        dry_run,
        dry_run_ticks,
    }
}

/// Discover the market, fund the taker, and run the tick loop.
fn run_live(cfg: &BotConfig, args: &Args) -> Result<()> {
    let client = chain::rpc(&cfg.rpc_url);
    // Guard before reading the mint-authority wallet or sending anything: this
    // bot mints inventory and signs swaps with local keys, so it must only run
    // against a localnet validator, never a public cluster.
    chain::assert_localnet(&client)?;
    let taker = read_key(&args.taker_key, "taker")?;
    let mint_authority = read_key(&expand_tilde(&args.mint_authority_key), "mint authority")?;

    let market = chain::discover_market(&client, args.market_address)?;
    println!(
        "discovered market {} ({}/{})",
        market.market, market.base_mint, market.quote_mint
    );
    let flow = Flow::new(cfg.flow.clone());
    let ctx = Context::new(client, taker, mint_authority, market, flow);
    tasks::run(ctx, cfg.clone())
}

/// Sample the flow for `ticks` ticks and print the orders it would submit,
/// then a summary — no validator, no writes.
fn dry_run(cfg: &BotConfig, ticks: u32) -> Result<()> {
    let mut flow = Flow::new(cfg.flow.clone());
    println!(
        "dry run: sampling {ticks} ticks{}",
        match cfg.flow.seed {
            Some(s) => format!(" (seed {s})"),
            None => " (random seed)".to_string(),
        }
    );

    let mut orders = 0u32;
    let mut buys = 0u32;
    let mut burst_ticks = 0u32;
    let mut notional_sum = 0.0;
    for t in 0..ticks {
        let tick_orders = flow.tick();
        if matches!(flow.regime(), Regime::Burst) {
            burst_ticks += 1;
        }
        for order in &tick_orders {
            orders += 1;
            notional_sum += order.notional;
            if order.side == SwapSide::Buy {
                buys += 1;
            }
            println!(
                "  tick {t:>3} [{:?}]: {}",
                flow.regime(),
                tasks::describe(order)
            );
        }
    }

    println!("\nSummary over {ticks} ticks:");
    println!("  orders:      {orders}");
    println!("  burst ticks: {burst_ticks} / {ticks}");
    if orders > 0 {
        let sells = orders - buys;
        println!(
            "  buy / sell:  {buys} / {sells} ({:.0}% buys)",
            100.0 * buys as f64 / orders as f64
        );
        println!("  mean size:   {:.2} quote", notional_sum / orders as f64);
    }
    Ok(())
}

/// Read a keypair from `path`, labeling failures with `role`.
fn read_key(path: &str, role: &str) -> Result<solana_keypair::Keypair> {
    solana_keypair::read_keypair_file(path).map_err(|e| anyhow!("read {role} key {path}: {e}"))
}

/// Expand a leading `~/` to the user's home directory, leaving other paths
/// untouched. The mock-mint authority defaults to the repo key `keys/BBBB.json`
/// (no tilde), so this only matters for a user-supplied `~`-relative override.
fn expand_tilde(path: &str) -> String {
    match path.strip_prefix("~/") {
        Some(rest) => match std::env::var_os("HOME") {
            Some(home) => format!("{}/{}", home.to_string_lossy(), rest),
            None => path.to_string(),
        },
        None => path.to_string(),
    }
}
