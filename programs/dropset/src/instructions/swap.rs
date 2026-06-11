//! `swap` (spec's `Take`) — multi-vault taker fill on the ephemeral
//! book.
//!
//! Walks the market's active DLL from `market.head` via `Vault.next`,
//! flushes per-vault on `FLUSH_BIT`, and collects every live level
//! into a `Vec<HeapEntry>` on the program heap. Sorts by
//! `(price_key, nonce, sector_idx, level_idx)` so the spec's
//! cross-vault price-time priority falls out of a single
//! `sort_by_key`, then walks the sorted vec filling the taker
//! leg-by-leg until `amount_in` is exhausted, the limit price is
//! crossed, or the heap is drained.
//!
//! `min_out` adds an SDK-composability soft-revert: if the
//! achievable net output (after taker fee) is below the caller's
//! `min_out`, every per-leg snapshot is restored to its pre-swap
//! value and the handler returns `Ok(Vec::new())` so the
//! surrounding tx can survive a no-fill. `min_out == 0` opts out
//! and the legacy any-fill-counts behavior holds.
//!
//! Per spec § **Events and emission**, every filled `(vault, level)`
//! leg emits one bytemuck `FillEvent` via `emit_cpi!`. No
//! truncation: the matcher accumulates a `Vec<FillEvent>` and
//! `lib.rs` dispatches each via the macro one at a time.

extern crate alloc;

use anchor_lang_v2::prelude::*;
#[allow(unused_imports)]
use anchor_spl_v2::{
    associated_token::AssociatedToken,
    token_2022::{transfer_checked, TransferChecked},
    token_interface::{Mint, TokenAccount, TokenInterface},
};

use crate::{
    errors::DropsetError,
    events::FillEvent,
    state::{Market, BPS, FLUSH_BIT, PPM},
    Price, N_LEVELS,
};

/// Side of a taker fill — `Buy` consumes asks (taker pays quote, gets
/// base); `Sell` consumes bids (taker pays base, gets quote). Lives
/// here rather than in [`crate::events`] because it is a swap
/// instruction argument, not an event field.
#[repr(u8)]
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum SwapSide {
    Buy = 0,
    Sell = 1,
}

impl SwapSide {
    /// Convert from the wire `u8` argument.
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(Self::Buy),
            1 => Some(Self::Sell),
            _ => None,
        }
    }
}

#[event_cpi]
#[derive(Accounts)]
pub struct Swap {
    /// Taker.
    #[account(mut)]
    pub taker: Signer,
    /// Market the target vault lives on.
    #[account(mut)]
    pub market: Market,
    #[account(address = market.base_mint)]
    pub base_mint: InterfaceAccount<Mint>,
    #[account(address = market.quote_mint)]
    pub quote_mint: InterfaceAccount<Mint>,
    pub base_token_program: Interface<'static, TokenInterface>,
    pub quote_token_program: Interface<'static, TokenInterface>,
    #[account(
        mut,
        associated_token::mint = base_mint,
        associated_token::authority = taker,
        associated_token::token_program = base_token_program,
    )]
    pub taker_base_ata: InterfaceAccount<TokenAccount>,
    #[account(
        mut,
        associated_token::mint = quote_mint,
        associated_token::authority = taker,
        associated_token::token_program = quote_token_program,
    )]
    pub taker_quote_ata: InterfaceAccount<TokenAccount>,
    #[account(
        mut,
        associated_token::mint = base_mint,
        associated_token::authority = market,
        associated_token::token_program = base_token_program,
    )]
    pub market_base_treasury: InterfaceAccount<TokenAccount>,
    #[account(
        mut,
        associated_token::mint = quote_mint,
        associated_token::authority = market,
        associated_token::token_program = quote_token_program,
    )]
    pub market_quote_treasury: InterfaceAccount<TokenAccount>,
    pub clock: Sysvar<Clock>,
}

/// Materialize an absolute-price `Price` from a reference price and a
/// ppm offset. For asks: `ref × (PPM + offset) / PPM`. For bids:
/// `ref × max(PPM − offset, 0) / PPM` (saturating; bids with offset ≥
/// PPM produce `Price::ZERO`, which the limit-price filter then
/// excludes).
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

