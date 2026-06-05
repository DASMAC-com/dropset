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
    #[msg("signer is not a registry admin")]
    Unauthorized,
    #[msg("the named pubkey is not a registry admin")]
    AdminNotFound,
    #[msg("cannot remove the last remaining registry admin")]
    CannotRemoveLastAdmin,
}
