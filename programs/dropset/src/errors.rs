use anchor_lang_v2::prelude::*;

#[error_code]
pub enum DropsetError {
    #[msg("program_data account is not the canonical PDA for this program")]
    InvalidProgramDataAddress,
    #[msg("program_data account contents could not be decoded")]
    InvalidProgramData,
    #[msg("Init must be signed by the program's upgrade authority")]
    InvalidUpgradeAuthority,
    #[msg("the registry admin set has no room for another admin")]
    AdminSetFull,
    #[msg("the named pubkey is already a registry admin")]
    AlreadyAdmin,
    #[msg("signer is not a registry admin")]
    Unauthorized,
    #[msg("the named pubkey is not a registry admin")]
    AdminNotFound,
    #[msg("cannot remove the last remaining registry admin")]
    CannotRemoveLastAdmin,
    #[msg("fee mint does not match the registry's configured fee mint")]
    InvalidFeeMint,
    #[msg("base and quote mints must differ")]
    DuplicateBaseQuoteMint,
    #[msg("supplied sector index is out of range")]
    InvalidSectorIndex,
    #[msg("vault list pointers are inconsistent with the list head")]
    CorruptVaultList,
    #[msg("registry market_count cannot exceed u32::MAX")]
    MarketCountOverflow,
    #[msg("market vault cap (registry.max_vaults_per_market) is full")]
    VaultCapExceeded,
    #[msg("perf_fee_rate exceeds 1_000_000 ppm (100%)")]
    InvalidPerfFeeRate,
    #[msg("min_leader_share exceeds 1_000_000 ppm (100%)")]
    InvalidMinLeaderShare,
    #[msg("non-admin caller cannot open a vault on someone else's behalf")]
    LeaderOverrideNotAllowed,
    #[msg("supplied vault sector is not assigned (leader == default)")]
    VaultEmpty,
    #[msg("vault is frozen")]
    VaultFrozen,
    #[msg("price bit pattern is not a valid encoding")]
    InvalidPrice,
    // No longer emitted: `set_reference_price` stores `quote_slot` raw
    // (see the architecture spec's **SetReferencePrice**). Retained so the
    // custom error codes of the variants below it don't shift.
    #[msg("quote_slot is invalid")]
    InvalidQuoteSlot,
    // Raised by `deposit`, which stamps the depositor's `entry_ref_price`
    // from the vault's reference price and can't do that basis math against
    // the zero / INF sentinels. `set_liquidity_profile` used to raise it too
    // and no longer does: a ladder armed before a price is inert, since
    // matching skips the whole vault ahead of the flush.
    #[msg("vault's reference price must be set first")]
    ReferencePriceNotSet,
    // No longer emitted: `set_liquidity_profile` stores the ladder raw (see
    // the architecture spec's **SetLiquidityProfile**), and the per-side
    // `Σ size_bps ≤ 10000` invariant is enforced at match time by skipping
    // the offending side rather than by an error. Retained so the custom
    // error codes of the variants below it don't shift.
    #[msg("liquidity profile size_bps sum exceeds 10_000 on one side")]
    LiquidityProfileSizeOverflow,
    #[msg("leader has not enabled outside depositors on this vault")]
    OutsideDepositorsNotAllowed,
    #[msg("admin has not approved outside deposits on this vault")]
    OutsideDepositorsNotApproved,
    #[msg("first deposit to a vault must come from its leader")]
    SeedingRequiresLeader,
    #[msg("first deposit to a vault must supply both base and quote legs")]
    SeedingRequiresBothLegs,
    #[msg("non-seeding deposit must size exactly one of base_in / quote_in")]
    SingleLegRequired,
    #[msg("derived basket exceeds caller's slippage bounds")]
    BasketSlippage,
    #[msg("operation would violate the vault's min_leader_share floor")]
    MinLeaderShareViolated,
    #[msg("requested shares exceed the caller's available stake")]
    InsufficientShares,
    #[msg("swap amount_in must be greater than zero")]
    InvalidAmountIn,
    #[msg("supplied VaultDepositor PDA does not match the (market, sector, owner) seeds")]
    VaultDepositorMismatch,
    #[msg("arithmetic overflow in basket / share math")]
    MathOverflow,
    #[msg("swap `side` argument is neither Buy nor Sell")]
    InvalidSwapSide,
    #[msg("limit_price sentinel is invalid for this swap side")]
    InvalidLimitPrice,
    #[msg("vault is already on the tombstone list")]
    VaultAlreadyTombstoned,
    #[msg("market vaults still hold inventory for this leg")]
    MarketVaultsNotDrained,
    #[msg("market treasury must be closed before the market can be closed")]
    MarketTreasuryNotClosed,
    #[msg("market still has outstanding VaultDepositor PDAs")]
    MarketHasDepositors,
    #[msg("registry still has live markets (market_count != 0)")]
    RegistryHasMarkets,
    #[msg("registry still has admins beyond the caller")]
    RegistryHasOtherAdmins,
    #[msg("supplied mint is not one of the market's base/quote legs")]
    NotAMarketTreasury,
    #[msg("teardown instructions are disabled in this build (admin-teardown feature off)")]
    TeardownDisabled,
    #[msg("vault has been closed and moved to the tombstone list")]
    VaultTombstoned,
    #[msg("max_platform_fee exceeds 10_000 bps (100%)")]
    InvalidMaxPlatformFee,
    #[msg("declared platform_fee_bps exceeds the market's max_platform_fee")]
    PlatformFeeTooHigh,
    #[msg("a non-zero platform_fee_bps requires both the fee authority and its fee token account")]
    MissingPlatformFeeAccounts,
    #[msg("market still has vaults on the active list")]
    MarketHasActiveVaults,
}