/// `level.size_bps` × the matching leg, in atoms.
///
/// `size_bps <= BPS` is enforced at `set_liquidity_profile`, so the
/// guard below never fires on a profile written through the normal
/// path. It is load-bearing only against a future instruction that
/// reshapes a profile without that validation: with the invariant held
/// the product is at most `leg_atoms * BPS`, which divided by `BPS` is
/// `<= leg_atoms <= u64::MAX`, so the cast is lossless. We `require!`
/// the invariant rather than silently `.min(u64::MAX)`-clamping, which
/// would mask the bug by shrinking the level's materialized size.
fn flush_level_size(size_bps: u16, leg_atoms: u64) -> Result<u64> {
    require!(
        size_bps as u64 <= BPS,
        DropsetError::LiquidityProfileSizeOverflow
    );
    Ok((leg_atoms as u128 * size_bps as u128 / BPS as u128) as u64)
}

/// One entry on the ephemeral matching heap. Built per-`(vault,
/// level)` pair during the active-DLL walk; sorted by
/// `(price_key, nonce, sector_idx, level_idx)` so the canonical
/// price-time priority falls out of a single `sort_by_key`. `nonce` is
/// `stamp & !FLUSH_BIT` from the vault's reference price — i.e. the
/// `market.nonce` snapshot at the most recent quote write, which is
/// the spec's price-time tiebreaker.
#[derive(Copy, Clone)]
struct HeapEntry {
    /// Sort key: `price.as_u32()` for asks, `price.bid_key()` for bids.
    price_key: u32,
    /// Original `Price` for fill math.
    price: crate::Price,
    /// `stamp & !FLUSH_BIT` — older nonce wins on equal-price ties.
    nonce: u64,
    /// Sector index of the source vault.
    sector_idx: u32,
    /// Level index within the source vault.
    level_idx: u32,
    /// Materialized level size at heap-build time. The fill loop
    /// re-reads the live `remaining[i].size` so it sees prior-leg
    /// decrements; this snapshot is only the upper bound at first
    /// visit, used to skip dead levels.
    size: u64,
}

