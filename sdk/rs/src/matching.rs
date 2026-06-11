//! Off-chain book reconstruction + fill simulation.
//!
//! A faithful port of the on-chain matcher in
//! programs/dropset/src/instructions/swap.rs: walk the active DLL,
//! materialize each vault's live levels (flushing from the
//! `LiquidityProfile` when `FLUSH_BIT` is armed, else reading
//! `remaining`), sort by cross-vault price-time priority, then fill the
//! taker leg-by-leg until the input is exhausted or the limit price is
//! crossed.
//!
//! Used by the router quoting adapters (e.g. DFlow) and any depth/quote
//! endpoint. It is a hand-mirror of the program logic; per interface.md
//! § SDK it must be pinned to the engine via shared conformance vectors
//! (a CI follow-up) before it is trusted as authoritative.

use crate::layout::{MarketView, Vault, BPS, N_LEVELS, PPM};
use crate::price::Price;

/// Taker side. `Buy` consumes asks (pays quote, receives base); `Sell`
/// consumes bids (pays base, receives quote). Wire value matches the
/// `swap` instruction's `side` arg.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum SwapSide {
    Buy = 0,
    Sell = 1,
}

/// Result of simulating a take against the current book.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct Quote {
    /// Input atoms actually consumed (`<= amount_in`; quote for Buy, base
    /// for Sell). DFlow requires `in_amount <= requested`.
    pub in_amount: u64,
    /// Net output atoms delivered to the taker after the taker fee (base
    /// for Buy, quote for Sell).
    pub out_amount: u64,
    /// Taker fee retained in the matched vaults (output-leg atoms).
    pub fee_amount: u64,
    /// Number of `(vault, level)` legs that filled.
    pub legs: u32,
}

/// Materialize an absolute level price from a reference price and a ppm
/// offset — mirrors `swap::flush_level_price`.
fn flush_level_price(reference: Price, offset_ppm: u32, is_ask: bool) -> Price {
    if reference.is_zero() || reference.is_infinity() {
        return reference;
    }
    let sig = reference.significand() as u128;
    let exp = reference.biased_exponent() as i16;
    let factor: u128 = if is_ask {
        PPM as u128 + offset_ppm as u128
    } else {
        (PPM as u128).saturating_sub(offset_ppm as u128)
    };
    if factor == 0 {
        return Price::ZERO;
    }
    let scaled = (sig * factor) / (PPM as u128);
    Price::from_scaled(scaled as u64, exp).unwrap_or(Price::ZERO)
}

/// A live, matchable level pulled from a vault during book construction.
#[derive(Copy, Clone)]
struct Lvl {
    /// Sort key: `price.as_u32()` (asks) or `price.bid_key()` (bids).
    key: u32,
    price: Price,
    nonce: u64,
    sector: u32,
    level: u32,
    size: u64,
}

