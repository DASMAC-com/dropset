//! `swap` (spec's `Take`) — taker fills a single vault's side of the
//! ephemeral book.
//!
//! MVP shape: caller picks the `vault_idx` to match against; the
//! handler iterates that vault's [`Vault::remaining`] for the chosen
//! side (asks for Buy, bids for Sell), flushes from
//! [`Vault::profile`] if `FLUSH_BIT` is armed, and fills the taker
//! against levels in their stored order. Cross-vault aggregation —
//! the heap-based price-time-priority match the spec describes —
//! lands in a follow-up: with `registry.max_vaults_per_market = 10`
//! the single-vault path covers the MVP launch (one leader, one
//! vault) and the heap upgrade is purely additive.
//!
//! Per spec § **Events and emission**, every filled `(vault, level)`
//! leg emits one `FillEvent` via `emit!` (default log path for MVP;
//! `emit_cpi!` upgrade lands with the same follow-up that wires up
//! `#[event_cpi]` on this handler). No truncation: the loop emits as
//! it fills.

use anchor_lang_v2::{address_eq, prelude::*};
#[allow(unused_imports)]
use anchor_spl_v2::{
    associated_token::AssociatedToken,
    token_2022::{transfer_checked, TransferChecked},
    token_interface::{Mint, TokenAccount, TokenInterface},
};

use crate::{
    errors::DropsetError,
    events::{FillEvent, SwapSide},
    state::{Market, BPS, FLUSH_BIT, PPM},
    Price, N_LEVELS,
};

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
fn flush_level_size(size_bps: u16, leg_atoms: u64) -> u64 {
    (((leg_atoms as u128) * (size_bps as u128)) / (BPS as u128)).min(u64::MAX as u128) as u64
}

impl Swap {
    pub fn swap(
        &mut self,
        vault_idx: u32,
        side_u8: u8,
        amount_in: u64,
        limit_price_bits: u32,
    ) -> Result<()> {
        let side = SwapSide::from_u8(side_u8).ok_or(DropsetError::InvalidPrice)?;
        let limit_price = Price::from_bits(limit_price_bits);
        require!(limit_price.is_valid(), DropsetError::InvalidPrice);
        require!(amount_in > 0, DropsetError::NothingFilled);

        let len = self.market.len();
        require!((vault_idx as usize) < len, DropsetError::InvalidSectorIndex);

        // Snapshot vault meta + flush if FLUSH_BIT armed.
        let market_addr = *self.market.address();
        let market_bump = self.market.bump;
        let base_mint_addr = self.market.base_mint;
        let quote_mint_addr = self.market.quote_mint;
        let taker_fee_ppm = self.market.taker_fee.get() as u64;
        let current_slot = self.clock.slot as u32;

        {
            // Flush + sanity-check the vault's reference price.
            let v = &mut self.market.as_mut_slice()[vault_idx as usize];
            require!(
                !address_eq(&v.leader, &Address::default()),
                DropsetError::VaultEmpty
            );
            require!(
                v.reference_price.price.is_valid()
                    && !v.reference_price.price.is_zero()
                    && !v.reference_price.price.is_infinity(),
                DropsetError::InvalidPrice
            );
            let stamp = v.reference_price.stamp.get();
            if stamp & FLUSH_BIT != 0 {
                let reference = v.reference_price.price;
                let quote_slot_base = v.reference_price.quote_slot.get();
                let base_atoms = v.base_atoms.get();
                let quote_atoms = v.quote_atoms.get();
                for i in 0..N_LEVELS {
                    let bid = v.profile.bids[i];
                    let ask = v.profile.asks[i];
                    v.remaining.bids[i].price =
                        flush_level_price(reference, bid.price_offset.get(), false);
                    v.remaining.bids[i].size =
                        flush_level_size(bid.size_bps.get(), quote_atoms).into();
                    v.remaining.bids[i].expires_at = quote_slot_base
                        .saturating_add(bid.expiry_offset.get())
                        .into();
                    v.remaining.asks[i].price =
                        flush_level_price(reference, ask.price_offset.get(), true);
                    v.remaining.asks[i].size =
                        flush_level_size(ask.size_bps.get(), base_atoms).into();
                    v.remaining.asks[i].expires_at = quote_slot_base
                        .saturating_add(ask.expiry_offset.get())
                        .into();
                }
                v.reference_price.stamp = (stamp & !FLUSH_BIT).into();
            }
        }

        // Build a heap-ish view: collect (level_idx, price, size,
        // expires_at) tuples for the side, filter expired / zero-size /
        // sentinel, and sort by price.
        let mut levels: [(u32, Price, u64, u32); N_LEVELS] = [(0, Price::ZERO, 0, 0); N_LEVELS];
        let mut live_count: usize = 0;
        {
            let v = &self.market.as_slice()[vault_idx as usize];
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
                levels[live_count] = (i as u32, price, size, expires_at);
                live_count += 1;
            }
        }
        // Sort active levels: asks ascending by price, bids
        // descending (= ascending by `bid_key`). Ties broken by
        // level_idx ascending — surrogate for nonce in MVP, since a
        // single vault's levels share its stamp.
        match side {
            SwapSide::Buy => levels[..live_count].sort_by_key(|t| (t.1.as_u32(), t.0)),
            SwapSide::Sell => levels[..live_count].sort_by_key(|t| (t.1.bid_key(), t.0)),
        }

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