impl Swap {
    /// Returns the per-leg `FillEvent` list for `lib.rs` to dispatch
    /// via `emit_cpi!`. The matching engine can't call `emit_cpi!`
    /// directly — the macro requires `ctx` in scope, which the
    /// `&mut self` handler can't see. Accumulating and emitting at
    /// the lib.rs layer preserves the spec's per-leg-fill rule
    /// (§ Events and emission → Granularity).
    pub fn swap(
        &mut self,
        side_u8: u8,
        amount_in: u64,
        limit_price_bits: u32,
        min_out: u64,
    ) -> Result<alloc::vec::Vec<FillEvent>> {
        let side = SwapSide::from_u8(side_u8).ok_or(DropsetError::InvalidSwapSide)?;
        let limit_price = Price::from_bits(limit_price_bits);
        require!(limit_price.is_valid(), DropsetError::InvalidPrice);
        // Sentinel semantics per side — see `price.rs` for the
        // `ZERO`/`INFINITY` definitions:
        //   * `Buy` accepts `INFINITY` (no upper bound on ask price)
        //     and any regular price; `ZERO` would reject every ask
        //     and is rejected up front as a likely caller mistake.
        //   * `Sell` accepts `ZERO` (no lower bound on bid price)
        //     and any regular price; `INFINITY` would reject every
        //     bid and is rejected symmetrically.
        let limit_ok = match side {
            SwapSide::Buy => !limit_price.is_zero(),
            SwapSide::Sell => !limit_price.is_infinity(),
        };
        require!(limit_ok, DropsetError::InvalidLimitPrice);
        require!(amount_in > 0, DropsetError::InvalidAmountIn);

        // Snapshot market-wide constants the loop needs.
        let market_addr = *self.market.address();
        let market_bump = self.market.bump;
        let base_mint_addr = self.market.base_mint;
        let quote_mint_addr = self.market.quote_mint;
        let taker_fee_ppm = self.market.taker_fee.get() as u64;
        let current_slot = self.clock.slot as u32;
        let head = self.market.head.get();

        // Walk the active DLL from `market.head` via `Vault.next`.
        // Per the spec § Order matching → Book construction: for
        // each active vault, range-check `reference_price.price`,
        // flush from `LiquidityProfile` if `FLUSH_BIT` is armed, then
        // push the chosen side's live levels onto an ephemeral heap.
        // Tombstoned vaults sit on a separate DLL and are skipped by
        // construction.
        let mut heap: alloc::vec::Vec<HeapEntry> = alloc::vec::Vec::new();
        let mut cur = head;
        // Bound the walk by `market.len()` so a corrupt `next` ptr
        // that creates a cycle can't burn CU indefinitely. Each live
        // sector should appear at most once on the active DLL.
        let mut steps_remaining = self.market.len();
        // Track which vaults' `FLUSH_BIT` got cleared so the soft-
        // revert path can re-arm them. The materialized
        // `remaining[*]` arrays stay in place — they're derived from
        // the (now-restored) inventory, so the next flush against
        // current state will overwrite anyway.
        let mut flushed_sectors: alloc::vec::Vec<u32> = alloc::vec::Vec::new();
        while cur != crate::state::NULL_SECTOR {
            require!(steps_remaining > 0, DropsetError::CorruptVaultList);
            steps_remaining -= 1;
            // Sanity: bounds-check before borrowing the sector. A
            // corrupt `next` would have already broken the DLL ops
            // that produced it, but the matching engine reads the
            // pointer adversarially.
            require!(
                (cur as usize) < self.market.len(),
                DropsetError::CorruptVaultList
            );
            let next_sector = {
                let v = &self.market.as_slice()[cur as usize];
                v.next.get()
            };
            // Read vault meta + decide whether to flush. A vault on
            // the active DLL must have a non-default leader by
            // construction (free-list sectors live elsewhere). An
            // invalid / sentinel reference price skips the vault
            // (rather than aborting) — spec L1554-1563.
            let (vault_active, stamp, ref_price, ref_slot, base_atoms, quote_atoms) = {
                let v = &self.market.as_slice()[cur as usize];
                let p = v.reference_price.price;
                let valid = p.is_valid() && !p.is_zero() && !p.is_infinity();
                // Spec § Vault → Frozen and tombstoned vaults: frozen
                // vaults stay on the active DLL but their levels die
                // off as `expires_at` passes. Implement that
                // semantically here by skipping frozen vaults
                // entirely from the matching set, so the leader's
                // freeze is enforced from the first instruction
                // after it lands rather than waiting for level
                // expiry to kick in.
                let frozen = v.frozen.get();
                (
                    valid && !frozen,
                    v.reference_price.stamp.get(),
                    p,
                    v.reference_price.quote_slot.get(),
                    v.base_atoms.get(),
                    v.quote_atoms.get(),
                )
            };
            if !vault_active {
                cur = next_sector;
                continue;
            }
            if stamp & FLUSH_BIT != 0 {
                // Materialize Remaining from LiquidityProfile +
                // current inventory, then clear FLUSH_BIT.
                let v = &mut self.market.as_mut_slice()[cur as usize];
                for i in 0..N_LEVELS {
                    let bid = v.profile.bids[i];
                    let ask = v.profile.asks[i];
                    v.remaining.bids[i].price =
                        flush_level_price(ref_price, bid.price_offset.get(), false);
                    v.remaining.bids[i].size =
                        flush_level_size(bid.size_bps.get(), quote_atoms)?.into();
                    v.remaining.bids[i].expires_at =
                        ref_slot.saturating_add(bid.expiry_offset.get()).into();
                    v.remaining.asks[i].price =
                        flush_level_price(ref_price, ask.price_offset.get(), true);
                    v.remaining.asks[i].size =
                        flush_level_size(ask.size_bps.get(), base_atoms)?.into();
                    v.remaining.asks[i].expires_at =
                        ref_slot.saturating_add(ask.expiry_offset.get()).into();
                }
                v.reference_price.stamp = (stamp & !FLUSH_BIT).into();
                flushed_sectors.push(cur);
            }
            // Collect live levels of the chosen side from this vault.
            let nonce = stamp & !FLUSH_BIT;
            {
                let v = &self.market.as_slice()[cur as usize];
                for i in 0..N_LEVELS {
                    let lvl = match side {
                        SwapSide::Buy => v.remaining.asks[i],
                        SwapSide::Sell => v.remaining.bids[i],
                    };
                    let size = lvl.size.get();
                    let expires_at = lvl.expires_at.get();
                    let price = lvl.price;
                    if size == 0
                        || expires_at <= current_slot
                        || price.is_zero()
                        || price.is_infinity()
                        || !price.is_valid()
                    {
                        continue;
                    }
                    let price_key = match side {
                        SwapSide::Buy => price.as_u32(),
                        SwapSide::Sell => price.bid_key(),
                    };
                    heap.push(HeapEntry {
                        price_key,
                        price,
                        nonce,
                        sector_idx: cur,
                        level_idx: i as u32,
                        size,
                    });
                }
            }
            cur = next_sector;
        }
        // Sort by (price_key, nonce, sector_idx, level_idx) — best
        // price first; on ties, older quote (lower nonce) wins; on
        // further ties, lower sector_idx wins; finally lower
        // level_idx. This is the spec's price-time priority over a
        // single materialized snapshot of the book.
        heap.sort_by_key(|e| (e.price_key, e.nonce, e.sector_idx, e.level_idx));
        let live_count = heap.len();
        let levels: alloc::vec::Vec<(u32, u32, Price, u64, u32)> = heap
            .iter()
            .map(|e| {
                (
                    e.sector_idx,
                    e.level_idx,
                    e.price,
                    e.size,
                    e.nonce.min(u32::MAX as u64) as u32,
                )
            })
            .collect();

        // Build the market PDA signer seeds (for the return-leg CPI).
        let bump_arr = [market_bump];
        let base_seed: &[u8] = base_mint_addr.as_ref();
        let quote_seed: &[u8] = quote_mint_addr.as_ref();
        let bump_seed: &[u8] = &bump_arr;
        let signer_seeds_inner: [&[u8]; 3] = [base_seed, quote_seed, bump_seed];
        let signer_seeds: [&[&[u8]]; 1] = [&signer_seeds_inner];

        // The caller supplies one side's "input" amount. The other
        // side's effective amount comes from filling against levels.
        // Convention:
        //   Buy:  `amount_in` is quote atoms (the taker's spend budget)
        //   Sell: `amount_in` is base atoms (the taker's sell amount)
        let mut taker_unfilled_in: u128 = amount_in as u128;
        let mut total_out: u128 = 0;
        let mut total_fee: u128 = 0;
        let mut filled_legs: u32 = 0;
        let mut fill_events: alloc::vec::Vec<FillEvent> = alloc::vec::Vec::new();
        // Snapshots taken BEFORE each leg mutates state, so that a
        // failed `min_out` check can restore the per-vault inventory
        // and per-level `remaining.size` to exactly the pre-swap
        // values. Walked in reverse so overlapping snapshots of the
        // same `sector_idx` resolve to the earliest captured state.
        struct LegSnapshot {
            sector_idx: u32,
            level_idx: u32,
            is_ask_side: bool,
            base_before: u64,
            quote_before: u64,
            size_before: u64,
        }
        let mut snapshots: alloc::vec::Vec<LegSnapshot> = alloc::vec::Vec::new();
        let nonce_at_start = self.market.nonce.get();

        for &(sector_idx, level_idx, price, level_size, _nonce) in levels.iter().take(live_count) {
            // Limit-price filter — also the review's "early-exit when
            // the best level can't fill" gate (WARNING 1a). `levels`
            // is sorted best-price-first, so the first level that
            // crosses the taker's limit means every remaining level
            // crosses too: `break` bails before any snapshot or fill
            // work, and the `filled_legs == 0` path below re-arms
            // FLUSH_BIT and returns an empty result. (This does not
            // meter the INF-limit / MAX-`min_out` soft-revert spam,
            // which crosses zero levels and walks the whole book —
            // that needs a soft-revert fee or per-slot cooldown,
            // tracked separately.)
            let crosses = match side {
                SwapSide::Buy => {
                    price.as_u32() > limit_price.as_u32() && !limit_price.is_infinity()
                }
                SwapSide::Sell => price.as_u32() < limit_price.as_u32() && !limit_price.is_zero(),
            };
            if crosses {
                break;
            }
            if taker_unfilled_in == 0 {
                break;
            }

            // Snapshot the matched vault's current inventory — each
            // leg debits/credits it, so we read fresh.
            let (base_atoms, quote_atoms) = {
                let v = &self.market.as_slice()[sector_idx as usize];
                (v.base_atoms.get(), v.quote_atoms.get())
            };

            let (fill_base, fill_quote): (u64, u64) = match side {
                SwapSide::Buy => {
                    // level.size is in base; convert the taker's
                    // quote budget to base via the level price.
                    let cap_by_taker_quote =
                        price.base_for_quote(taker_unfilled_in.min(u64::MAX as u128) as u64);
                    let cap_by_level = level_size as u128;
                    let cap_by_vault = base_atoms as u128;
                    let fill_b = cap_by_taker_quote.min(cap_by_level).min(cap_by_vault);
                    if fill_b == 0 {
                        continue;
                    }
                    let fill_b_u64 = fill_b.min(u64::MAX as u128) as u64;
                    let fill_q = price.quote_for_base(fill_b_u64);
                    // 1d: a huge price can push the quote leg past
                    // u64::MAX. Reject explicitly instead of silently
                    // truncating — a clamped quote would debit the
                    // taker (and book the vault) less than the price
                    // implies, shattering the treasury invariant.
                    require!(fill_q <= u64::MAX as u128, DropsetError::MathOverflow);
                    // 1c: never charge the taker more than their
                    // remaining input. The decoders truncate toward
                    // zero in both directions, so
                    // `quote_for_base(base_for_quote(q))` can exceed
                    // `q` by a few atoms; capping the quote leg (the
                    // taker's input on a Buy) keeps the per-leg vault
                    // credit equal to what the taker actually pays and
                    // stops `taker_unfilled_in` from saturating to 0
                    // and billing the full budget for a partial fill.
                    let fill_q = fill_q.min(taker_unfilled_in);
                    (fill_b_u64, fill_q as u64)
                }
                SwapSide::Sell => {
                    // level.size is in quote; convert the taker's
                    // unfilled base to quote via the level price.
                    let taker_implied_quote =
                        price.quote_for_base(taker_unfilled_in.min(u64::MAX as u128) as u64);
                    let cap_by_level = level_size as u128;
                    let cap_by_vault = quote_atoms as u128;
                    let fill_q = taker_implied_quote.min(cap_by_level).min(cap_by_vault);
                    if fill_q == 0 {
                        continue;
                    }
                    let fill_q_u64 = fill_q.min(u64::MAX as u128) as u64;
                    let fill_b = price.base_for_quote(fill_q_u64);
                    // 1d: symmetric overflow guard on the base leg.
                    require!(fill_b <= u64::MAX as u128, DropsetError::MathOverflow);
                    // 1c: cap the base leg (the taker's input on a
                    // Sell) to the remaining input for the same
                    // round-trip-rounding reason as the Buy side.
                    let fill_b = fill_b.min(taker_unfilled_in);
                    (fill_b as u64, fill_q_u64)
                }
            };

            // Apply taker fee on the *output* leg, retained in the
            // matched vault for the depositors' benefit.
            let fee = match side {
                SwapSide::Buy => ((fill_base as u128) * (taker_fee_ppm as u128)) / (PPM as u128),
                SwapSide::Sell => ((fill_quote as u128) * (taker_fee_ppm as u128)) / (PPM as u128),
            };
            let fee_u64 = fee.min(u64::MAX as u128) as u64;

            // Snapshot the pre-leg state so the `min_out` soft-revert
            // at the end can roll back every mutation cleanly.
            {
                let v = &self.market.as_slice()[sector_idx as usize];
                let size_before = match side {
                    SwapSide::Buy => v.remaining.asks[level_idx as usize].size.get(),
                    SwapSide::Sell => v.remaining.bids[level_idx as usize].size.get(),
                };
                snapshots.push(LegSnapshot {
                    sector_idx,
                    level_idx,
                    is_ask_side: matches!(side, SwapSide::Buy),
                    base_before: v.base_atoms.get(),
                    quote_before: v.quote_atoms.get(),
                    size_before,
                });
            }

            // Update vault inventory and level remaining size. The
            // fee is retained in the vault on the output leg — so the
            // inventory debit is `fill_<out> - fee_u64`, matching the
            // `net_out` actually transferred to the taker. This keeps
            // the treasury-vs-vault invariant
            // `treasury.amount == Σ vault.<leg>_atoms` holding per
            // leg: treasury sends `net_out`, vault books `-(net_out)`.
            let (new_base, new_quote) = {
                let v = &mut self.market.as_mut_slice()[sector_idx as usize];
                let (b_new, q_new) = match side {
                    SwapSide::Buy => {
                        // Taker buys base, pays quote. Fee retained in base.
                        let net_base_out = fill_base.saturating_sub(fee_u64);
                        let b = v.base_atoms.get().saturating_sub(net_base_out);
                        let q = v.quote_atoms.get().saturating_add(fill_quote);
                        v.base_atoms = b.into();
                        v.quote_atoms = q.into();
                        v.remaining.asks[level_idx as usize].size = (v.remaining.asks
                            [level_idx as usize]
                            .size
                            .get()
                            .saturating_sub(fill_base))
                        .into();
                        (b, q)
                    }
                    SwapSide::Sell => {
                        // Taker sells base, receives quote. Fee retained in quote.
                        let net_quote_out = fill_quote.saturating_sub(fee_u64);
                        let b = v.base_atoms.get().saturating_add(fill_base);
                        let q = v.quote_atoms.get().saturating_sub(net_quote_out);
                        v.base_atoms = b.into();
                        v.quote_atoms = q.into();
                        v.remaining.bids[level_idx as usize].size = (v.remaining.bids
                            [level_idx as usize]
                            .size
                            .get()
                            .saturating_sub(fill_quote))
                        .into();
                        (b, q)
                    }
                };
                (b_new, q_new)
            };

            // Bump market.nonce per leg (header borrow after the tail
            // mutation completes).
            let nonce = self.market.nonce.get();
            let new_nonce = nonce.saturating_add(1);
            self.market.nonce = new_nonce.into();

            // Decrement the taker's remaining input.
            let consumed_in: u128 = match side {
                SwapSide::Buy => fill_quote as u128,
                SwapSide::Sell => fill_base as u128,
            };
            taker_unfilled_in = taker_unfilled_in.saturating_sub(consumed_in);
            total_out += match side {
                SwapSide::Buy => fill_base as u128,
                SwapSide::Sell => fill_quote as u128,
            };
            total_fee = total_fee.saturating_add(fee);
            filled_legs = filled_legs.saturating_add(1);

            // Emit one event per matched (vault, level) leg.
            let (leader, quote_authority) = {
                let v = &self.market.as_slice()[sector_idx as usize];
                (v.leader, v.quote_authority)
            };
            fill_events.push(FillEvent {
                market: market_addr,
                taker: *self.taker.address(),
                leader,
                quote_authority,
                side: side_u8,
                _pad: [0; 7],
                sector_idx,
                level_idx,
                fill_base,
                fill_quote,
                fill_price: price,
                _pad2: [0; 4],
                base_atoms_after: new_base,
                quote_atoms_after: new_quote,
                nonce_after: new_nonce,
                taker_fee_atoms: fee_u64,
            });
        }

        // Soft-revert check: if the net output the taker would receive
        // is below the caller-specified `min_out`, roll back every
        // mutation and return `Ok(Vec::new())`. The instruction
        // doesn't error — the surrounding transaction survives, which
        // is the contract SDK callers rely on when bundling a swap
        // with other instructions. `min_out == 0` opts out (the
        // legacy fail-on-zero-fill behavior is recovered by passing
        // any non-zero `min_out`).
        let achievable_net_out = total_out.saturating_sub(total_fee).min(u64::MAX as u128) as u64;
        if filled_legs == 0 || achievable_net_out < min_out {
            // Walk snapshots in reverse so two legs that touched the
            // same sector's inventory restore to the earliest
            // captured value.
            for snap in snapshots.iter().rev() {
                let v = &mut self.market.as_mut_slice()[snap.sector_idx as usize];
                v.base_atoms = snap.base_before.into();
                v.quote_atoms = snap.quote_before.into();
                if snap.is_ask_side {
                    v.remaining.asks[snap.level_idx as usize].size = snap.size_before.into();
                } else {
                    v.remaining.bids[snap.level_idx as usize].size = snap.size_before.into();
                }
            }
            // Re-arm `FLUSH_BIT` on every vault we flushed during
            // the matching walk. Without this, a leader's pending
            // re-materialization would be silently consumed by a
            // failed-`min_out` taker, leaving subsequent legitimate
            // takers reading stale `remaining[*]`.
            for &sector_idx in &flushed_sectors {
                let v = &mut self.market.as_mut_slice()[sector_idx as usize];
                let cur = v.reference_price.stamp.get();
                v.reference_price.stamp = (cur | FLUSH_BIT).into();
            }
            self.market.nonce = nonce_at_start.into();
            return Ok(alloc::vec::Vec::new());
        }

        // Net taker transfer: pay the input leg in, receive the output
        // leg out. Both legs are aggregated across all matched levels
        // — one SPL transfer per side.
        let (taker_in_atoms, taker_out_atoms) = match side {
            SwapSide::Buy => (
                (amount_in as u128 - taker_unfilled_in) as u64, // quote spent
                total_out as u64,                               // base received
            ),
            SwapSide::Sell => (
                (amount_in as u128 - taker_unfilled_in) as u64, // base spent
                total_out as u64,                               // quote received
            ),
        };

        // Input leg: taker → treasury.
        if taker_in_atoms > 0 {
            match side {
                SwapSide::Buy => {
                    let decimals = self.quote_mint.decimals();
                    let cpi = CpiContext::new(
                        self.quote_token_program.address(),
                        TransferChecked {
                            from: self.taker_quote_ata.cpi_handle_mut(),
                            mint: self.quote_mint.cpi_handle(),
                            to: self.market_quote_treasury.cpi_handle_mut(),
                            authority: self.taker.cpi_handle(),
                        },
                    );
                    transfer_checked(cpi, taker_in_atoms, decimals)?;
                }
                SwapSide::Sell => {
                    let decimals = self.base_mint.decimals();
                    let cpi = CpiContext::new(
                        self.base_token_program.address(),
                        TransferChecked {
                            from: self.taker_base_ata.cpi_handle_mut(),
                            mint: self.base_mint.cpi_handle(),
                            to: self.market_base_treasury.cpi_handle_mut(),
                            authority: self.taker.cpi_handle(),
                        },
                    );
                    transfer_checked(cpi, taker_in_atoms, decimals)?;
                }
            }
        }
        // Output leg: treasury → taker, signed by market PDA. Net
        // amount = total_out − fee retained in the vault.
        let net_out = taker_out_atoms.saturating_sub(total_fee as u64);
        if net_out > 0 {
            match side {
                SwapSide::Buy => {
                    let decimals = self.base_mint.decimals();
                    let cpi = CpiContext::new_with_signer(
                        self.base_token_program.address(),
                        TransferChecked {
                            from: self.market_base_treasury.cpi_handle_mut(),
                            mint: self.base_mint.cpi_handle(),
                            to: self.taker_base_ata.cpi_handle_mut(),
                            authority: self.market.cpi_handle(),
                        },
                        &signer_seeds,
                    );
                    transfer_checked(cpi, net_out, decimals)?;
                }
                SwapSide::Sell => {
                    let decimals = self.quote_mint.decimals();
                    let cpi = CpiContext::new_with_signer(
                        self.quote_token_program.address(),
                        TransferChecked {
                            from: self.market_quote_treasury.cpi_handle_mut(),
                            mint: self.quote_mint.cpi_handle(),
                            to: self.taker_quote_ata.cpi_handle_mut(),
                            authority: self.market.cpi_handle(),
                        },
                        &signer_seeds,
                    );
                    transfer_checked(cpi, net_out, decimals)?;
                }
            }
        }
        Ok(fill_events)
    }
}
