// cspell:word retarget

//! Per-cluster program ids.
//!
//! The program is deployed at a different address per cluster: localnet
//! keeps the committed vanity key (`keys/AAAA.json`, the generated
//! [`DROPSET_ID`]), and mainnet uses a key whose secret lives only in
//! 1Password, its public half committed at `keys/dropset-mainnet.pubkey`.
//! The program crate's `mainnet` feature selects the matching
//! `declare_id!`; `tests/program_id.rs` pins that literal, the key file and
//! [`DROPSET_MAINNET_ID`] together.
//!
//! The Codama-generated builders hardcode [`DROPSET_ID`], and a cargo
//! feature cannot switch them, since features unify across one binary
//! that serves several clusters. So the id is chosen at runtime: derive
//! PDAs through the helpers here with [`Deployment::program_id`], and pass
//! a generated instruction through [`retarget`] before signing it.

use solana_instruction::Instruction;
use solana_pubkey::{pubkey, Pubkey};

use crate::DROPSET_ID;

/// The mainnet program id: the public half of the 1Password-held program
/// keypair, mirrored from `keys/dropset-mainnet.pubkey`.
pub const DROPSET_MAINNET_ID: Pubkey = pubkey!("FXv1Dsy4CL5UvvkAAcPmzHZCvaotJMi9TfYLraj99KUL");

/// The cluster a program id is resolved for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Deployment {
    /// The committed localnet vanity id, [`DROPSET_ID`].
    Localnet,
    /// The mainnet id, [`DROPSET_MAINNET_ID`].
    Mainnet,
}

impl Deployment {
    /// The program id deployed on this cluster.
    pub const fn program_id(self) -> Pubkey {
        match self {
            Self::Localnet => DROPSET_ID,
            Self::Mainnet => DROPSET_MAINNET_ID,
        }
    }
}

/// The registry PDA, seeded on `"registry"`.
pub fn registry_pda(program_id: &Pubkey) -> (Pubkey, u8) {
    Pubkey::find_program_address(&[b"registry"], program_id)
}

/// A market's PDA, seeded on its `[base, quote]` mints.
pub fn market_pda(program_id: &Pubkey, base_mint: &Pubkey, quote_mint: &Pubkey) -> (Pubkey, u8) {
    Pubkey::find_program_address(&[base_mint.as_ref(), quote_mint.as_ref()], program_id)
}

/// A vault depositor's PDA, seeded on `"vault_depositor"`, the market, the
/// little-endian vault index and the depositor.
pub fn vault_depositor_pda(
    program_id: &Pubkey,
    market: &Pubkey,
    vault_idx: u32,
    depositor: &Pubkey,
) -> (Pubkey, u8) {
    Pubkey::find_program_address(
        &[
            b"vault_depositor",
            market.as_ref(),
            &vault_idx.to_le_bytes(),
            depositor.as_ref(),
        ],
        program_id,
    )
}

/// The `emit_cpi!` event authority PDA, seeded on `"__event_authority"`.
/// Address only: no instruction takes its bump.
pub fn event_authority(program_id: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(&[b"__event_authority"], program_id).0
}

/// Point a generated instruction at `program_id`.
///
/// The input is assumed to be as a generated builder emits it, aimed at
/// [`DROPSET_ID`]; an instruction already aimed elsewhere is not mapped
/// back. Rewrites the instruction's program id and every account meta
/// that the builder filled from [`DROPSET_ID`]: the `program` account
/// `emit_cpi!` reads, the placeholder for an absent optional account, and
/// the localnet event authority. Other PDAs are the caller's to derive
/// with the matching id, since the builders take them as inputs. A no-op
/// for [`DROPSET_ID`] itself.
pub fn retarget(mut ix: Instruction, program_id: &Pubkey) -> Instruction {
    if *program_id == DROPSET_ID {
        return ix;
    }
    let localnet_authority = event_authority(&DROPSET_ID);
    let target_authority = event_authority(program_id);
    ix.program_id = *program_id;
    for meta in &mut ix.accounts {
        if meta.pubkey == DROPSET_ID {
            meta.pubkey = *program_id;
        } else if meta.pubkey == localnet_authority {
            meta.pubkey = target_authority;
        }
    }
    ix
}

