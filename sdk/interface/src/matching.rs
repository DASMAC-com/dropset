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
//! endpoint. The consensus-critical arithmetic — flush-level pricing, the
//! size-bps fill cap, the price-time sort key — is shared with the
//! on-chain engine via [`crate::matching_math`], so only the iteration /
//! IO around it (reconstructing a book vs. walking the live slab) is
//! distinct here. That residual seam is pinned to the engine by the
//! shared conformance vectors (see `sdk/conformance`).

use crate::layout::{MarketView, Vault, BPS, N_LEVELS};
use crate::matching_math::{flush_level_price, level_fill_atoms, sort_key, taker_fee_atoms};
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

/// A resting level in the reconstructed book: an absolute `price` and the
/// matchable depth at it expressed in **base atoms**, before the taker fee.
/// (Internally an ask carries base atoms and a bid carries quote atoms;
/// [`resting_levels`] normalizes the bid leg to base at the level price so
/// both sides are comparable.)
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct BookLevel {
    pub price: Price,
    pub size: u64,
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

    // Reconstruct the chosen side's book in cross-vault price-time priority.
    // `None` means the book is in a state the engine hard-rejects (a corrupt
    // active DLL) — refuse to quote, matching `swap.rs`. An oversized flush
    // side is not a hard reject: it's skipped per-vault inside
    // [`collect_side_levels`], mirroring the engine zeroing that side.
    let Some(mut levels) = collect_side_levels(market, is_buy, current_slot) else {
        return Quote::default();
    };

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
            let cap_by_taker_quote = lvl
                .price
                .base_for_quote(unfilled.min(u64::MAX as u128) as u64);
            let fill_b = cap_by_taker_quote
                .min(lvl.size as u128)
                .min(base_atoms as u128);
            if fill_b == 0 {
                continue;
            }
            let fill_b = fill_b.min(u64::MAX as u128) as u64;
            let fill_q = lvl.price.quote_for_base(fill_b);
            // A reverse leg past u64::MAX makes the on-chain engine abort
            // the whole take (swap.rs `compute_fill` `require!`s
            // `MathOverflow`), so refuse to quote rather than return the
            // partial accumulated from earlier legs — mirroring the
            // `collect_side_levels` early returns above. Unreachable in
            // practice: `fill_b <= base_for_quote(unfilled)`, so the floor
            // round-trip gives `fill_q <= unfilled <= u64::MAX`. Kept to
            // stay in lockstep with the engine should the taker cap change.
            if fill_q > u64::MAX as u128 {
                return Quote::default();
            }
            let fill_q = fill_q.min(unfilled) as u64;
            (fill_b, fill_q)
        } else {
            let taker_implied_quote = lvl
                .price
                .quote_for_base(unfilled.min(u64::MAX as u128) as u64);
            let fill_q = taker_implied_quote
                .min(lvl.size as u128)
                .min(quote_atoms as u128);
            if fill_q == 0 {
                continue;
            }
            let fill_q = fill_q.min(u64::MAX as u128) as u64;
            let fill_b = lvl.price.base_for_quote(fill_q);
            // Symmetric to the Buy guard: the engine aborts the whole take
            // on a u64 overflow, so refuse to quote rather than return the
            // partial. Unreachable for the same reason
            // (`fill_q <= quote_for_base(unfilled)` ⟹ `fill_b <= unfilled`);
            // kept to mirror the engine.
            if fill_b > u64::MAX as u128 {
                return Quote::default();
            }
            let fill_b = fill_b.min(unfilled) as u64;
            (fill_b, fill_q)
        };

        // Taker fee on the output leg (base on a Buy, quote on a Sell).
        let fee = taker_fee_atoms(if is_buy { fill_base } else { fill_quote }, taker_fee_ppm);

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

/// Reconstruct the **resting book** on one `side` at `current_slot`: the
/// live, matchable levels across every active vault, in cross-vault
/// price-time priority (best price first). This is the same book
/// [`simulate_swap`] fills against, exposed for depth / order-book views;
/// the fill itself is not run.
///
/// Each [`BookLevel`]'s `size` is normalized to **base atoms** — an ask
/// carries base atoms directly, a bid's matchable quote leg is converted to
/// base at the level price — so the two sides are directly comparable. An
/// empty `Vec` means either no live levels or a book the engine would reject
/// (a router must not show depth the engine won't fill).
pub fn resting_levels(
    market: &MarketView<'_>,
    side: SwapSide,
    current_slot: u32,
) -> Vec<BookLevel> {
    let is_buy = side == SwapSide::Buy;
    let Some(levels) = collect_side_levels(market, is_buy, current_slot) else {
        return Vec::new();
    };
    levels
        .into_iter()
        .map(|l| {
            // Asks already carry base atoms; convert a bid's matchable quote
            // leg to base at the level price so depth is base-denominated on
            // both sides.
            let size = if is_buy {
                l.size
            } else {
                l.price.base_for_quote(l.size).min(u64::MAX as u128) as u64
            };
            BookLevel {
                price: l.price,
                size,
            }
        })
        .collect()
}

