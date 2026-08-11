//! SPL plumbing for seeding a local validator's mock mints.
//!
//! The program-id consts, the canonical associated-token-account derivation,
//! and the raw byte-instruction builders for the ATA program's
//! `CreateIdempotent` and SPL Token's `MintTo`. Pure: every function returns
//! an [`Instruction`] or a [`Pubkey`] and touches neither `RpcClient` nor
//! `Keypair`, so each consumer keeps its own sign-and-send path.

use solana_instruction::{AccountMeta, Instruction};
use solana_pubkey::{pubkey, Pubkey};

/// SPL Token program (the mock demo mints live here, not Token-2022).
pub const SPL_TOKEN_PROGRAM_ID: Pubkey = pubkey!("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA");
/// Associated Token Account program.
pub const ATA_PROGRAM_ID: Pubkey = pubkey!("ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL");
/// System program.
pub const SYSTEM_PROGRAM_ID: Pubkey = pubkey!("11111111111111111111111111111111");

/// Canonical associated-token-account address for `(wallet, mint)` under
/// `token_program` — seeds `[wallet, token_program, mint]`. Pass
/// [`SPL_TOKEN_PROGRAM_ID`] for the demo's mock mints; the parameter keeps the
/// derivation correct for a Token-2022 mint too.
pub fn associated_token_address(wallet: &Pubkey, mint: &Pubkey, token_program: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(
        &[wallet.as_ref(), token_program.as_ref(), mint.as_ref()],
        &ATA_PROGRAM_ID,
    )
    .0
}

/// The ATA-program `CreateIdempotent` instruction (index 1) for
/// `(wallet, mint, token_program)`, paid by `payer` — idempotent, so a re-run
/// after a partial bootstrap doesn't fail on an ATA that already exists.
/// Derive the resulting address with [`associated_token_address`].
pub fn create_ata_idempotent_ix(
    payer: &Pubkey,
    wallet: &Pubkey,
    mint: &Pubkey,
    token_program: &Pubkey,
) -> Instruction {
    let ata = associated_token_address(wallet, mint, token_program);
    Instruction::new_with_bytes(
        ATA_PROGRAM_ID,
        &[1u8],
        vec![
            AccountMeta::new(*payer, true),
            AccountMeta::new(ata, false),
            AccountMeta::new_readonly(*wallet, false),
            AccountMeta::new_readonly(*mint, false),
            AccountMeta::new_readonly(SYSTEM_PROGRAM_ID, false),
            AccountMeta::new_readonly(*token_program, false),
        ],
    )
}

/// The SPL Token `MintTo` instruction (index 7): mint `amount` atoms of `mint`
/// to `ata`. `authority` must be the mint authority. The demo's mock mints are
/// SPL Token, so the program id is [`SPL_TOKEN_PROGRAM_ID`].
pub fn mint_to_ix(authority: &Pubkey, mint: &Pubkey, ata: &Pubkey, amount: u64) -> Instruction {
    let mut data = vec![7u8];
    data.extend_from_slice(&amount.to_le_bytes());
    Instruction::new_with_bytes(
        SPL_TOKEN_PROGRAM_ID,
        &data,
        vec![
            AccountMeta::new(*mint, false),
            AccountMeta::new(*ata, false),
            AccountMeta::new_readonly(*authority, true),
        ],
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The ATA derivation follows the canonical `[wallet, token_program, mint]`
    /// seed order under the ATA program.
    #[test]
    fn ata_is_canonical() {
        let wallet = Pubkey::new_unique();
        let mint = Pubkey::new_unique();
        let expected = Pubkey::find_program_address(
            &[
                wallet.as_ref(),
                SPL_TOKEN_PROGRAM_ID.as_ref(),
                mint.as_ref(),
            ],
            &ATA_PROGRAM_ID,
        )
        .0;
        assert_eq!(
            associated_token_address(&wallet, &mint, &SPL_TOKEN_PROGRAM_ID),
            expected
        );
    }

    /// `CreateIdempotent` uses ATA-program index 1 and orders its metas
    /// payer, ata, wallet, mint, system, token-program — with the ata matching
    /// the canonical derivation.
    #[test]
    fn create_ata_ix_shape() {
        let payer = Pubkey::new_unique();
        let wallet = Pubkey::new_unique();
        let mint = Pubkey::new_unique();
        let ix = create_ata_idempotent_ix(&payer, &wallet, &mint, &SPL_TOKEN_PROGRAM_ID);
        assert_eq!(ix.program_id, ATA_PROGRAM_ID);
        assert_eq!(ix.data, vec![1u8]);
        let ata = associated_token_address(&wallet, &mint, &SPL_TOKEN_PROGRAM_ID);
        let keys: Vec<Pubkey> = ix.accounts.iter().map(|m| m.pubkey).collect();
        assert_eq!(
            keys,
            vec![
                payer,
                ata,
                wallet,
                mint,
                SYSTEM_PROGRAM_ID,
                SPL_TOKEN_PROGRAM_ID
            ]
        );
        assert!(ix.accounts[0].is_signer);
    }

    /// `MintTo` uses SPL-Token index 7 followed by the little-endian amount,
    /// with the authority as the sole signer.
    #[test]
    fn mint_to_ix_shape() {
        let authority = Pubkey::new_unique();
        let mint = Pubkey::new_unique();
        let ata = Pubkey::new_unique();
        let ix = mint_to_ix(&authority, &mint, &ata, 730);
        assert_eq!(ix.program_id, SPL_TOKEN_PROGRAM_ID);
        let mut expected = vec![7u8];
        expected.extend_from_slice(&730u64.to_le_bytes());
        assert_eq!(ix.data, expected);
        assert_eq!(ix.accounts[2].pubkey, authority);
        assert!(ix.accounts[2].is_signer);
    }
}
