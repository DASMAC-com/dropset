//! DFlow router adapter.
//!
//! Maps the Dropset market onto DFlow's `Amm` quoting + swap-CPI contract
//! (`DFlowProtocol/dflow-amm-interface`, a fork of `jupiter-amm-interface`).
//! [`DropsetAmm`] mirrors the trait method-for-method using the SDK's own
//! types; the methods are named after their `Amm` counterparts so the
//! upstream `impl Amm for DropsetAmm` is a thin shim.
//!
//! ## Wiring to the upstream `dflow-amm-interface` crate
//!
//! Two boundary items keep that shim out of this crate today (both
//! external, neither blocks the quoting logic below):
//!
//! 1. **Type skew.** DFlow pins `solana-sdk = "=2.3.*"` (the monolith),
//!    whereas this SDK uses the split `solana-pubkey`/`solana-instruction`
//!    3.x crates. They coexist in one tree (different majors); the shim
//!    converts `Pubkey`/`Instruction` at the boundary via raw bytes
//!    (`Pubkey::to_bytes()` round-trips), so no logic changes.
//! 2. **`Swap` enum.** `SwapAndAccountMetas.swap` is DFlow's closed `Swap`
//!    enum; emitting one for Dropset needs a variant added upstream in
//!    DFlow's fork (the instruction-level dependency ENG-378 anticipates).
//!    Until then [`DropsetAmm::swap_instruction`] returns the ready-built
//!    `solana_instruction::Instruction` + account metas directly.

use solana_instruction::Instruction;
use solana_pubkey::Pubkey;

use crate::generated::instructions::{Swap as SwapAccounts, SwapInstructionArgs};
use crate::layout::{LayoutError, MarketView};
use crate::matching::{simulate_swap, SwapSide};
use crate::price::Price;
use crate::DROPSET_ID;

/// The clock sysvar — read by `swap` for the current slot (expiry checks).
pub const CLOCK_SYSVAR: Pubkey = solana_pubkey::pubkey!("SysvarC1ock11111111111111111111111111111111");

/// A Dropset market presented through DFlow's `Amm` surface.
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

/// A quote request — mirrors `QuoteParams` (atoms in `amount`).
#[derive(Clone, Copy, Debug)]
pub struct DropsetQuoteParams {
    pub input_mint: Pubkey,
    pub output_mint: Pubkey,
    pub amount: u64,
    /// Current slot, for per-level expiry filtering. The upstream `Amm`
    /// sources this from `AmmContext.clock_ref`.
    pub current_slot: u32,
}

/// A quote result — `Amm::quote` returns `{ in_amount, out_amount }`;
/// `fee_amount` is exposed too for fee-aware routing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DropsetQuote {
    pub in_amount: u64,
    pub out_amount: u64,
    pub fee_amount: u64,
}

/// Accounts + args for a swap — mirrors `SwapParams`.
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
    /// `Amm::from_keyed_account`: build from a market account's key + data.
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

    /// `Amm::program_id`.
    pub fn program_id(&self) -> Pubkey {
        DROPSET_ID
    }

    /// `Amm::key`.
    pub fn key(&self) -> Pubkey {
        self.market_key
    }

    /// `Amm::get_reserve_mints` — `[base, quote]`.
    pub fn reserve_mints(&self) -> Vec<Pubkey> {
        vec![self.base_mint, self.quote_mint]
    }

    /// `Amm::get_accounts_to_update`. The whole book lives in the single
    /// market account, so quoting only needs that one account refreshed
    /// (interface.md § 4: the take is "not account-hungry").
    pub fn accounts_to_update(&self) -> Vec<Pubkey> {
        vec![self.market_key]
    }

    /// `Amm::update` — refresh the cached market account data.
    pub fn update(&mut self, data: &[u8]) -> Result<(), AmmError> {
        // Validate it decodes before swapping the cache in.
        MarketView::load(data)?;
        self.data = data.to_vec();
        Ok(())
    }

    /// `Amm::quote` — simulate a take against the current book.
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

    /// `Amm::get_swap_and_account_metas` (instruction half). Returns the
    /// ready-to-submit `swap` instruction; `.accounts` are the account
    /// metas. See the module docs on the upstream `Swap`-enum seam.
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
    fn side_for(&self, input_mint: Pubkey, output_mint: Pubkey) -> Result<SwapSide, AmmError> {
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
    /// slab len + one vault sector) from layout structs.
    fn build_market(header: MarketHeader, vaults: &[Vault]) -> Vec<u8> {
        let mut buf = vec![0u8; 8]; // discriminator (value unchecked by load)
        buf.extend_from_slice(bytemuck::bytes_of(&header));
        buf.extend_from_slice(&(vaults.len() as u32).to_le_bytes());
        for v in vaults {
            buf.extend_from_slice(bytemuck::bytes_of(v));
        }
        buf
    }

    #[test]
    fn quote_buy_against_one_ask_level() {
        let base_mint = [1u8; 32];
        let quote_mint = [2u8; 32];

        let mut header = MarketHeader::zeroed();
        header.head = 0u32.into();
        header.active_count = 1u32.into();
        header.taker_fee = 0u16.into(); // no fee for a clean assertion
        header.base_mint = base_mint;
        header.quote_mint = quote_mint;

        let mut v = Vault::zeroed();
        v.next = NULL_SECTOR.into();
        v.prev = NULL_SECTOR.into();
        v.leader = [9u8; 32]; // non-zero => active (not on the free list)
        // Reference price valid + no flush armed (read `remaining` directly).
        v.reference_price.price = Price::encode(10_000_000, 0).unwrap().as_u32().into(); // 1.0
        v.reference_price.stamp = 1u64.into(); // nonce 1, FLUSH_BIT clear
        v.base_atoms = 10_000_000u64.into();
        v.quote_atoms = 10_000_000u64.into();
        // One live ask: price 1.0, 500_000 base available, expires far out.
        v.remaining.asks[0].price = Price::encode(10_000_000, 0).unwrap().as_u32().into();
        v.remaining.asks[0].size = 500_000u64.into();
        v.remaining.asks[0].expires_at = 1_000u32.into();

        let data = build_market(header, &[v]);
        let amm = DropsetAmm::from_account(Pubkey::from([7u8; 32]), &data).unwrap();

        assert_eq!(amm.reserve_mints().len(), 2);
        assert_eq!(amm.accounts_to_update(), vec![amm.key()]);

        // Buy: pay 1_000_000 quote -> capped by the 500_000-base level.
        let q = amm
            .quote(&DropsetQuoteParams {
                input_mint: Pubkey::from(quote_mint),
                output_mint: Pubkey::from(base_mint),
                amount: 1_000_000,
                current_slot: 10,
            })
            .unwrap();
        assert_eq!(q.out_amount, 500_000, "fills the whole ask level");
        assert_eq!(q.in_amount, 500_000, "input capped to what filled @1.0");
        assert_eq!(q.fee_amount, 0);

        // A swap instruction targets the program with all accounts present.
        let ix = amm
            .swap_instruction(&DropsetSwapParams {
                taker: Pubkey::from([8u8; 32]),
                source_mint: Pubkey::from(quote_mint),
                destination_mint: Pubkey::from(base_mint),
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
    fn quote_rejects_foreign_mints() {
        let mut header = MarketHeader::zeroed();
        header.base_mint = [1u8; 32];
        header.quote_mint = [2u8; 32];
        let data = build_market(header, &[]);
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
