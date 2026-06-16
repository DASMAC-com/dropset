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
    #[msg("quote_slot is future-dated or backdated past MAX_BACKDATE")]
    InvalidQuoteSlot,
    #[msg("set_liquidity_profile requires the vault's reference price to be set first")]
    ReferencePriceNotSet,
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
    #[msg("token account must be drained to zero before it can be closed")]
    TokenAccountNotEmpty,
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
}
