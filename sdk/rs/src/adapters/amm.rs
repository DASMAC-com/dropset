//! Router-agnostic AMM core.
//!
//! The Jupiter / DFlow / Titan integration traits all want the same four
//! things from a venue: build from an account, list accounts to refresh,
//! quote off-chain, and emit a swap instruction. [`DropsetAmm`] provides
//! exactly that against the Dropset SDK's own types — book state via
//! [`crate::layout`], quotes via [`crate::matching::simulate_swap`], and
//! the swap instruction via the generated [`crate::instructions`] builder.
//!
//! Each router module ([`super::dflow`], [`super::jupiter`],
//! [`super::titan`]) maps its trait onto this core and documents the
//! per-router boundary (solana-sdk version skew, closed `Swap` enums).
//!
//! **No network calls.** Every method operates on the cached account bytes
//! passed to [`DropsetAmm::from_account`] / [`DropsetAmm::update`] — Jupiter
//! batches and caches the accounts from `get_accounts_to_update` and may
//! call `quote` many times against one cache, so its integration contract
//! forbids any I/O in the implementation. This core satisfies that.

use solana_instruction::Instruction;
use solana_pubkey::Pubkey;

use crate::generated::instructions::{Swap as SwapAccounts, SwapInstructionArgs};
use crate::layout::{LayoutError, MarketView};
use crate::matching::{simulate_swap, SwapSide};
use crate::price::Price;
use crate::DROPSET_ID;

/// The clock sysvar — read by `swap` for the current slot (expiry checks).
pub const CLOCK_SYSVAR: Pubkey =
    solana_pubkey::pubkey!("SysvarC1ock11111111111111111111111111111111");

/// Human-readable venue label (`Amm::label` / `TradingVenue` name).
pub const LABEL: &str = "Dropset";

/// Account count of the `swap` instruction — the routers' `get_accounts_len`.
/// Constant: the whole book is one market account, so a take never grows
/// its account list (interface.md § 4, "not account-hungry").
pub const SWAP_ACCOUNTS_LEN: usize = 13;

/// A Dropset market presented through a router's quoting + swap surface.
#[derive(Clone)]
pub struct DropsetAmm {
    market_key: Pubkey,
    /// Cached market account data (including the 8-byte discriminator).
    data: Vec<u8>,
    base_mint: Pubkey,
    quote_mint: Pubkey,
    base_treasury: Pubkey,
    quote_treasury: Pubkey,
}

/// A quote request (atoms in `amount`). Mirrors the routers' quote-params
/// structs (Jupiter/DFlow `QuoteParams`, Titan `QuoteRequest`).
#[derive(Clone, Copy, Debug)]
pub struct DropsetQuoteParams {
    pub input_mint: Pubkey,
    pub output_mint: Pubkey,
    pub amount: u64,
    /// Current slot, for per-level expiry filtering. The router supplies
    /// this from its clock cache (e.g. Jupiter's `AmmContext.clock_ref`).
    pub current_slot: u32,
}

/// A quote result. The router traits return `{ in_amount, out_amount }`;
/// `fee_amount` is exposed too for fee-aware routing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DropsetQuote {
    pub in_amount: u64,
    pub out_amount: u64,
    pub fee_amount: u64,
}

/// Accounts + args for a swap. Mirrors the routers' swap-params structs.
#[derive(Clone, Copy, Debug)]
pub struct DropsetSwapParams {
    pub taker: Pubkey,
    pub source_mint: Pubkey,
    pub destination_mint: Pubkey,
    pub taker_base_ata: Pubkey,
    pub taker_quote_ata: Pubkey,
    pub base_token_program: Pubkey,
    pub quote_token_program: Pubkey,
    pub amount_in: u64,
    /// Worst acceptable fill (raw `Price` bits). Use `Price::INFINITY` for
    /// a buy / `Price::ZERO` for a sell to disable the bound.
    pub limit_price_bits: u32,
    pub min_out: u64,
}

