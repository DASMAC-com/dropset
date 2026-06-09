use anchor_lang_v2::{
    accounts::Slab,
    address_eq,
    bytemuck::{Pod, Zeroable},
    prelude::*,
};

use crate::errors::DropsetError;

/// Parts-per-million rate (1_000_000 = 100%). Logical 16-bit form, max
/// ~6.55% — the taker fee. Stored in `RegistryHeader` as an
/// alignment-1 `PodU16`.
pub type Ppm16 = u16;
/// Parts-per-million rate (1_000_000 = 100%). Logical 32-bit form — the
/// skin-in-the-game floor. Stored as an alignment-1 `PodU32`.
pub type Ppm32 = u32;

/// Initial per-market vault cap stamped onto new markets.
pub const DEFAULT_MAX_VAULTS_PER_MARKET: u8 = 10;
/// Initial taker fee (ppm) stamped onto new markets. The spec sets no
/// number; 0 means no taker fee until an admin changes a market's rate.
pub const DEFAULT_TAKER_FEE: Ppm16 = 0;
/// Initial skin-in-the-game floor (ppm): 5%, per the architecture spec.
pub const DEFAULT_MIN_LEADER_SHARE: Ppm32 = 50_000;

/// A flat fee charged in `mint`, paid to the registry fee ATA. Mirrors
/// the `FeeConfig` in the architecture spec; reused per-market by the
/// `MarketHeader` once markets exist. Carrying `token_program`
/// alongside `mint` lets downstream fee-collection paths derive the
/// canonical ATA — `(authority, token_program, mint)` — without
/// guessing which token program owns the mint.
#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable, IdlType)]
#[bytemuck(crate = "anchor_lang_v2::bytemuck")]
pub struct FeeConfig {
    /// Mint accepted for this fee.
    pub mint: Address,
    /// Token program owning `mint` — SPL Token or Token-2022.
    pub token_program: Address,
    /// Amount in atoms of `mint`.
    pub atoms: PodU64,
}

/// Header of the global singleton `Registry`. Holds the protocol-wide
/// governance defaults stamped onto new markets; the admin allowlist
/// (the spec's `Set<Pubkey>`) is the slab tail — see [`Registry`].
///
/// All fields are alignment-1 (`Address`, `Pod*` wrappers, `u8`) so the
/// header is padding-free and casts directly from the account bytes.
#[account]
pub struct RegistryHeader {
    /// Default fee for the per-`OpenVault` charge, stamped into
    /// `MarketHeader.fee_config` at market creation.
    pub default_fee_config: FeeConfig,
    /// Number of live markets created against this registry. Incremented
    /// by `create_market`, decremented by `close_market`. `close_registry`
    /// requires this to be zero — the only on-chain witness that no
    /// orphan markets remain, since the program cannot iterate all PDAs
    /// to verify by enumeration. See the architecture spec, **Account
    /// lifecycle and rent reclamation**.
    pub market_count: PodU32,
    /// Default skin-in-the-game floor (ppm, [`Ppm32`]) stamped into markets.
    pub default_min_leader_share: PodU32,
    /// Default taker fee (ppm, [`Ppm16`]) stamped into markets.
    pub default_taker_fee: PodU16,
    /// Default cap on vaults per market.
    pub max_vaults_per_market: u8,
    /// Registry PDA bump.
    pub bump: u8,
}

/// The `Registry` account: a [`RegistryHeader`] followed by a
/// **densely-packed** slab tail of admin pubkeys (the spec's
/// `Set<Pubkey>`). The tail holds exactly the active admins — removal
/// compacts it (swap-remove) and refunds the freed rent, so the account
/// size always tracks the admin count and there is no free list. Use
/// the [`AdminSet`] trait for membership operations.
pub type Registry = Slab<RegistryHeader, Address>;

/// Set operations over the admin tail of the [`Registry`]. Implemented
/// for the foreign `Slab` alias, so an extension trait rather than
/// inherent methods. Membership is a linear scan — the admin set is tiny
/// and only admins mutate the registry, so the simplicity wins over any
/// index structure.
pub trait AdminSet {
    /// Whether `admin` is in the set.
    fn admin_contains(&self, admin: &Address) -> bool;
    /// Insert `admin`, growing the tail by one slot and **funding the
    /// extra rent** from `payer` when the account is full. Rejects with
    /// [`DropsetError::AlreadyAdmin`] if `admin` is already a member — an
    /// admin cannot be added twice.
    fn admin_insert(&mut self, admin: Address, payer: &AccountView) -> Result<()>;
    /// Remove `admin`, compacting the tail and **refunding the freed
    /// rent** to `rent_recipient`. Returns whether `admin` was present.
    /// Rejects with [`DropsetError::CannotRemoveLastAdmin`] if `admin` is
    /// the sole remaining admin — the set is never allowed to empty.
    fn admin_remove(&mut self, admin: &Address, rent_recipient: &mut AccountView) -> Result<bool>;
}

impl AdminSet for Registry {
    fn admin_contains(&self, admin: &Address) -> bool {
        self.as_slice().iter().any(|a| address_eq(a, admin))
    }

    fn admin_insert(&mut self, admin: Address, payer: &AccountView) -> Result<()> {
        if self.admin_contains(&admin) {
            return Err(DropsetError::AlreadyAdmin.into());
        }
        // Grow the tail by one slot if there's no room, funding the
        // resulting rent shortfall from `payer`.
        let needed = self.len() + 1;
        if self.capacity() < needed {
            self.resize_to_capacity(needed as u32)?;
            self.top_up(payer)?;
        }
        self.try_push(admin)
            .map_err(|_| DropsetError::AdminSetFull)?;
        Ok(())
    }

    fn admin_remove(&mut self, admin: &Address, rent_recipient: &mut AccountView) -> Result<bool> {
        let pos = match self.as_slice().iter().position(|a| address_eq(a, admin)) {
            Some(pos) => pos,
            None => return Ok(false),
        };
        // Never let the set go empty — at least one admin must remain to
        // govern the registry.
        if self.len() <= 1 {
            return Err(DropsetError::CannotRemoveLastAdmin.into());
        }
        // Move the last admin into the gap and drop the tail entry, then
        // shrink the account to fit and return the freed rent.
        self.swap_remove(pos);
        let new_len = self.len();
        self.resize_to_capacity(new_len as u32)?;
        self.refund(rent_recipient)?;
        Ok(true)
    }
}