/// Simulate a take. Returns the achievable [`Quote`] against the book in
/// `market` at `current_slot`, capping the consumed input when the book
/// cannot fully absorb `amount_in`.
///
/// `taker_fee_ppm` is read from the market header; `limit_price` is the
/// worst acceptable fill (use [`Price::INFINITY`] for a Buy / [`Price::ZERO`]
/// for a Sell to disable the bound).
pub fn simulate_swap(
    market: &MarketView<'_>,
    side: SwapSide,
    amount_in: u64,
    limit_price: Price,
    current_slot: u32,
) -> Quote {
    let taker_fee_ppm = market.header.taker_fee.get() as u128;
    let is_buy = side == SwapSide::Buy;

    // ── Book construction: collect live levels of the chosen side. ──
    let mut levels: Vec<Lvl> = Vec::new();
    for (sector, v) in market.active_vaults() {
        let reference = v.reference_price.price();
        // Skip vaults the matcher won't touch: invalid/sentinel ref
        // price or frozen (frozen vaults stay on the active DLL but are
        // skipped from the matching set — see swap.rs).
        if !reference.is_valid()
            || reference.is_zero()
            || reference.is_infinity()
            || v.frozen != 0
        {
            continue;
        }
        let nonce = v.reference_price.nonce();
        let flush = v.reference_price.flush_armed();
        let ref_slot = v.reference_price.quote_slot.get();
        let base_atoms = v.base_atoms.get();
        let quote_atoms = v.quote_atoms.get();

        for i in 0..N_LEVELS {
            let (price, size, expires_at) = level_state(
                v,
                i,
                is_buy,
                flush,
                reference,
                ref_slot,
                base_atoms,
                quote_atoms,
            );
            if size == 0
                || expires_at <= current_slot
                || price.is_zero()
                || price.is_infinity()
                || !price.is_valid()
            {
                continue;
            }
            let key = if is_buy { price.as_u32() } else { price.bid_key() };
            levels.push(Lvl {
                key,
                price,
                nonce,
                sector,
                level: i as u32,
                size,
            });
        }
    }

    // Cross-vault price-time priority: best price first; on ties, older
    // quote (lower nonce) wins, then lower sector, then lower level.
    levels.sort_by_key(|e| (e.key, e.nonce, e.sector, e.level));

    // ── Fill loop. Track per-touched-sector inventory so a vault whose
    //    multiple levels match decrements consistently (cap_by_vault). ──
    let mut inv: std::collections::BTreeMap<u32, (u64, u64)> = std::collections::BTreeMap::new();
    let mut unfilled: u128 = amount_in as u128;
    let mut total_out: u128 = 0;
    let mut total_fee: u128 = 0;
    let mut legs: u32 = 0;

    for lvl in &mut levels {
        if unfilled == 0 {
            break;
        }
        // Limit-price filter — levels are best-first, so the first cross
        // means every remaining level crosses too.
        let crosses = if is_buy {
            lvl.price.as_u32() > limit_price.as_u32() && !limit_price.is_infinity()
        } else {
            lvl.price.as_u32() < limit_price.as_u32() && !limit_price.is_zero()
        };
        if crosses {
            break;
        }

        let v = &market.sectors()[lvl.sector as usize];
        let (base_atoms, quote_atoms) = *inv
            .entry(lvl.sector)
            .or_insert((v.base_atoms.get(), v.quote_atoms.get()));

        let (fill_base, fill_quote): (u64, u64) = if is_buy {
            let cap_by_taker_quote =
                lvl.price.base_for_quote(unfilled.min(u64::MAX as u128) as u64);
            let fill_b = cap_by_taker_quote
                .min(lvl.size as u128)
                .min(base_atoms as u128);
            if fill_b == 0 {
                continue;
            }
            let fill_b = fill_b.min(u64::MAX as u128) as u64;
            let fill_q = lvl.price.quote_for_base(fill_b);
            if fill_q > u64::MAX as u128 {
                break;
            }
            let fill_q = fill_q.min(unfilled) as u64;
            (fill_b, fill_q)
        } else {
            let taker_implied_quote =
                lvl.price.quote_for_base(unfilled.min(u64::MAX as u128) as u64);
            let fill_q = taker_implied_quote
                .min(lvl.size as u128)
                .min(quote_atoms as u128);
            if fill_q == 0 {
                continue;
            }
            let fill_q = fill_q.min(u64::MAX as u128) as u64;
            let fill_b = lvl.price.base_for_quote(fill_q);
            if fill_b > u64::MAX as u128 {
                break;
            }
            let fill_b = fill_b.min(unfilled) as u64;
            (fill_b, fill_q)
        };

        // Taker fee on the output leg.
        let fee = if is_buy {
            (fill_base as u128 * taker_fee_ppm) / PPM as u128
        } else {
            (fill_quote as u128 * taker_fee_ppm) / PPM as u128
        } as u128;

        // Decrement simulated vault inventory + this level's allowance,
        // mirroring the on-chain per-leg mutation.
        let entry = inv.get_mut(&lvl.sector).unwrap();
        if is_buy {
            let net_base_out = fill_base.saturating_sub(fee as u64);
            entry.0 = entry.0.saturating_sub(net_base_out);
            entry.1 = entry.1.saturating_add(fill_quote);
            lvl.size = lvl.size.saturating_sub(fill_base);
            unfilled = unfilled.saturating_sub(fill_quote as u128);
            total_out += fill_base as u128;
        } else {
            let net_quote_out = fill_quote.saturating_sub(fee as u64);
            entry.0 = entry.0.saturating_add(fill_base);
            entry.1 = entry.1.saturating_sub(net_quote_out);
            lvl.size = lvl.size.saturating_sub(fill_quote);
            unfilled = unfilled.saturating_sub(fill_base as u128);
            total_out += fill_quote as u128;
        }
        total_fee += fee;
        legs += 1;
    }

    let out_net = total_out.saturating_sub(total_fee).min(u64::MAX as u128) as u64;
    Quote {
        in_amount: (amount_in as u128 - unfilled).min(u64::MAX as u128) as u64,
        out_amount: out_net,
        fee_amount: total_fee.min(u64::MAX as u128) as u64,
        legs,
    }
}

/// Resolve a single level's `(price, size, expires_at)` for the chosen
/// side: materialize from the `LiquidityProfile` if a flush is armed
/// (mirroring `swap.rs`), else read the stored `remaining` state.
#[allow(clippy::too_many_arguments)]
fn level_state(
    v: &Vault,
    i: usize,
    is_buy: bool,
    flush: bool,
    reference: Price,
    ref_slot: u32,
    base_atoms: u64,
    quote_atoms: u64,
) -> (Price, u64, u32) {
    if flush {
        if is_buy {
            let a = v.profile.asks[i];
            let price = flush_level_price(reference, a.price_offset.get(), true);
            let size = (base_atoms as u128 * a.size_bps.get() as u128 / BPS as u128) as u64;
            let expires_at = ref_slot.saturating_add(a.expiry_offset.get());
            (price, size, expires_at)
        } else {
            let b = v.profile.bids[i];
            let price = flush_level_price(reference, b.price_offset.get(), false);
            let size = (quote_atoms as u128 * b.size_bps.get() as u128 / BPS as u128) as u64;
            let expires_at = ref_slot.saturating_add(b.expiry_offset.get());
            (price, size, expires_at)
        }
    } else {
        let p = if is_buy {
            v.remaining.asks[i]
        } else {
            v.remaining.bids[i]
        };
        (Price::from_bits(p.price.get()), p.size.get(), p.expires_at.get())
    }
}