/// Token metadata for a leg — Titan's `TokenInfo` / Jupiter's reserve
/// mints. Decimals live on the mint accounts, which the router fetches
/// separately; this carries the mint + its token program.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LegInfo {
    pub mint: Pubkey,
    /// The treasury (pooled custody) ATA for this leg.
    pub treasury: Pubkey,
}

/// Errors building or quoting a [`DropsetAmm`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AmmError {
    Layout(LayoutError),
    /// The `(input_mint, output_mint)` pair doesn't match this market's legs.
    MintMismatch,
}

impl From<LayoutError> for AmmError {
    fn from(e: LayoutError) -> Self {
        AmmError::Layout(e)
    }
}

impl DropsetAmm {
    /// Build from a market account's key + data (`from_keyed_account` /
    /// `from_account`).
    pub fn from_account(market_key: Pubkey, data: &[u8]) -> Result<Self, AmmError> {
        let view = MarketView::load(data)?;
        let h = view.header;
        Ok(Self {
            market_key,
            data: data.to_vec(),
            base_mint: Pubkey::from(h.base_mint),
            quote_mint: Pubkey::from(h.quote_mint),
            base_treasury: Pubkey::from(h.base_treasury),
            quote_treasury: Pubkey::from(h.quote_treasury),
        })
    }

    /// The Dropset program id (`program_id`).
    pub fn program_id(&self) -> Pubkey {
        DROPSET_ID
    }

    /// The market account address (`key`).
    pub fn key(&self) -> Pubkey {
        self.market_key
    }

    pub fn base_mint(&self) -> Pubkey {
        self.base_mint
    }

    pub fn quote_mint(&self) -> Pubkey {
        self.quote_mint
    }

    /// Reserve mints `[base, quote]` (`get_reserve_mints`).
    pub fn reserve_mints(&self) -> Vec<Pubkey> {
        vec![self.base_mint, self.quote_mint]
    }

    /// Per-leg info `[base, quote]` (Titan `get_token_info`).
    pub fn leg_info(&self) -> [LegInfo; 2] {
        [
            LegInfo {
                mint: self.base_mint,
                treasury: self.base_treasury,
            },
            LegInfo {
                mint: self.quote_mint,
                treasury: self.quote_treasury,
            },
        ]
    }

    /// Accounts to refresh before quoting (`get_accounts_to_update` /
    /// `update_state`). The whole book lives in the single market account,
    /// so quoting only needs that one refreshed (interface.md § 4: the
    /// take is "not account-hungry").
    pub fn accounts_to_update(&self) -> Vec<Pubkey> {
        vec![self.market_key]
    }