/// Collect the live, matchable levels of one side (`is_buy` ⇒ asks) across
/// all active vaults, sorted into cross-vault price-time priority: best
/// price first; on ties, older quote (lower nonce) wins, then lower sector,
/// then lower level. Shared by [`simulate_swap`] (which then fills against
/// the levels) and [`resting_levels`] (which exposes them) so the canonical
/// book reconstruction lives in one place.
///
/// Returns `None` only when the book is in a state the on-chain engine
/// hard-rejects, so both callers can refuse rather than quote/show a fill
/// the engine won't honor:
///
/// - **Corrupt active DLL.** `swap.rs` bounds its walk by `market.len()`
///   steps and rejects the whole `swap` (`CorruptVaultList`) when a
///   `Vault.next` pointer cycles or points out of bounds; the bounded
///   `active_vaults` iterator instead *silently truncates* at the same
///   budget and would otherwise quote whatever it collected first.
///
/// An **oversized flush side** (`Σ size_bps > BPS`) is *not* a hard reject:
/// `swap.rs` zeroes that side's `remaining` at flush time so it contributes
/// nothing while the rest of the book still matches, and this collector
/// mirrors that by skipping the offending vault's contribution on the
/// collected side (see [`flush_side_sum_exceeds_bps`]) rather than returning
/// `None`. Both conditions are only reachable from account bytes the program
/// never wrote — see [`MarketView::active_dll_is_corrupt`].
fn collect_side_levels(
    market: &MarketView<'_>,
    is_buy: bool,
    current_slot: u32,
) -> Option<Vec<Lvl>> {
    if market.active_dll_is_corrupt() {
        return None;
    }

    let mut levels: Vec<Lvl> = Vec::new();
    for (sector, v) in market.active_vaults() {
        let reference = v.reference_price.price();
        // Skip vaults the matcher won't touch: invalid/sentinel ref price or
        // frozen (frozen vaults stay on the active DLL but are skipped from
        // the matching set — see swap.rs).
        if !reference.is_valid() || reference.is_zero() || reference.is_infinity() || v.frozen != 0
        {
            continue;
        }
        let nonce = v.reference_price.nonce();
        let flush = v.reference_price.flush_armed();
        // Match-time per-side gate (mirrors `swap.rs`'s flush): when a flush
        // is armed, a side whose `Σ size_bps > BPS` is thrown out of
        // matching — the engine zeroes that side's `remaining`, so it
        // contributes nothing — rather than aborting the whole take. Skip
        // just this vault's contribution on the collected side. When no
        // flush is armed we read `remaining`, already gated at the last
        // flush, so no check is needed there.
        if flush && flush_side_sum_exceeds_bps(v, is_buy) {
            continue;
        }
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
            let key = sort_key(price, is_buy);
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

    levels.sort_by_key(|e| (e.key, e.nonce, e.sector, e.level));
    Some(levels)
}

/// True when the flush profile's *collected* side (`is_buy` ⇒ asks) sums
/// past `BPS` — `Σ size_bps > BPS` on that side, which subsumes any single
/// level `> BPS`. The on-chain matcher zeroes such a side's `remaining` at
/// flush time (see `swap.rs`), dropping it from matching without aborting
/// the take, so the simulator mirrors that by skipping this vault's
/// contribution on the collected side. `set_liquidity_profile` still bounds
/// the sum at write time, so this only fires on an oversized profile written
/// outside that path (corrupted account bytes, or a future write that skips
/// the sum check).
fn flush_side_sum_exceeds_bps(v: &Vault, is_buy: bool) -> bool {
    let side = if is_buy {
        &v.profile.asks
    } else {
        &v.profile.bids
    };
    let sum: u32 = side.iter().map(|l| l.size_bps.get() as u32).sum();
    sum > BPS as u32
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
            // A side that sums past BPS is skipped whole by the
            // [`flush_side_sum_exceeds_bps`] gate in `collect_side_levels`
            // before this runs, so on a collected side every level is
            // `≤ Σ ≤ BPS` and `unwrap_or(0)` is an unreachable total-function
            // fallback, not a silent level drop.
            let size = level_fill_atoms(a.size_bps.get(), base_atoms).unwrap_or(0);
            let expires_at = ref_slot.saturating_add(a.expiry_offset.get());
            (price, size, expires_at)
        } else {
            let b = v.profile.bids[i];
            let price = flush_level_price(reference, b.price_offset.get(), false);
            let size = level_fill_atoms(b.size_bps.get(), quote_atoms).unwrap_or(0);
            let expires_at = ref_slot.saturating_add(b.expiry_offset.get());
            (price, size, expires_at)
        }
    } else {
        let p = if is_buy {
            v.remaining.asks[i]
        } else {
            v.remaining.bids[i]
        };
        (
            Price::from_bits(p.price.get()),
            p.size.get(),
            p.expires_at.get(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::{resting_levels, simulate_swap, BookLevel, Quote, SwapSide};
    use crate::layout::{
        Level, MarketHeader, MarketView, Position, ReferencePrice, Vault,
        ACCOUNT_DISCRIMINATOR_LEN, FLUSH_BIT, NULL_SECTOR, VAULT_ALIGN,
    };
    use crate::price::Price;
    use bytemuck::{bytes_of, cast_slice, Zeroable};

    /// One live `remaining` book level — mirrors the conformance generator.
    fn position(significand: u32, size: u64) -> Position {
        Position {
            price: Price::encode(significand, 0).unwrap().as_u32().into(),
            size: size.into(),
            expires_at: u32::MAX.into(),
        }
    }

    /// A one-vault market whose single active vault carries a live EUR/USD
    /// book in its `remaining` positions (no flush armed): two asks (1.0904
    /// ×1.0M, 1.1393 ×0.8M base) and two bids (1.0796 ×2.0M, 1.0416 ×1.5M
    /// quote). Same shape as `examples/gen_simulate_swap.rs`.
    fn market_data() -> Vec<u8> {
        let mut header = MarketHeader::zeroed();
        header.head = 0u32.into();
        header.tombstone_head = NULL_SECTOR.into();
        header.free_head = NULL_SECTOR.into();
        header.active_count = 1u32.into();
        header.base_mint = [2u8; 32];
        header.quote_mint = [3u8; 32];

        let mut v = Vault::zeroed();
        v.next = NULL_SECTOR.into();
        v.prev = NULL_SECTOR.into();
        v.leader = [1u8; 32];
        v.reference_price = ReferencePrice {
            stamp: 1u64.into(),
            price: Price::encode(10_850_000, 0).unwrap().as_u32().into(),
            quote_slot: 0u32.into(),
        };
        v.base_atoms = 10_000_000u64.into();
        v.quote_atoms = 10_000_000u64.into();
        v.remaining.asks[0] = position(10_904_000, 1_000_000);
        v.remaining.asks[1] = position(11_393_000, 800_000);
        v.remaining.bids[0] = position(10_796_000, 2_000_000);
        v.remaining.bids[1] = position(10_416_000, 1_500_000);

        let vaults = [v];
        let mut buf = Vec::new();
        buf.extend_from_slice(&[0u8; ACCOUNT_DISCRIMINATOR_LEN]);
        buf.extend_from_slice(bytes_of(&header));
        buf.extend_from_slice(&(vaults.len() as u32).to_le_bytes());
        while !buf.len().is_multiple_of(VAULT_ALIGN) {
            buf.push(0);
        }
        buf.extend_from_slice(cast_slice(&vaults));
        buf
    }

    /// Asks come back best-first (lowest price), base-denominated, exactly
    /// as written.
    #[test]
    fn resting_asks_are_best_first_and_base_sized() {
        let data = market_data();
        let view = MarketView::load(&data).unwrap();
        let asks = resting_levels(&view, SwapSide::Buy, 1);
        assert_eq!(
            asks,
            vec![
                BookLevel {
                    price: Price::encode(10_904_000, 0).unwrap(),
                    size: 1_000_000,
                },
                BookLevel {
                    price: Price::encode(11_393_000, 0).unwrap(),
                    size: 800_000,
                },
            ]
        );
    }

    /// Bids come back best-first (highest price), with each level's quote
    /// leg normalized to base at the level price.
    #[test]
    fn resting_bids_are_best_first_and_normalized_to_base() {
        let data = market_data();
        let view = MarketView::load(&data).unwrap();
        let bids = resting_levels(&view, SwapSide::Sell, 1);
        let best = Price::encode(10_796_000, 0).unwrap();
        let next = Price::encode(10_416_000, 0).unwrap();
        assert_eq!(
            bids,
            vec![
                BookLevel {
                    price: best,
                    size: best.base_for_quote(2_000_000).min(u64::MAX as u128) as u64,
                },
                BookLevel {
                    price: next,
                    size: next.base_for_quote(1_500_000).min(u64::MAX as u128) as u64,
                },
            ]
        );
    }

    /// The reconstructed ask depth is exactly what a take large enough to
    /// clear the book consumes: total ask base = gross out (out + fee).
    #[test]
    fn resting_ask_depth_matches_a_clearing_buy() {
        let data = market_data();
        let view = MarketView::load(&data).unwrap();
        let asks = resting_levels(&view, SwapSide::Buy, 1);
        let total_base: u64 = asks.iter().map(|l| l.size).sum();
        assert_eq!(total_base, 1_800_000);

        let q = simulate_swap(&view, SwapSide::Buy, 10_000_000, Price::INFINITY, 1);
        assert_eq!(q.out_amount + q.fee_amount, total_base);
    }

    /// Levels expired at `current_slot` are dropped — past every level's
    /// `expires_at` (here `u32::MAX`), the book is empty on both sides.
    #[test]
    fn expired_levels_are_excluded() {
        let data = market_data();
        let view = MarketView::load(&data).unwrap();
        assert!(resting_levels(&view, SwapSide::Buy, u32::MAX).is_empty());
        assert!(resting_levels(&view, SwapSide::Sell, u32::MAX).is_empty());
    }

    fn level(offset_ppm: u32, size_bps: u16) -> Level {
        Level {
            price_offset: offset_ppm.into(),
            size_bps: size_bps.into(),
            expiry_offset: u32::MAX.into(),
        }
    }

    /// A one-vault market with `FLUSH_BIT` armed and a single-level profile a
    /// side (`ask_bps` / `bid_bps` set on level 0, ±500 ppm off a 1.0850
    /// reference, 1.0M each leg). The taker's first read materializes
    /// `remaining` from this profile — the path the per-side size gate lives
    /// on.
    fn market_data_flush(ask_bps: u16, bid_bps: u16) -> Vec<u8> {
        let mut header = MarketHeader::zeroed();
        header.head = 0u32.into();
        header.tombstone_head = NULL_SECTOR.into();
        header.free_head = NULL_SECTOR.into();
        header.active_count = 1u32.into();
        header.base_mint = [2u8; 32];
        header.quote_mint = [3u8; 32];

        let mut v = Vault::zeroed();
        v.next = NULL_SECTOR.into();
        v.prev = NULL_SECTOR.into();
        v.leader = [1u8; 32];
        v.reference_price = ReferencePrice {
            stamp: (FLUSH_BIT | 1).into(),
            price: Price::encode(10_850_000, 0).unwrap().as_u32().into(),
            quote_slot: 0u32.into(),
        };
        v.base_atoms = 1_000_000u64.into();
        v.quote_atoms = 1_000_000u64.into();
        v.profile.asks[0] = level(5_000, ask_bps);
        v.profile.bids[0] = level(5_000, bid_bps);

        let vaults = [v];
        let mut buf = Vec::new();
        buf.extend_from_slice(&[0u8; ACCOUNT_DISCRIMINATOR_LEN]);
        buf.extend_from_slice(bytes_of(&header));
        buf.extend_from_slice(&(vaults.len() as u32).to_le_bytes());
        while !buf.len().is_multiple_of(VAULT_ALIGN) {
            buf.push(0);
        }
        buf.extend_from_slice(cast_slice(&vaults));
        buf
    }

    /// A flush side whose `Σ size_bps > BPS` is thrown out of matching whole —
    /// its levels don't appear — while the other side still reconstructs and
    /// fills, mirroring the engine zeroing only the offending side rather than
    /// aborting the whole take.
    #[test]
    fn oversize_flush_side_is_skipped_not_the_whole_book() {
        let data = market_data_flush(20_000, 5_000); // asks 200% of leg, bids 50%
        let view = MarketView::load(&data).unwrap();
        assert!(
            resting_levels(&view, SwapSide::Buy, 1).is_empty(),
            "oversized ask side contributes no depth"
        );
        assert!(
            !resting_levels(&view, SwapSide::Sell, 1).is_empty(),
            "healthy bid side still reconstructs"
        );
        assert_eq!(
            simulate_swap(&view, SwapSide::Buy, 500_000, Price::INFINITY, 1),
            Quote::default(),
            "a Buy against the oversized ask side no-fills, it does not abort"
        );
        assert!(
            simulate_swap(&view, SwapSide::Sell, 500_000, Price::ZERO, 1).out_amount > 0,
            "the healthy bid side still fills a Sell"
        );
    }

    /// `Σ == BPS` exactly is valid — the gate is strict (`> BPS`), so a fully
    /// committed side still materializes and fills.
    #[test]
    fn flush_side_sum_at_bps_is_accepted() {
        let data = market_data_flush(10_000, 10_000);
        let view = MarketView::load(&data).unwrap();
        assert!(!resting_levels(&view, SwapSide::Buy, 1).is_empty());
        assert!(simulate_swap(&view, SwapSide::Buy, 500_000, Price::INFINITY, 1).out_amount > 0);
    }
}