/// Whether a program binary embeds `program_id` and not [`DROPSET_ID`].
///
/// An advisory wrong-cluster build detector, not a deploy gate: it says
/// nothing about build features, or about whether the keypair a deploy
/// signs with is the one for `program_id`; the publish step checks that.
///
/// A raw 32-byte scan of the `.so`, not ELF-aware: `declare_id!` puts the
/// id in read-only data, so a build for the wrong cluster shows up as the
/// wrong id present, or the localnet id still there. It fails safe: an id
/// the compiler did not lay out as 32 contiguous bytes reads as absent.
/// For `DROPSET_ID` itself this is just "embeds it".
pub fn embeds_program_id(binary: &[u8], program_id: &Pubkey) -> bool {
    let embeds = |key: &Pubkey| binary.windows(32).any(|window| window == key.as_ref());
    embeds(program_id) && (*program_id == DROPSET_ID || !embeds(&DROPSET_ID))
}

#[cfg(test)]
mod tests {
    use solana_instruction::AccountMeta;

    use super::*;

    #[test]
    fn deployments_resolve_distinct_ids() {
        assert_eq!(Deployment::Localnet.program_id(), DROPSET_ID);
        assert_eq!(Deployment::Mainnet.program_id(), DROPSET_MAINNET_ID);
        assert_ne!(DROPSET_ID, DROPSET_MAINNET_ID);
    }

    #[test]
    fn retarget_rewrites_program_placeholders_and_event_authority() {
        let other = Pubkey::new_unique();
        let ix = Instruction {
            program_id: DROPSET_ID,
            accounts: vec![
                AccountMeta::new(other, true),
                AccountMeta::new_readonly(DROPSET_ID, false),
                AccountMeta::new_readonly(event_authority(&DROPSET_ID), false),
            ],
            data: vec![1, 2, 3],
        };
        let target = DROPSET_MAINNET_ID;
        let out = retarget(ix.clone(), &target);
        assert_eq!(out.program_id, target);
        assert_eq!(out.accounts[0], ix.accounts[0]);
        assert_eq!(out.accounts[1], AccountMeta::new_readonly(target, false));
        assert_eq!(
            out.accounts[2],
            AccountMeta::new_readonly(event_authority(&target), false),
        );
        assert_eq!(out.data, ix.data);
        assert_eq!(retarget(ix.clone(), &DROPSET_ID), ix);
    }

    #[test]
    fn embeds_requires_the_target_and_rejects_the_localnet_id() {
        let target = DROPSET_MAINNET_ID;
        let with = |keys: &[&Pubkey]| {
            let mut binary = vec![0xAB; 7];
            for key in keys {
                binary.extend_from_slice(key.as_ref());
                binary.extend_from_slice(&[0xCD; 5]);
            }
            binary
        };
        assert!(embeds_program_id(&with(&[&target]), &target));
        assert!(!embeds_program_id(&with(&[]), &target));
        assert!(!embeds_program_id(&with(&[&target, &DROPSET_ID]), &target));
        assert!(embeds_program_id(&with(&[&DROPSET_ID]), &DROPSET_ID));
    }

    #[test]
    fn pdas_match_the_generated_ts_helpers() {
        // Reference addresses from the IDL-generated TS `find*Pda` helpers
        // under the localnet id, for base = the token program, quote = the
        // clock sysvar, vault index 258 and depositor = base.
        let base = pubkey!("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA");
        let quote = pubkey!("SysvarC1ock11111111111111111111111111111111");
        let market = market_pda(&DROPSET_ID, &base, &quote).0;
        assert_eq!(
            registry_pda(&DROPSET_ID).0,
            pubkey!("4AJ5raZiphprYuDpURY2L872VajxaeJTrnuzNoegq5pe"),
        );
        assert_eq!(
            market,
            pubkey!("HYSgcQyn9A4KZcHpNX66DsViPtBd83ky7uZQGcwiUiTQ")
        );
        assert_eq!(
            event_authority(&DROPSET_ID),
            // cspell:disable-next-line
            pubkey!("AzuJgpibLcuvGQwdtWPMu9jaeDQobh6694eySB2Xzkvx"),
        );
        assert_eq!(
            vault_depositor_pda(&DROPSET_ID, &market, 258, &base).0,
            pubkey!("J79enZn3RrUspGLYqJKtNa5CuxifSDCsePzEV2e2tinF"),
        );
    }

    #[test]
    fn pdas_follow_the_program_id() {
        let (base, quote) = (Pubkey::new_unique(), Pubkey::new_unique());
        assert_ne!(
            market_pda(&DROPSET_ID, &base, &quote),
            market_pda(&DROPSET_MAINNET_ID, &base, &quote),
        );
        assert_ne!(registry_pda(&DROPSET_ID), registry_pda(&DROPSET_MAINNET_ID));
    }
}