    /// Venue label (`Amm::label`).
    pub fn label(&self) -> &'static str {
        LABEL
    }

    /// Swap-instruction account count (`Amm::get_accounts_len`).
    pub fn accounts_len(&self) -> usize {
        SWAP_ACCOUNTS_LEN
    }

    /// The program quotes exact-in only; ExactOut is unsupported
    /// (`Amm::supports_exact_out`).
    pub fn supports_exact_out(&self) -> bool {
        false
    }

    /// The book trades both directions (`Amm::unidirectional` -> false).
    pub fn unidirectional(&self) -> bool {
        false
    }

    /// Whether the market can currently trade (`Amm::is_active`): at least
    /// one active vault with a valid, non-sentinel reference price that
    /// isn't frozen — the same gate the matcher applies.
    pub fn is_active(&self) -> bool {
        let Ok(view) = MarketView::load(&self.data) else {
            return false;
        };
        view.active_vaults().any(|(_, v)| {
            let p = v.reference_price.price();
            v.frozen == 0 && p.is_valid() && !p.is_zero() && !p.is_infinity()
        })
    }

    /// Refresh the cached market account data (`update`).
    pub fn update(&mut self, data: &[u8]) -> Result<(), AmmError> {
        // Validate it decodes before swapping the cache in.
        MarketView::load(data)?;
        self.data = data.to_vec();
        Ok(())
    }

    /// Quote a take against the current book (`quote`). Zero-input-safe
    /// (Titan requires this): `amount == 0` yields an empty quote.
    pub fn quote(&self, p: &DropsetQuoteParams) -> Result<DropsetQuote, AmmError> {
        let side = self.side_for(p.input_mint, p.output_mint)?;
        let view = MarketView::load(&self.data)?;
        // No slippage bound at quote time — the no-bound sentinel per side.
        let limit = match side {
            SwapSide::Buy => Price::INFINITY,
            SwapSide::Sell => Price::ZERO,
        };
        let q = simulate_swap(&view, side, p.amount, limit, p.current_slot);
        Ok(DropsetQuote {
            in_amount: q.in_amount,
            out_amount: q.out_amount,
            fee_amount: q.fee_amount,
        })
    }

    /// Build the `swap` instruction (`get_swap_and_account_metas` /
    /// `generate_swap_instruction`). Returns the ready-to-submit
    /// `solana_instruction::Instruction`; `.accounts` are the metas.
    pub fn swap_instruction(&self, p: &DropsetSwapParams) -> Result<Instruction, AmmError> {
        let side = self.side_for(p.source_mint, p.destination_mint)?;
        let (event_authority, _) =
            Pubkey::find_program_address(&[b"__event_authority"], &DROPSET_ID);
        let accounts = SwapAccounts {
            taker: p.taker,
            market: self.market_key,
            base_mint: self.base_mint,
            quote_mint: self.quote_mint,
            base_token_program: p.base_token_program,
            quote_token_program: p.quote_token_program,
            taker_base_ata: p.taker_base_ata,
            taker_quote_ata: p.taker_quote_ata,
            market_base_treasury: self.base_treasury,
            market_quote_treasury: self.quote_treasury,
            clock: CLOCK_SYSVAR,
            event_authority,
            program: DROPSET_ID,
        };
        Ok(accounts.instruction(SwapInstructionArgs {
            side: side as u8,
            amount_in: p.amount_in,
            limit_price_bits: p.limit_price_bits,
            min_out: p.min_out,
        }))
    }

    /// Resolve taker side from the `(input, output)` mint pair.
    pub fn side_for(&self, input_mint: Pubkey, output_mint: Pubkey) -> Result<SwapSide, AmmError> {
        if input_mint == self.quote_mint && output_mint == self.base_mint {
            Ok(SwapSide::Buy) // pay quote, receive base
        } else if input_mint == self.base_mint && output_mint == self.quote_mint {
            Ok(SwapSide::Sell) // pay base, receive quote
        } else {
            Err(AmmError::MintMismatch)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::{MarketHeader, Vault, NULL_SECTOR};
    use bytemuck::Zeroable;

    /// Assemble a market account buffer (8-byte discriminator + header +
    /// slab len + vault sectors) from layout structs.
    pub(crate) fn build_market(header: MarketHeader, vaults: &[Vault]) -> Vec<u8> {
        let mut buf = vec![0u8; 8]; // discriminator (value unchecked by load)
        buf.extend_from_slice(bytemuck::bytes_of(&header));
        buf.extend_from_slice(&(vaults.len() as u32).to_le_bytes());
        for v in vaults {
            buf.extend_from_slice(bytemuck::bytes_of(v));
        }
        buf
    }

    /// A one-vault market with a single live ask at price 1.0 and
    /// `ask_size` base available. base==quote mint set to `[1;32]`/`[2;32]`.
    pub(crate) fn one_ask_market(ask_size: u64) -> Vec<u8> {
        let mut header = MarketHeader::zeroed();
        header.head = 0u32.into();
        header.active_count = 1u32.into();
        header.base_mint = [1u8; 32];
        header.quote_mint = [2u8; 32];

        let mut v = Vault::zeroed();
        v.next = NULL_SECTOR.into();
        v.prev = NULL_SECTOR.into();
        v.leader = [9u8; 32]; // non-zero => active (not on the free list)
        v.reference_price.price = Price::encode(10_000_000, 0).unwrap().as_u32().into(); // 1.0
        v.reference_price.stamp = 1u64.into(); // nonce 1, FLUSH_BIT clear
        v.base_atoms = 10_000_000u64.into();
        v.quote_atoms = 10_000_000u64.into();
        v.remaining.asks[0].price = Price::encode(10_000_000, 0).unwrap().as_u32().into();
        v.remaining.asks[0].size = ask_size.into();
        v.remaining.asks[0].expires_at = 1_000u32.into();
        build_market(header, &[v])
    }

    #[test]
    fn quote_buy_against_one_ask_level() {
        let data = one_ask_market(500_000);
        let amm = DropsetAmm::from_account(Pubkey::from([7u8; 32]), &data).unwrap();

        assert_eq!(amm.reserve_mints().len(), 2);
        assert_eq!(amm.accounts_to_update(), vec![amm.key()]);

        let q = amm
            .quote(&DropsetQuoteParams {
                input_mint: Pubkey::from([2u8; 32]),
                output_mint: Pubkey::from([1u8; 32]),
                amount: 1_000_000,
                current_slot: 10,
            })
            .unwrap();
        assert_eq!(q.out_amount, 500_000, "fills the whole ask level");
        assert_eq!(q.in_amount, 500_000, "input capped to what filled @1.0");
        assert_eq!(q.fee_amount, 0);
    }

    #[test]
    fn zero_input_is_safe() {
        let data = one_ask_market(500_000);
        let amm = DropsetAmm::from_account(Pubkey::from([7u8; 32]), &data).unwrap();
        let q = amm
            .quote(&DropsetQuoteParams {
                input_mint: Pubkey::from([2u8; 32]),
                output_mint: Pubkey::from([1u8; 32]),
                amount: 0,
                current_slot: 10,
            })
            .unwrap();
        assert_eq!(q, DropsetQuote::default());
    }

    #[test]
    fn swap_instruction_shape() {
        let data = one_ask_market(500_000);
        let amm = DropsetAmm::from_account(Pubkey::from([7u8; 32]), &data).unwrap();
        let ix = amm
            .swap_instruction(&DropsetSwapParams {
                taker: Pubkey::from([8u8; 32]),
                source_mint: Pubkey::from([2u8; 32]),
                destination_mint: Pubkey::from([1u8; 32]),
                taker_base_ata: Pubkey::from([10u8; 32]),
                taker_quote_ata: Pubkey::from([11u8; 32]),
                base_token_program: Pubkey::from([12u8; 32]),
                quote_token_program: Pubkey::from([13u8; 32]),
                amount_in: 1_000_000,
                limit_price_bits: Price::INFINITY.as_u32(),
                min_out: 0,
            })
            .unwrap();
        assert_eq!(ix.program_id, DROPSET_ID);
        assert_eq!(ix.data[0], 9, "swap discriminator");
        assert_eq!(ix.accounts.len(), 13);
    }

    #[test]
    fn is_active_reflects_matchable_vaults() {
        use crate::layout::MarketHeader;
        use bytemuck::Zeroable;

        // One live ask -> active.
        let live = one_ask_market(500_000);
        let amm = DropsetAmm::from_account(Pubkey::from([7u8; 32]), &live).unwrap();
        assert!(amm.is_active());
        assert_eq!(amm.label(), "Dropset");
        assert_eq!(amm.accounts_len(), 13);
        assert!(!amm.supports_exact_out());

        // No vaults -> inactive.
        let empty = build_market(MarketHeader::zeroed(), &[]);
        let amm = DropsetAmm::from_account(Pubkey::from([7u8; 32]), &empty).unwrap();
        assert!(!amm.is_active());
    }

    #[test]
    fn quote_rejects_foreign_mints() {
        let data = one_ask_market(500_000);
        let amm = DropsetAmm::from_account(Pubkey::from([7u8; 32]), &data).unwrap();
        let err = amm.quote(&DropsetQuoteParams {
            input_mint: Pubkey::from([3u8; 32]),
            output_mint: Pubkey::from([1u8; 32]),
            amount: 1,
            current_slot: 0,
        });
        assert_eq!(err, Err(AmmError::MintMismatch));
    }
}
