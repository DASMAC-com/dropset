//! `dropset-maker-bot` entrypoint.
//!
//! Default mode runs the bot live against a localnet market: discover the
//! market, fund the leader, and drive the tick loop. `--dry-run` instead polls
//! the feeds once and prints the reference and ladder it *would* stamp — the
//! wiring check for feed credentials, with no validator and no writes.
//!
//! Flags:
//!   --rpc <url>            RPC endpoint (default http://127.0.0.1:8899)
//!   --ws <url>             PubSub websocket (default: derived from --rpc)
//!   --leader-key <path>    leader/quote-authority keypair (default keys/EEEE.json)
//!   --aerodrome <net>:<pool>  enable the Aerodrome feed (off by default)
//!   --dry-run              poll feeds and print the intended quote, then exit

use anyhow::{anyhow, Context, Result};
use dropset_maker_bot::config::{ws_url_from_rpc, AerodromeConfig, BotConfig, DEFAULT_LEADER_KEY};
use dropset_maker_bot::context::Context as BotContext;
use dropset_maker_bot::model::fair_mid::{compose, Quote};
use dropset_maker_bot::model::feeds::Feeds;
use dropset_maker_bot::model::inventory::Inventory;
use dropset_maker_bot::model::{killswitch, ladder, skew};
use dropset_maker_bot::{chain, fills, tasks};
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
}

fn main() -> Result<()> {
    let mut cfg = BotConfig::default();
    let args = parse_args(&mut cfg);
    if args.dry_run {
        dry_run(&cfg)
    } else {
        run_live(&cfg, &args)
    }
}

/// Parse flags, mutating `cfg` and returning the run options.
fn parse_args(cfg: &mut BotConfig) -> Args {
    let mut leader_key = DEFAULT_LEADER_KEY.to_string();
    let mut dry_run = false;
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
            "--aerodrome" => {
                if let Some((network, pool)) = it.next().and_then(|s| split_pair(&s)) {
                    cfg.feeds.aerodrome = Some(AerodromeConfig {
                        network,
                        pool,
                        poll: Duration::from_secs(10),
                    });
                }
            }
            "--dry-run" => dry_run = true,
            _ => {}
        }
    }
    Args {
        leader_key,
        dry_run,
    }
}

/// Split a `network:pool` argument into its owned parts.
fn split_pair(spec: &str) -> Option<(String, String)> {
    spec.split_once(':')
        .map(|(n, p)| (n.to_string(), p.to_string()))
}

/// Discover the market, fund the leader, and run the tick loop.
fn run_live(cfg: &BotConfig, args: &Args) -> Result<()> {
    let client = chain::rpc(&cfg.rpc_url);
    // Guard before funding or signing anything: this bot airdrops itself and
    // signs quoting transactions with the leader key, so it must only run
    // against a localnet validator, never a public cluster.
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

    let market = chain::discover_market(&client)?;
    println!(
        "discovered market {} ({}/{})",
        market.market, market.base_mint, market.quote_mint
    );

    // Start the fill-event subscription before the tick loop so fills landing
    // during warm-up aren't missed. The thread reconnects on its own; a failed
    // subscription degrades to the per-tick inventory-diff fallback.
    let ws_url = cfg
        .ws_url
        .clone()
        .unwrap_or_else(|| ws_url_from_rpc(&cfg.rpc_url));
    let fills = fills::spawn(ws_url, cfg.rpc_url.clone(), leader.pubkey());

    let ctx = BotContext::new(client, leader, cfg.vault_idx, market);
    let ctx = match fills {
        Some(rx) => ctx.with_fills(rx),
        None => ctx,
    };
    let feeds = Feeds::new(cfg.feeds.clone());
    tasks::run(ctx, feeds, cfg.clone())
}

/// Poll the feeds once, compose the reference, and print the intended quote.
fn dry_run(cfg: &BotConfig) -> Result<()> {
    let feeds = Feeds::new(cfg.feeds.clone());
    let now = Duration::from_secs(0);

    let cg = feeds.poll_coingecko();
    let fx = feeds.poll_oanda();
    let ae = if feeds.aerodrome_enabled() {
        Some(feeds.poll_aerodrome())
    } else {
        None
    };

    println!("Feed readings:");
    print_feed("  CoinGecko CADC/USD", &cg);
    print_feed("  Oanda CAD/USD     ", &fx);
    match &ae {
        Some(r) => print_feed("  Aerodrome CADC/USD", r),
        None => println!("  Aerodrome CADC/USD: disabled (pass --aerodrome <net>:<pool>)"),
    }

    let quote = |r: &Result<f64>| r.as_ref().ok().map(|&v| Quote::new(v, now));
    let fair = compose(
        quote(&cg),
        ae.as_ref().and_then(quote),
        quote(&fx),
        &cfg.kill,
    );

    println!("\nComposed reference:");
    println!("  health:     {:?}", fair.health);
    match fair.mid {
        Some(mid) => println!("  fair_mid:   {mid:.6} USDC/CADC"),
        None => {
            println!("  fair_mid:   <paused — no usable CADC source>");
            return Ok(());
        }
    }
    match fair.peg {
        Some(peg) => println!("  peg:        {peg:.4} (breach: {})", fair.peg_breach),
        None => println!("  peg:        <no fresh FX feed>"),
    }

    // Without a chain read we can't value live inventory, so assume neutral —
    // the dry run is about feeds and ladder shape, not inventory skew.
    let mid = fair.mid.unwrap();
    let neutral = Inventory {
        base_value_usd: 50.0,
        quote_value_usd: 50.0,
    };
    let skew_bps = skew::ref_skew_bps(&neutral, &cfg.strategy);
    let reference = skew::apply_skew(mid, skew_bps);
    let action = killswitch::evaluate(&fair, &neutral, &cfg.kill, false);

    println!("\nIntended quote (neutral inventory assumed):");
    println!("  reference:  {reference:.6} (skew {skew_bps:+.1} bps)");
    println!("  action:     {action:?}");
    println!("  ladder:");
    let profile = ladder::build_profile(&cfg.strategy.ladder);
    for (i, level) in cfg.strategy.ladder.iter().enumerate() {
        let bid = mid * (1.0 - level.offset_ppm as f64 / 1_000_000.0);
        let ask = reference * (1.0 + level.offset_ppm as f64 / 1_000_000.0);
        println!(
            "    L{}: ±{} ppm, {} bps, expiry {} slots  (bid {bid:.6} / ask {ask:.6})",
            i + 1,
            level.offset_ppm,
            level.size_bps,
            level.expiry_offset,
        );
    }
    // Touch the serialized form so the dry run also exercises it.
    let _bytes = ladder::to_bytes(&profile);

    Ok(())
}

fn print_feed(label: &str, result: &Result<f64>) {
    match result {
        Ok(v) => println!("{label}: {v:.6}"),
        Err(e) => println!("{label}: error — {e}"),
    }
}
