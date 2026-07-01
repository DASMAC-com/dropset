// cspell:word cooldown
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

use dropset_math_core::matching_math::{
    flush_level_price, level_fill_atoms, sort_key, taker_fee_atoms,
};

use crate::{
    errors::DropsetError,
    events::FillEvent,
    state::{Market, VaultAccess, FLUSH_BIT},
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

    // ── Side-keyed matching helpers ──────────────────────────────────
    //
    // The `swap` matching loop and settlement branch on `Buy`/`Sell` in
    // ~a dozen places — level selection, sort key, limit-cross test,
    // fill math, fee leg, snapshot, inventory mutation, taker
    // accounting. These `#[inline]` helpers fold every such decision
    // back to a single `match` so the Buy and Sell paths can't drift out
    // of lockstep: change a matching rule once here and both sides move
    // together. They're inlined and branch on the same runtime `side`
    // the caller already holds, so they add no per-leg dispatch cost
    // over the original inline matches — the CU profile is unchanged.

    /// A `Buy` taker consumes the resting **ask** side (pays quote, gets
    /// base); a `Sell` taker consumes the **bid** side (pays base, gets
    /// quote). This single predicate drives level selection, the
    /// snapshot's `is_ask_side`, and which `remaining[*]` slot the fill
    /// decrements.
    #[inline]
    fn consumes_asks(self) -> bool {
        matches!(self, SwapSide::Buy)
    }

    /// Up-front limit-price sentinel check. `Buy` rejects `ZERO` (it
    /// would reject every ask — a likely caller mistake); `Sell` rejects
    /// `INFINITY` symmetrically. Each accepts the open-ended sentinel for
    /// its own side and any regular price.
    #[inline]
    fn limit_price_ok(self, limit: Price) -> bool {
        match self {
            SwapSide::Buy => !limit.is_zero(),
            SwapSide::Sell => !limit.is_infinity(),
        }
    }

    /// Sort key for the matching heap: asks order by raw `as_u32()`
    /// (cheapest ask fills first); bids order by `bid_key()` (highest
    /// bid fills first). Combined with `(nonce, sector_idx, level_idx)`
    /// this yields the spec's price-time priority from a single sort.
    #[inline]
    fn price_sort_key(self, price: Price) -> u32 {
        sort_key(price, self.consumes_asks())
    }

    /// True when `price` is worse than the taker's `limit`, so the leg
    /// must be skipped. Because `levels` is sorted best-first, the first
    /// crossing leg means every later one crosses too, so the caller
    /// `break`s. A `Buy` crosses when the ask exceeds the limit; a
    /// `Sell` crosses when the bid falls below it. The open-ended
    /// sentinel for the side never crosses.
    #[inline]
    fn crosses_limit(self, price: Price, limit: Price) -> bool {
        match self {
            SwapSide::Buy => price.as_u32() > limit.as_u32() && !limit.is_infinity(),
            SwapSide::Sell => price.as_u32() < limit.as_u32() && !limit.is_zero(),
        }
    }

    /// The leg the taker **receives** and the vault pays out: base on a
    /// Buy, quote on a Sell. The taker fee is charged on this leg and
    /// `total_out` accumulates it.
    #[inline]
    fn output_atoms(self, fill_base: u64, fill_quote: u64) -> u64 {
        match self {
            SwapSide::Buy => fill_base,
            SwapSide::Sell => fill_quote,
        }
    }

    /// The leg the taker **pays in** and the vault books: quote on a
    /// Buy, base on a Sell. Decrements `taker_unfilled_in`.
    #[inline]
    fn input_atoms(self, fill_base: u64, fill_quote: u64) -> u64 {
        match self {
            SwapSide::Buy => fill_quote,
            SwapSide::Sell => fill_base,
        }
    }

    /// Compute the `(fill_base, fill_quote)` for one matched leg, or
    /// `Ok(None)` when it fills zero (the caller `continue`s). The Buy
    /// and Sell arms are mirror images: size the **output** leg the
    /// taker receives as `min(taker input converted to output, level
    /// cap, vault output inventory)`, reverse-convert to the **input**
    /// leg, then apply two guards.
    ///
    /// * **1d (overflow):** guard the reverse-converted input leg
    ///   against `u64::MAX` and reject explicitly rather than silently
    ///   truncate — a clamped input would debit the taker (and book the
    ///   vault) less than the price implies, shattering the treasury
    ///   invariant. Defense-in-depth: the output leg is already capped
    ///   by `base_for_quote(taker_in)` / `quote_for_base(taker_in)`
    ///   (the 1c cap), so the floor round-trip bounds the reverse leg at
    ///   `<= taker_in <= u64::MAX` and this `require!` cannot fire on a
    ///   valid price today. Kept so an overflow stays a hard abort if
    ///   the cap is ever weakened — and so the off-chain simulator's
    ///   matching guard has an on-chain counterpart to mirror.
    /// * **1c (round-trip cap):** the decoders truncate toward zero in
    ///   both directions, so `out→in→` can exceed the taker's remaining
    ///   input by a few atoms. Capping the input leg at
    ///   `taker_unfilled_in` keeps the per-leg vault credit equal to
    ///   what the taker actually pays and stops `taker_unfilled_in` from
    ///   saturating to 0 and billing the full budget for a partial fill.
    #[inline]
    fn compute_fill(
        self,
        price: Price,
        taker_unfilled_in: u128,
        level_size: u64,
        base_atoms: u64,
        quote_atoms: u64,
    ) -> Result<Option<(u64, u64)>> {
        let taker_in = taker_unfilled_in.min(u64::MAX as u128) as u64;
        match self {
            SwapSide::Buy => {
                // level.size is in base; convert the taker's quote
                // budget to base via the level price, then take the
                // tightest of the taker, level, and vault-base caps.
                let fill_b = price
                    .base_for_quote(taker_in)
                    .min(level_size as u128)
                    .min(base_atoms as u128);
                if fill_b == 0 {
                    return Ok(None);
                }
                let fill_b_u64 = fill_b.min(u64::MAX as u128) as u64;
                let fill_q = price.quote_for_base(fill_b_u64);
                require!(fill_q <= u64::MAX as u128, DropsetError::MathOverflow);
                let fill_q = fill_q.min(taker_unfilled_in);
                Ok(Some((fill_b_u64, fill_q as u64)))
            }
            SwapSide::Sell => {
                // level.size is in quote; convert the taker's unfilled
                // base to quote via the level price, then take the
                // tightest of the taker, level, and vault-quote caps.
                let fill_q = price
                    .quote_for_base(taker_in)
                    .min(level_size as u128)
                    .min(quote_atoms as u128);
                if fill_q == 0 {
                    return Ok(None);
                }
                let fill_q_u64 = fill_q.min(u64::MAX as u128) as u64;
                let fill_b = price.base_for_quote(fill_q_u64);
                require!(fill_b <= u64::MAX as u128, DropsetError::MathOverflow);
                let fill_b = fill_b.min(taker_unfilled_in);
                Ok(Some((fill_b as u64, fill_q_u64)))
            }
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

/// `level.size_bps` × the matching leg, in atoms — the program-side
/// wrapper over [`level_fill_atoms`] that maps its `size_bps > BPS`
/// rejection to a hard `DropsetError`.
///
/// `set_liquidity_profile` bounds the per-side Σ `size_bps` to `BPS`, and
/// each `size_bps` is a non-negative `u16`, so every *individual* level is
/// `<= BPS` for any profile written through that path — the rejection is
/// implied by the sum check and never fires on the normal path. It is
/// load-bearing only against account bytes the program never wrote
/// (corruption, or a future profile-writing instruction that skips the sum
/// check). We reject rather than silently clamp, which would mask the bug
/// by shrinking the level's materialized size. The pricing
/// (`flush_level_price`) and the cap itself are shared with the off-chain
/// simulator via [`dropset_math_core::matching_math`] so the two can't
/// drift; the simulator pins the same contract from the other direction —
/// on `size_bps > BPS` it yields an empty quote instead of a fill this
/// handler would abort (conformance test in `tests/sdk_conformance.rs`).
fn flush_level_size(size_bps: u16, leg_atoms: u64) -> Result<u64> {
    level_fill_atoms(size_bps, leg_atoms)
        .ok_or_else(|| DropsetError::LiquidityProfileSizeOverflow.into())
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
    /// Materialized level size at heap-build time, used directly as
    /// the per-leg level cap (`cap_by_level`) in the fill loop. Each
    /// `(vault, level)` pair is pushed once and visited once, so the
    /// snapshot needs no re-read; only the vault inventory
    /// (`base_atoms`/`quote_atoms`) is re-read per leg to reflect
    /// prior-leg decrements.
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
        require!(
            side.limit_price_ok(limit_price),
            DropsetError::InvalidLimitPrice
        );
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
                // `cur` was just range-checked above (CorruptVaultList),
                // so the accessor's own bounds check never fires here —
                // it is the same named borrow the slab index performed.
                let v = self.market.read_vault(cur)?;
                v.next.get()
            };
            // Read vault meta + decide whether to flush. A vault on
            // the active DLL must have a non-default leader by
            // construction (free-list sectors live elsewhere). An
            // invalid / sentinel reference price skips the vault
            // (rather than aborting) — spec § Order matching → Book
            // construction.
            let (vault_active, stamp, ref_price, ref_slot, base_atoms, quote_atoms) = {
                let v = self.market.read_vault(cur)?;
                let p = v.reference_price.price;
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
                    v.has_valid_reference_price() && !frozen,
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
                let v = self.market.mutate_vault(cur)?;
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
                let v = self.market.read_vault(cur)?;
                for i in 0..N_LEVELS {
                    let lvl = if side.consumes_asks() {
                        v.remaining.asks[i]
                    } else {
                        v.remaining.bids[i]
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
                    let price_key = side.price_sort_key(price);
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
        // Set if a per-leg `market.nonce` bump would overflow `u64`.
        // Practically unreachable (the spec sizes `nonce` to never wrap
        // over a market's lifetime), but we reject it as a hard error
        // to stay consistent with the quote paths
        // (`set_reference_price.rs`, `set_liquidity_profile.rs`), which
        // both `checked_add(1).ok_or(MathOverflow)?`. The shared revert
        // block below restores the pre-swap state before we error, so a
        // wrap leaves no partial fill behind.
        let mut nonce_overflow = false;

        for &(sector_idx, level_idx, price, level_size, _nonce) in levels.iter().take(live_count) {
            // Limit-price filter — also the review's "early-exit when
            // the best level can't fill" gate (WARNING 1a). `levels`
            // is sorted best-price-first, so the first level that
            // crosses the taker's limit means every remaining level
            // crosses too: `break` bails before any snapshot or fill
            // work, and the `filled_legs == 0` path below re-arms
            // FLUSH_BIT and returns an empty result. (The INF-limit /
            // MAX-`min_out` soft-revert that crosses zero levels and
            // walks the whole book is deliberately not metered: the
            // walk is bounded by `market.len()`, the revert restores
            // inventory and level sizes and re-arms FLUSH_BIT so no
            // book state advances, and the caller pays its full CU +
            // base/priority fees, so the spam is self-funded. A
            // protocol fee or per-slot cooldown would tax honest
            // price-moved no-fill
            // takers identically and still not address the only
            // residual — Market write-lock contention, which is
            // architectural, not a per-swap concern. Accepted risk.)
            if side.crosses_limit(price, limit_price) {
                break;
            }
            if taker_unfilled_in == 0 {
                break;
            }

            // Snapshot the matched vault's current inventory — each
            // leg debits/credits it, so we read fresh.
            let (base_atoms, quote_atoms) = {
                let v = self.market.read_vault(sector_idx)?;
                (v.base_atoms.get(), v.quote_atoms.get())
            };

            // Size the leg (mirror-symmetric across sides; see
            // `compute_fill`). A zero fill skips the leg entirely.
            let (fill_base, fill_quote): (u64, u64) = match side.compute_fill(
                price,
                taker_unfilled_in,
                level_size,
                base_atoms,
                quote_atoms,
            )? {
                Some(fill) => fill,
                None => continue,
            };

            // Apply taker fee on the *output* leg (base on a Buy, quote
            // on a Sell), retained in the matched vault for the
            // depositors' benefit.
            let fee = taker_fee_atoms(
                side.output_atoms(fill_base, fill_quote),
                taker_fee_ppm as u128,
            );
            let fee_u64 = fee.min(u64::MAX as u128) as u64;

            // Snapshot the pre-leg state so the `min_out` soft-revert
            // at the end can roll back every mutation cleanly. The
            // consumed side (asks on a Buy, bids on a Sell) is the only
            // level array this leg touches, so it's the only one to
            // record.
            let is_ask_side = side.consumes_asks();
            {
                let v = self.market.read_vault(sector_idx)?;
                let size_before = if is_ask_side {
                    v.remaining.asks[level_idx as usize].size.get()
                } else {
                    v.remaining.bids[level_idx as usize].size.get()
                };
                snapshots.push(LegSnapshot {
                    sector_idx,
                    level_idx,
                    is_ask_side,
                    base_before: v.base_atoms.get(),
                    quote_before: v.quote_atoms.get(),
                    size_before,
                });
            }

            // Update vault inventory and the consumed level's remaining
            // size, written once in input/output terms. The fee is
            // retained in the vault on the output leg, so the output
            // inventory debit is `output - fee` (`net_out`), matching
            // what the treasury actually sends the taker; the input
            // inventory is credited the full input leg. This keeps the
            // treasury-vs-vault invariant `treasury.amount == Σ
            // vault.<leg>_atoms` holding per leg. The level's remaining
            // size shrinks by the gross output fill (pre-fee), since the
            // fee slice was still liquidity the taker consumed.
            let output_atoms = side.output_atoms(fill_base, fill_quote);
            let input_atoms = side.input_atoms(fill_base, fill_quote);
            let net_output_out = output_atoms.saturating_sub(fee_u64);
            let (new_base, new_quote) = {
                let v = self.market.mutate_vault(sector_idx)?;
                // On a Buy the output leg is base and the input leg is
                // quote; on a Sell the legs swap. `is_ask_side` (Buy)
                // selects which.
                let (b, q) = if is_ask_side {
                    (
                        v.base_atoms.get().saturating_sub(net_output_out),
                        v.quote_atoms.get().saturating_add(input_atoms),
                    )
                } else {
                    (
                        v.base_atoms.get().saturating_add(input_atoms),
                        v.quote_atoms.get().saturating_sub(net_output_out),
                    )
                };
                v.base_atoms = b.into();
                v.quote_atoms = q.into();
                let level = if is_ask_side {
                    &mut v.remaining.asks[level_idx as usize]
                } else {
                    &mut v.remaining.bids[level_idx as usize]
                };
                level.size = level.size.get().saturating_sub(output_atoms).into();
                (b, q)
            };

            // Bump market.nonce per leg (header borrow after the tail
            // mutation completes). Reject an overflowing bump the same
            // way the quote paths do; the current leg's inventory
            // mutation is already snapshotted above, so breaking here
            // lets the shared revert block roll it (and every prior
            // leg) back before we return the error.
            let nonce = self.market.nonce.get();
            let Some(new_nonce) = nonce.checked_add(1) else {
                nonce_overflow = true;
                break;
            };
            self.market.nonce = new_nonce.into();

            // Decrement the taker's remaining input by the input leg
            // and accumulate the output leg they receive.
            taker_unfilled_in = taker_unfilled_in.saturating_sub(input_atoms as u128);
            total_out += output_atoms as u128;
            total_fee = total_fee.saturating_add(fee);
            filled_legs = filled_legs.saturating_add(1);

            // Emit one event per matched (vault, level) leg.
            let (leader, quote_authority) = {
                let v = self.market.read_vault(sector_idx)?;
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
        if nonce_overflow || filled_legs == 0 || achievable_net_out < min_out {
            // Walk snapshots in reverse so two legs that touched the
            // same sector's inventory restore to the earliest
            // captured value.
            for snap in snapshots.iter().rev() {
                let v = self.market.mutate_vault(snap.sector_idx)?;
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
                let v = self.market.mutate_vault(sector_idx)?;
                let cur = v.reference_price.stamp.get();
                v.reference_price.stamp = (cur | FLUSH_BIT).into();
            }
            self.market.nonce = nonce_at_start.into();
            // A nonce overflow is a hard error, not a soft revert: the
            // state is fully restored above, but the swap could not be
            // applied, so surface it like the quote paths do rather
            // than silently returning an empty fill.
            if nonce_overflow {
                return Err(DropsetError::MathOverflow.into());
            }
            return Ok(alloc::vec::Vec::new());
        }

        // Net taker transfer: pay the input leg in, receive the output
        // leg out. Both legs are aggregated across all matched levels
        // — one SPL transfer per side. The atom counts are side-agnostic
        // here (input = budget consumed, output = `total_out`); only the
        // CPI account selection below is keyed on `side`.
        let taker_in_atoms = (amount_in as u128 - taker_unfilled_in) as u64;
        let taker_out_atoms = total_out as u64;

        // Input leg: taker → treasury. The two settlement CPIs (this
        // and the output leg below) are kept as explicit `match side`
        // arms rather than folded behind a helper: each arm selects a
        // distinct set of token accounts (quote vs base ATA, mint,
        // treasury, program), so the duplication is account wiring, not
        // matching logic, and is clearest read inline. The Buy and Sell
        // arms here MUST stay mirror images — Buy pays quote in / base
        // out, Sell pays base in / quote out.
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

#[cfg(test)]
mod overflow_bound_tests {
    //! Pin the invariant that makes `compute_fill`'s two `u128`→`u64`
    //! `MathOverflow` guards (WARNING 1d) unreachable.
    //!
    //! Each side sizes the taker's **output** leg first — capped by the
    //! taker's converted budget, the level, and the vault — then
    //! reverse-converts to the **input** leg and guards it against
    //! `u64::MAX`. Because the output leg is capped by
    //! `quote_for_base(taker_in)` / `base_for_quote(taker_in)` (the 1c
    //! cap) and both decoders truncate toward zero, the reverse leg
    //! round-trips back to `<= taker_in <= u64::MAX` — so neither guard
    //! can fire on any reachable state, for either side. `quote_for_base`
    //! also never saturates for a `u64` input (max exponent gives a
    //! `10^8` factor, so the widest product is `~1.8e35 < u128::MAX`), so
    //! the round-trip bound has no saturation escape hatch.
    //!
    //! The guards are therefore defense-in-depth: kept so an overflow
    //! stays a hard abort if the cap is ever weakened. These tests pin
    //! the bound so such a regression (or a switch to round-half-up in
    //! the decoders) trips a fast local unit test instead of shipping a
    //! live overflow path. The second test nails the exact case the
    //! hardening review suspected could overflow — a tiny-price `Sell`
    //! with the taker's base budget at `u64::MAX` — and shows it doesn't.

    use super::SwapSide;
    use crate::Price;

    /// Valid prices spanning the full exponent range, chosen to stress
    /// both the tiny-price direction (base blows up on `base_for_quote`)
    /// and the large-price direction (quote blows up on `quote_for_base`).
    fn stress_prices() -> [Price; 8] {
        [
            Price::encode(10_000_000, -16).unwrap(), // ~1e-16, smallest
            Price::encode(99_999_999, -16).unwrap(), // ~1e-15
            Price::encode(10_000_000, -2).unwrap(),  // 0.01
            Price::encode(10_000_000, 0).unwrap(),   // 1.0
            Price::encode(10_850_000, 0).unwrap(),   // 1.0850 (FX)
            Price::encode(98_700_000, 2).unwrap(),   // 987
            Price::encode(10_000_000, 15).unwrap(),  // 1e15
            Price::encode(99_999_999, 15).unwrap(),  // ~1e16, largest
        ]
    }

    /// `taker_unfilled_in` values, including the `u64::MAX` extreme and a
    /// `u128` above it to exercise the internal `min(u64::MAX)` clamp.
    const TAKER_INS: [u128; 6] = [
        1,
        1_000_000,
        7_777_777_777,
        u64::MAX as u128 - 1,
        u64::MAX as u128,
        u64::MAX as u128 + 999,
    ];

    /// Level-size and vault-inventory caps, spanning both extremes so the
    /// binding cap is sometimes the taker, sometimes the level, sometimes
    /// the vault inventory.
    const CAPS: [u64; 4] = [1, 1_000_000, u64::MAX / 2, u64::MAX];

    #[test]
    fn compute_fill_never_overflows_for_any_valid_price() {
        for side in [SwapSide::Buy, SwapSide::Sell] {
            for price in stress_prices() {
                for &taker_in in &TAKER_INS {
                    for &level_size in &CAPS {
                        for &inv in &CAPS {
                            let got = side.compute_fill(price, taker_in, level_size, inv, inv);
                            assert!(
                                got.is_ok(),
                                "compute_fill overflowed: side={side:?} price={price:?} \
                                 taker_in={taker_in} level_size={level_size} inv={inv}"
                            );
                            // On a fill, the input leg the taker pays must
                            // never exceed their remaining budget — the 1c
                            // cap that keeps the reverse leg under u64::MAX.
                            if let Ok(Some((fill_base, fill_quote))) = got {
                                let input = side.input_atoms(fill_base, fill_quote);
                                assert!(
                                    input as u128 <= taker_in,
                                    "input leg {input} exceeded taker budget {taker_in}: \
                                     side={side:?} price={price:?}"
                                );
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn pathological_tiny_price_sell_at_max_base_does_not_overflow() {
        // The exact case the hardening review flagged: a `Sell` of
        // `u64::MAX` base into a bottomless bid at the smallest
        // representable price. `quote_for_base(u64::MAX)` at ~1e-16 is
        // only ~1844, so `base_for_quote(1844)` rounds back to ~1.844e19
        // < u64::MAX and the guard stays dormant.
        let price = Price::encode(10_000_000, -16).unwrap();
        let got =
            SwapSide::Sell.compute_fill(price, u64::MAX as u128, u64::MAX, u64::MAX, u64::MAX);
        assert!(
            got.is_ok(),
            "tiny-price max-base Sell must not overflow the fill-leg guard"
        );
        if let Ok(Some((fill_base, _))) = got {
            assert!(fill_base <= u64::MAX, "reverse-converted base leg within u64");
        }
    }
}
