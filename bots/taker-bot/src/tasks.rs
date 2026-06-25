//! The tick loop.
//!
//! Every tick: keep the taker funded, then advance the stochastic flow one
//! step and submit the orders it produced. Each order is sized against the
//! live book ([`chain::size_order`]) and sent as its own `swap`
//! ([`chain::send_swap`]); a failed or unfillable order is logged and skipped,
//! and the loop continues — one bad take never stalls the flow.

// cspell:word unfillable

use crate::chain;
use crate::config::BotConfig;
use crate::context::Context;
use crate::model::Order;
use anyhow::Result;
use dropset_sdk::matching::SwapSide;

/// Run the bot until interrupted. Each loop iteration is one tick; a tick
/// error is logged and the loop continues.
pub fn run(mut ctx: Context, cfg: BotConfig) -> Result<()> {
    println!(
        "taker-bot live: market {} ({}/{}) tick {:?}",
        ctx.market.market, ctx.market.base_mint, ctx.market.quote_mint, cfg.tick,
    );
    loop {
        if let Err(e) = tick(&mut ctx, &cfg) {
            eprintln!("[tick] error: {e}");
        }
        std::thread::sleep(cfg.tick);
    }
}

fn tick(ctx: &mut Context, cfg: &BotConfig) -> Result<()> {
    // 1. Keep the taker funded — top up SOL and refill either leg when low.
    chain::ensure_funded(
        &ctx.client,
        &ctx.taker,
        &ctx.mint_authority,
        &ctx.market,
        cfg.airdrop_lamports,
        cfg.min_taker_lamports,
        cfg.inventory_target_tokens,
        cfg.inventory_min_tokens,
    )?;

    // 2. Advance the flow and submit this tick's orders.
    let orders = ctx.flow.tick();
    if orders.is_empty() {
        return Ok(());
    }
    println!(
        "[tick] {} order(s) ({:?}, buy_bias {:.2})",
        orders.len(),
        ctx.flow.regime(),
        ctx.flow.buy_bias(),
    );
    for order in &orders {
        if let Err(e) = submit(ctx, cfg, order) {
            eprintln!("[order] {} skipped: {e}", describe(order));
        }
    }
    Ok(())
}

/// Size one order against the live book and send it, logging the fill.
fn submit(ctx: &Context, cfg: &BotConfig, order: &Order) -> Result<()> {
    let Some(swap) = chain::size_order(&ctx.client, &ctx.market, order, cfg.slippage_tolerance)?
    else {
        println!("[order] {} unfillable — skipped", describe(order));
        return Ok(());
    };
    let sig = chain::send_swap(&ctx.client, &ctx.taker, &ctx.market, &swap)?;
    println!(
        "[order] {} in {} → out ≥ {} (exp {}): {sig}",
        describe(order),
        swap.amount_in,
        swap.min_out,
        swap.expected_out,
    );
    Ok(())
}

/// A short human label for an order, for the log line.
pub fn describe(order: &Order) -> String {
    let side = match order.side {
        SwapSide::Buy => "buy",
        SwapSide::Sell => "sell",
    };
    format!("{side} ~{:.2} quote", order.notional)
}
