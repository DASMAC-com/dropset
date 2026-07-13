//! Shared localnet-host plumbing for the demo crates (the TUI and the
//! maker / taker bots).
//!
//! Three parallel copies of this code had drifted across `tui/`,
//! `bots/maker-bot/`, and `bots/taker-bot/`; hoisting it here means a fix
//! lands once. Two groups:
//!
//! * **SPL plumbing** — the SPL Token / Associated-Token-Account / System
//!   program ids, the canonical ATA derivation, and the raw byte-instruction
//!   builders for `CreateIdempotent` and `MintTo`. These are *pure*: they
//!   return an [`Instruction`] (or a [`Pubkey`]) and take no `RpcClient` or
//!   `Keypair`, so each consumer keeps its own sign-and-send path — the TUI's
//!   carries compute-unit measurement the bots don't need.
//! * **[`ws_url_from_rpc`]** — the Agave `http`→`ws` PubSub-endpoint
//!   derivation the fill subscriptions share.

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

/// Derive the PubSub websocket endpoint from an RPC URL, matching the Agave
/// convention: swap the scheme (`http`→`ws`, `https`→`wss`) and use the RPC
/// port + 1 (the validator serves logs/account subscriptions there, so
/// `8899` → `8900`). Returns the input unchanged for an unrecognized scheme
/// (assume it is already a ws endpoint) or a non-numeric port.
pub fn ws_url_from_rpc(rpc_url: &str) -> String {
    let (scheme, rest) = if let Some(rest) = rpc_url.strip_prefix("https://") {
        ("wss://", rest)
    } else if let Some(rest) = rpc_url.strip_prefix("http://") {
        ("ws://", rest)
    } else {
        return rpc_url.to_string();
    };
    // PubSub lives at the root, so drop any path and bump the port.
    let authority = rest.split('/').next().unwrap_or(rest);
    let ws_authority = match authority.rsplit_once(':') {
        Some((host, port)) => match port.parse::<u16>() {
            Ok(port) => format!("{host}:{}", port.saturating_add(1)),
            Err(_) => authority.to_string(),
        },
        None => authority.to_string(),
    };
    format!("{scheme}{ws_authority}")
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

    /// The websocket endpoint swaps the scheme and uses the RPC port + 1.
    #[test]
    fn ws_url_follows_the_agave_convention() {
        assert_eq!(
            ws_url_from_rpc("http://127.0.0.1:8899"),
            "ws://127.0.0.1:8900"
        );
        assert_eq!(
            ws_url_from_rpc("https://api.example.com:443/rpc"),
            "wss://api.example.com:444"
        );
        // Unrecognized scheme is assumed to already be a ws endpoint.
        assert_eq!(ws_url_from_rpc("ws://host:9000"), "ws://host:9000");
    }
}
