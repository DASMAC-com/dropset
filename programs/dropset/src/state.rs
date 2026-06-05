use anchor_lang_v2::{
    accounts::Slab,
    bytemuck::{Pod, Zeroable},
    prelude::*,
};

/// Parts-per-million rate (1_000_000 = 100%), stored alignment-1.
/// Tight 16-bit form, max ~6.55% — used for the taker fee.
pub type Ppm16 = PodU16;
/// Parts-per-million rate (1_000_000 = 100%), stored alignment-1.
/// Wide 32-bit form — used for the skin-in-the-game floor.
pub type Ppm32 = PodU32;

/// Initial per-market vault cap stamped onto new markets.
pub const DEFAULT_MAX_VAULTS_PER_MARKET: u8 = 10;
/// Initial taker fee (ppm) stamped onto new markets. The spec sets no
/// number; 0 means no taker fee until an admin changes a market's rate.
pub const DEFAULT_TAKER_FEE: u16 = 0;
/// Initial skin-in-the-game floor (ppm): 5%, per the architecture spec.
pub const DEFAULT_MIN_LEADER_SHARE: u32 = 50_000;

/// A flat fee charged in `mint`, paid to the registry fee ATA. Mirrors
/// the `FeeConfig` in the architecture spec; reused per-market by the
/// `MarketHeader` once markets exist.
#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable, IdlType)]
#[bytemuck(crate = "anchor_lang_v2::bytemuck")]
pub struct FeeConfig {
    /// Mint accepted for this fee.
    pub mint: Address,
    /// Amount in atoms of `mint`.
    pub atoms: PodU64,
}

/// Header of the global singleton `Registry` account. Holds the
/// protocol-wide governance defaults stamped onto new markets; the
/// admin allowlist (the spec's `Set<Pubkey>`) lives in the slab tail —
/// see [`Registry`].
///
/// All fields are alignment-1 (`Address`, `Pod*` wrappers, `u8`) so the
/// header is padding-free and casts directly from the account bytes.
#[account]
pub struct RegistryHeader {
    /// Default fee for the per-`OpenVault` charge, stamped into
    /// `MarketHeader.fee_config` at market creation.
    pub default_fee_config: FeeConfig,
    /// Default skin-in-the-game floor stamped into markets.
    pub default_min_leader_share: Ppm32,
    /// Default taker fee stamped into markets.
    pub default_taker_fee: Ppm16,
    /// Default cap on vaults per market.
    pub max_vaults_per_market: u8,
    /// Registry PDA bump.
    pub bump: u8,
}

/// The `Registry` account: a [`RegistryHeader`] followed by a slab tail
/// of admin pubkeys (the spec's `Set<Pubkey>`). Membership is the tail —
/// add with `try_push` (caller dup-checks), remove with `swap_remove`.
pub type Registry = Slab<RegistryHeader, Address>;