        for &(level_idx, price, level_size, _exp) in levels.iter().take(live_count) {
            // Limit-price filter.
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

            // Decode price to a scaled u64 ratio, conservatively. We
            // approximate by reusing the significand × 10^(exp - 7);
            // for atom-scale arithmetic at FX-style prices (~1.0 to
            // ~100) this is accurate. A full decoder lands with the
            // bytemuck heap upgrade.
            let sig = price.significand() as u128;
            let unb = price.unbiased_exponent() as i32 - 7;
            // price_scaled ≈ sig × 10^unb, in fixed quote-per-base units
            // scaled by 10^9 so atom math stays integral on typical
            // FX values. Cap negative shifts so we never divide by
            // zero downstream.
            let mut price_num: u128 = sig;
            let mut price_den: u128 = 1;
            if unb >= 0 {
                for _ in 0..unb {
                    price_num = price_num.saturating_mul(10);
                }
            } else {
                for _ in 0..(-unb) {
                    price_den = price_den.saturating_mul(10);
                }
            }
            if price_num == 0 || price_den == 0 {
                continue;
            }

            // Snapshot the vault's current inventory inside the fill
            // loop — each leg debits/credits it, so we read fresh.
            let (base_atoms, quote_atoms) = {
                let v = &self.market.as_slice()[vault_idx as usize];
                (v.base_atoms.get(), v.quote_atoms.get())
            };

            let (fill_base, fill_quote): (u64, u64) = match side {
                SwapSide::Buy => {
                    // level.size is in base; convert taker's quote
                    // budget to base via the level price.
                    let cap_by_taker_quote = (taker_unfilled_in * price_den) / price_num;
                    let cap_by_level = level_size as u128;
                    let cap_by_vault = base_atoms as u128;
                    let fill_b = cap_by_taker_quote.min(cap_by_level).min(cap_by_vault);
                    if fill_b == 0 {
                        continue;
                    }
                    let fill_q = (fill_b * price_num) / price_den;
                    (fill_b as u64, fill_q as u64)
                }
                SwapSide::Sell => {
                    // level.size is in quote; convert taker's base to
                    // quote via the level price.
                    let taker_implied_quote = (taker_unfilled_in * price_num) / price_den;
                    let cap_by_level = level_size as u128;
                    let cap_by_vault = quote_atoms as u128;
                    let fill_q = taker_implied_quote.min(cap_by_level).min(cap_by_vault);
                    if fill_q == 0 {
                        continue;
                    }
                    let fill_b = (fill_q * price_den) / price_num;
                    (fill_b as u64, fill_q as u64)
                }
            };

            // Apply taker fee on the *output* leg, retained in the
            // matched vault for the depositors' benefit.
            let fee = match side {
                SwapSide::Buy => ((fill_base as u128) * (taker_fee_ppm as u128)) / (PPM as u128),
                SwapSide::Sell => ((fill_quote as u128) * (taker_fee_ppm as u128)) / (PPM as u128),
            };
            let fee_u64 = fee.min(u64::MAX as u128) as u64;

            // Update vault inventory and level remaining size. The
            // fee is retained in the vault on the output leg — so the
            // inventory debit is `fill_<out> - fee_u64`, matching the
            // `net_out` actually transferred to the taker. This keeps
            // the treasury-vs-vault invariant
            // `treasury.amount == Σ vault.<leg>_atoms` holding per
            // leg: treasury sends `net_out`, vault books `-(net_out)`.
            let (new_base, new_quote) = {
                let v = &mut self.market.as_mut_slice()[vault_idx as usize];
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
                let v = &self.market.as_slice()[vault_idx as usize];
                (v.leader, v.quote_authority)
            };
            emit!(FillEvent {
                market: market_addr,
                taker: *self.taker.address(),
                leader,
                quote_authority,
                side: side_u8,
                _pad: [0; 7],
                sector_idx: vault_idx,
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

        require!(filled_legs > 0, DropsetError::NothingFilled);

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
        Ok(())
    }
}
