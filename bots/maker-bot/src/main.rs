//! `dropset-maker-bot` entrypoint.
//!
//! For now this drives a **dry run**: it polls the live price feeds, composes
//! the fair mid and peg, and prints the reference price and quote ladder it
//! *would* stamp — no validator, no writes. It's the wiring check for the
//! feed credentials and the composition rules before the bot drives a real
//! vault. The live tick loop (the `chain` + `tasks` modules) builds on this.

use anyhow::Result;
use dropset_maker_bot::config::{AerodromeConfig, BotConfig};
use dropset_maker_bot::model::fair_mid::{compose, Quote};
use dropset_maker_bot::model::feeds::Feeds;
use dropset_maker_bot::model::inventory::Inventory;
use dropset_maker_bot::model::{killswitch, ladder, skew};
use std::time::Duration;

fn main() -> Result<()> {
    let mut cfg = BotConfig::default();
    parse_args(&mut cfg);
    dry_run(&cfg)
}

/// Minimal flag parsing: `--aerodrome <network>:<pool>` enables the Aerodrome
/// feed for this run (off by default). Unknown flags are ignored.
fn parse_args(cfg: &mut BotConfig) {
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--aerodrome" {
            if let Some(spec) = args.next() {
                if let Some((network, pool)) = spec.split_once(':') {
                    cfg.feeds.aerodrome = Some(AerodromeConfig {
                        network: network.to_string(),
                        pool: pool.to_string(),
                        poll: Duration::from_secs(10),
                    });
                }
            }
        }
    }
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
