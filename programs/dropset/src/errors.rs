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
}
