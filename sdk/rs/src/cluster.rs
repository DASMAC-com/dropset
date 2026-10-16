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
//! A cargo feature cannot pick the id, since features unify across one
//! binary that serves several clusters. So it is chosen at runtime, and
//! never by default: every instruction builder (the generated ones, rewritten
//! by `sdk/codama/generate.mjs`, and the hand-written [`crate::quoting`] and
//! [`crate::adapters::amm`] surfaces) takes a [`Deployment`], and PDAs are
//! derived through the helpers here with [`Deployment::program_id`].
//!
//! The localnet id's secret is committed, so anyone can deploy a program at
//! that address on a public cluster. A [`Deployment`] can never resolve to
//! it off localnet; a raw id arriving from configuration goes through
//! [`Deployment::check_program_id`], which refuses it there.

use std::fmt;
use std::str::FromStr;

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

    /// Accept `program_id` for this cluster, refusing the localnet id
    /// anywhere but localnet: a program anyone can deploy must never be
    /// trusted or signed for on a public cluster. Any other id passes, so
    /// an operator can still point a client at a fresh deployment.
    pub fn check_program_id(self, program_id: Pubkey) -> Result<Pubkey, LocalnetIdOffLocalnet> {
        if self != Self::Localnet && program_id == DROPSET_ID {
            return Err(LocalnetIdOffLocalnet(self));
        }
        Ok(program_id)
    }
}

/// Parses `localnet`, or `mainnet` / `mainnet-beta`, case-insensitively.
impl FromStr for Deployment {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.to_ascii_lowercase().as_str() {
            "localnet" => Ok(Self::Localnet),
            "mainnet" | "mainnet-beta" => Ok(Self::Mainnet),
            other => Err(format!(
                "unknown cluster `{other}` (expected localnet or mainnet)"
            )),
        }
    }
}

/// [`Deployment::check_program_id`]'s refusal: the localnet id, offered
/// for the named non-localnet cluster.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LocalnetIdOffLocalnet(pub Deployment);

impl fmt::Display for LocalnetIdOffLocalnet {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "refusing the localnet program id {DROPSET_ID} on {:?}: its secret is \
             committed, so anyone can deploy a program at that address",
            self.0
        )
    }
}

impl std::error::Error for LocalnetIdOffLocalnet {}

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
    use super::*;
    use crate::instructions::{SetReferencePrice, SetReferencePriceInstructionArgs, Swap};

    #[test]
    fn deployments_resolve_distinct_ids() {
        assert_eq!(Deployment::Localnet.program_id(), DROPSET_ID);
        assert_eq!(Deployment::Mainnet.program_id(), DROPSET_MAINNET_ID);
        assert_ne!(DROPSET_ID, DROPSET_MAINNET_ID);
    }

    #[test]
    fn the_localnet_id_is_refused_off_localnet() {
        let fresh = Pubkey::new_unique();
        assert_eq!(
            Deployment::Mainnet.check_program_id(DROPSET_ID),
            Err(LocalnetIdOffLocalnet(Deployment::Mainnet)),
        );
        assert_eq!(Deployment::Mainnet.check_program_id(fresh), Ok(fresh));
        assert_eq!(
            Deployment::Mainnet.check_program_id(DROPSET_MAINNET_ID),
            Ok(DROPSET_MAINNET_ID),
        );
        assert_eq!(
            Deployment::Localnet.check_program_id(DROPSET_ID),
            Ok(DROPSET_ID)
        );
        assert_eq!(Deployment::Localnet.check_program_id(fresh), Ok(fresh));
    }

    #[test]
    fn deployment_parses_both_mainnet_spellings_and_rejects_junk() {
        assert_eq!("localnet".parse(), Ok(Deployment::Localnet));
        assert_eq!("mainnet".parse(), Ok(Deployment::Mainnet));
        assert_eq!("Mainnet-Beta".parse(), Ok(Deployment::Mainnet));
        assert!("devnet".parse::<Deployment>().is_err());
    }

    /// The generated builders aim every program-id slot at the deployment
    /// they are handed: the instruction itself and the placeholder an
    /// absent optional account takes. Guards the post-generation rewrite in
    /// `sdk/codama/generate.mjs` against a renderer change.
    #[test]
    fn generated_builders_follow_the_deployment() {
        let key = Pubkey::new_unique();
        for deployment in [Deployment::Localnet, Deployment::Mainnet] {
            let program = deployment.program_id();
            let ix = SetReferencePrice {
                signer: key,
                market: key,
            }
            .instruction(
                deployment,
                SetReferencePriceInstructionArgs {
                    vault_idx: 0,
                    price_bits: 0,
                    quote_slot: 0,
                    quote_unix: 0,
                },
            );
            assert_eq!(ix.program_id, program);

            let swap = Swap {
                taker: key,
                market: key,
                base_mint: key,
                quote_mint: key,
                base_token_program: key,
                quote_token_program: key,
                taker_base_ata: key,
                taker_quote_ata: key,
                market_base_treasury: key,
                market_quote_treasury: key,
                clock: key,
                platform_fee_authority: None,
                platform_fee_ata: None,
                associated_token_program: key,
                system_program: key,
                event_authority: event_authority(&program),
                program,
            };
            let ix = swap.instruction_with_remaining_accounts(
                deployment,
                crate::instructions::SwapInstructionArgs {
                    side: 0,
                    amount_in: 0,
                    limit_price_bits: 0,
                    min_out: 0,
                    platform_fee_bps: 0,
                },
                &[],
            );
            assert_eq!(ix.program_id, program);
            assert_eq!(
                ix.accounts[11].pubkey, program,
                "absent optional placeholder"
            );
            assert_eq!(
                ix.accounts[12].pubkey, program,
                "absent optional placeholder"
            );
            assert!(ix
                .accounts
                .iter()
                .all(|m| deployment == Deployment::Localnet || m.pubkey != DROPSET_ID));
        }
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
