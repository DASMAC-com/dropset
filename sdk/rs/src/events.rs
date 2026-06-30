//! Off-chain decoder for the program's `emit_cpi!` events.
//!
//! anchor v2's `emit_cpi!` records each event as a self-CPI to the
//! program (authority = the `__event_authority` PDA), so the event lands
//! in the transaction's *inner* instructions rather than the logs. Each
//! such inner-instruction `data` is
//!
//! ```text
//! EVENT_IX_TAG_LE (8)  ++  DISCRIMINATOR (8)  ++  body
//! ```
//!
//! where the body is the borsh wire form for a default `#[event]` and the
//! raw `repr(C)` bytes for `#[event(bytemuck)]` ([`FillEvent`]). The
//! generated [`crate::types`] structs mirror the on-chain layouts
//! field-for-field (the bytemuck `FillEvent` carries explicit `pad` /
//! `pad2` fields), so a single borsh decode reads either form — this is
//! the "Codama supplies only the post-extraction codec" split from
//! `interface.md` §2. This module supplies the extraction (walk inner
//! instructions, strip the `[tag][discriminator]` envelope) and the
//! dispatch.
//!
//! The 8-byte discriminators are `sha256("event:<StructName>")[..8]` (the
//! anchor scheme) and mirror the `events` list in the generated IDL
//! (`sdk/idl/dropset.json`), pinned by [`tests`] below. They are kept as
//! constants here for the same reason the account discriminators are
//! (e.g. [`crate::accounts::MARKET_HEADER_DISCRIMINATOR`]) — a decoder
//! shouldn't hash at runtime.

use crate::types::{
    CloseVaultEvent, CreateVaultEvent, DepositEvent, FillEvent, FreezeVaultEvent, RealizeEvent,
    SetDefaultFeeConfigEvent, SetMarketFeeConfigEvent, SetMinLeaderShareEvent,
    SetRegistryDefaultsEvent, SetTakerFeeEvent, WithdrawEvent,
};
use borsh::BorshDeserialize;

/// The anchor v2 `emit_cpi!` self-CPI tag, little-endian — the 8-byte
/// prefix on every event inner-instruction's data (`0x1d9acb512ea545e4`).
pub const EVENT_IX_TAG: u64 = 0x1d9a_cb51_2ea5_45e4;

/// [`EVENT_IX_TAG`] as the little-endian byte prefix to match on.
pub const EVENT_IX_TAG_LE: [u8; 8] = EVENT_IX_TAG.to_le_bytes();

/// Length of the discriminator that follows the tag.
pub const DISCRIMINATOR_LEN: usize = 8;

// Event discriminators (sha256("event:<Name>")[..8]) — mirror of the IDL
// `events` list, pinned by the test module.
const CLOSE_VAULT: [u8; 8] = [35, 37, 158, 74, 115, 93, 175, 136];
const CREATE_VAULT: [u8; 8] = [42, 221, 241, 92, 177, 139, 118, 240];
const DEPOSIT: [u8; 8] = [120, 248, 61, 83, 31, 142, 107, 144];
const FILL: [u8; 8] = [13, 89, 41, 228, 105, 178, 45, 112];
const FREEZE_VAULT: [u8; 8] = [9, 180, 143, 223, 189, 20, 1, 74];
const REALIZE: [u8; 8] = [255, 60, 160, 248, 4, 188, 32, 33];
const SET_DEFAULT_FEE_CONFIG: [u8; 8] = [173, 121, 245, 191, 189, 52, 211, 216];
const SET_MARKET_FEE_CONFIG: [u8; 8] = [29, 171, 38, 30, 62, 131, 204, 214];
const SET_MIN_LEADER_SHARE: [u8; 8] = [159, 194, 164, 181, 227, 131, 179, 105];
const SET_REGISTRY_DEFAULTS: [u8; 8] = [138, 35, 107, 189, 236, 175, 31, 9];
const SET_TAKER_FEE: [u8; 8] = [175, 232, 242, 29, 241, 48, 172, 41];
const WITHDRAW: [u8; 8] = [22, 9, 133, 26, 160, 44, 71, 192];

/// Every event the program emits via `emit_cpi!`, decoded into its
/// generated struct. Variants the indexer rolls up (fills, the
/// liquidity-flow events) and the admin retuning events the teardown path
/// reconstructs from history (see `interface.md` §1).
#[derive(Clone, Debug, PartialEq)]
pub enum DropsetEvent {
    Fill(FillEvent),
    Deposit(DepositEvent),
    Withdraw(WithdrawEvent),
    CreateVault(CreateVaultEvent),
    CloseVault(CloseVaultEvent),
    FreezeVault(FreezeVaultEvent),
    Realize(RealizeEvent),
    SetMinLeaderShare(SetMinLeaderShareEvent),
    SetMarketFeeConfig(SetMarketFeeConfigEvent),
    SetTakerFee(SetTakerFeeEvent),
    SetDefaultFeeConfig(SetDefaultFeeConfigEvent),
    SetRegistryDefaults(SetRegistryDefaultsEvent),
}

impl DropsetEvent {
    /// The discriminator name (the event struct name) — a stable key for
    /// the indexer's table dispatch.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Fill(_) => "FillEvent",
            Self::Deposit(_) => "DepositEvent",
            Self::Withdraw(_) => "WithdrawEvent",
            Self::CreateVault(_) => "CreateVaultEvent",
            Self::CloseVault(_) => "CloseVaultEvent",
            Self::FreezeVault(_) => "FreezeVaultEvent",
            Self::Realize(_) => "RealizeEvent",
            Self::SetMinLeaderShare(_) => "SetMinLeaderShareEvent",
            Self::SetMarketFeeConfig(_) => "SetMarketFeeConfigEvent",
            Self::SetTakerFee(_) => "SetTakerFeeEvent",
            Self::SetDefaultFeeConfig(_) => "SetDefaultFeeConfigEvent",
            Self::SetRegistryDefaults(_) => "SetRegistryDefaultsEvent",
        }
    }
}

/// Decode one tag-stripped event payload (`[discriminator(8)][body]`).
///
/// Returns `None` if the payload is too short, the discriminator matches
/// no known event, or the body fails to deserialize. The body decodes via
/// borsh against the generated struct; for the bytemuck `FillEvent` the
/// generated struct's explicit padding fields make the borsh read
/// byte-identical to the on-chain `repr(C)` bytes.
pub fn decode_event_payload(payload: &[u8]) -> Option<DropsetEvent> {
    if payload.len() < DISCRIMINATOR_LEN {
        return None;
    }
    let (disc, mut body) = payload.split_at(DISCRIMINATOR_LEN);
    let disc: [u8; 8] = disc.try_into().ok()?;
    macro_rules! decode {
        ($variant:ident, $ty:ty) => {
            <$ty>::deserialize(&mut body)
                .ok()
                .map(DropsetEvent::$variant)
        };
    }
    match disc {
        FILL => decode!(Fill, FillEvent),
        DEPOSIT => decode!(Deposit, DepositEvent),
        WITHDRAW => decode!(Withdraw, WithdrawEvent),
        CREATE_VAULT => decode!(CreateVault, CreateVaultEvent),
        CLOSE_VAULT => decode!(CloseVault, CloseVaultEvent),
        FREEZE_VAULT => decode!(FreezeVault, FreezeVaultEvent),
        REALIZE => decode!(Realize, RealizeEvent),
        SET_MIN_LEADER_SHARE => decode!(SetMinLeaderShare, SetMinLeaderShareEvent),
        SET_MARKET_FEE_CONFIG => decode!(SetMarketFeeConfig, SetMarketFeeConfigEvent),
        SET_TAKER_FEE => decode!(SetTakerFee, SetTakerFeeEvent),
        SET_DEFAULT_FEE_CONFIG => decode!(SetDefaultFeeConfig, SetDefaultFeeConfigEvent),
        SET_REGISTRY_DEFAULTS => decode!(SetRegistryDefaults, SetRegistryDefaultsEvent),
        _ => None,
    }
}

/// Strip the `EVENT_IX_TAG_LE` prefix from one inner-instruction `data`,
/// yielding the `[discriminator][body]` payload — or `None` if this inner
/// instruction is not a Dropset event-CPI.
pub fn strip_event_tag(inner_ix_data: &[u8]) -> Option<&[u8]> {
    inner_ix_data
        .strip_prefix(&EVENT_IX_TAG_LE)
        .filter(|payload| payload.len() >= DISCRIMINATOR_LEN)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Pin the constants to the anchor discriminator scheme
    // (sha256("event:<Name>")[..8]) without a runtime hash dependency:
    // a tiny vendored sha256 reproduces the bytes the IDL records.
    fn anchor_event_discriminator(name: &str) -> [u8; 8] {
        let mut input = b"event:".to_vec();
        input.extend_from_slice(name.as_bytes());
        let digest = sha256(&input);
        digest[..8].try_into().unwrap()
    }

    #[test]
    fn discriminators_match_anchor_scheme() {
        let cases = [
            (CLOSE_VAULT, "CloseVaultEvent"),
            (CREATE_VAULT, "CreateVaultEvent"),
            (DEPOSIT, "DepositEvent"),
            (FILL, "FillEvent"),
            (FREEZE_VAULT, "FreezeVaultEvent"),
            (REALIZE, "RealizeEvent"),
            (SET_DEFAULT_FEE_CONFIG, "SetDefaultFeeConfigEvent"),
            (SET_MARKET_FEE_CONFIG, "SetMarketFeeConfigEvent"),
            (SET_MIN_LEADER_SHARE, "SetMinLeaderShareEvent"),
            (SET_REGISTRY_DEFAULTS, "SetRegistryDefaultsEvent"),
            (SET_TAKER_FEE, "SetTakerFeeEvent"),
            (WITHDRAW, "WithdrawEvent"),
        ];
        for (constant, name) in cases {
            assert_eq!(constant, anchor_event_discriminator(name), "{name}");
        }
    }

    #[test]
    fn strip_rejects_non_event_data() {
        assert!(strip_event_tag(&[1, 2, 3]).is_none());
        let mut tagged = EVENT_IX_TAG_LE.to_vec();
        tagged.extend_from_slice(&FILL);
        // tag present, payload is exactly a discriminator (== DISCRIMINATOR_LEN)
        assert!(strip_event_tag(&tagged).is_some());
    }

    #[test]
    fn unknown_discriminator_decodes_to_none() {
        let mut payload = [9u8; 8].to_vec();
        payload.extend_from_slice(&[0u8; 16]);
        assert!(decode_event_payload(&payload).is_none());
    }

    #[test]
    fn fill_event_round_trips_through_borsh() {
        let fill = FillEvent {
            market: solana_pubkey::Pubkey::new_unique(),
            taker: solana_pubkey::Pubkey::new_unique(),
            leader: solana_pubkey::Pubkey::new_unique(),
            quote_authority: solana_pubkey::Pubkey::new_unique(),
            side: 1,
            pad: [0; 7],
            sector_idx: 3,
            level_idx: 7,
            fill_base: 1_000,
            fill_quote: 2_000,
            fill_price: 42_000_000,
            pad2: [0; 4],
            base_atoms_after: 9,
            quote_atoms_after: 11,
            nonce_after: 13,
            taker_fee_atoms: 5,
        };
        let mut payload = FILL.to_vec();
        borsh::to_writer(&mut payload, &fill).unwrap();
        assert_eq!(
            decode_event_payload(&payload),
            Some(DropsetEvent::Fill(fill))
        );
    }

    // ── minimal sha256, test-only (avoids a crate dependency) ──────────
    fn sha256(data: &[u8]) -> [u8; 32] {
        const K: [u32; 64] = [
            0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
            0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
            0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
            0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
            0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
            0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
            0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
            0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
            0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
            0xc67178f2,
        ];
        let mut h: [u32; 8] = [
            0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
            0x5be0cd19,
        ];
        let mut msg = data.to_vec();
        let bit_len = (data.len() as u64) * 8;
        msg.push(0x80);
        while msg.len() % 64 != 56 {
            msg.push(0);
        }
        msg.extend_from_slice(&bit_len.to_be_bytes());
        for chunk in msg.chunks_exact(64) {
            let mut w = [0u32; 64];
            for (i, word) in chunk.chunks_exact(4).enumerate() {
                w[i] = u32::from_be_bytes(word.try_into().unwrap());
            }
            for i in 16..64 {
                let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
                let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
                w[i] = w[i - 16]
                    .wrapping_add(s0)
                    .wrapping_add(w[i - 7])
                    .wrapping_add(s1);
            }
            let mut v = h;
            for i in 0..64 {
                let s1 = v[4].rotate_right(6) ^ v[4].rotate_right(11) ^ v[4].rotate_right(25);
                let ch = (v[4] & v[5]) ^ ((!v[4]) & v[6]);
                let t1 = v[7]
                    .wrapping_add(s1)
                    .wrapping_add(ch)
                    .wrapping_add(K[i])
                    .wrapping_add(w[i]);
                let s0 = v[0].rotate_right(2) ^ v[0].rotate_right(13) ^ v[0].rotate_right(22);
                let maj = (v[0] & v[1]) ^ (v[0] & v[2]) ^ (v[1] & v[2]);
                let t2 = s0.wrapping_add(maj);
                v[7] = v[6];
                v[6] = v[5];
                v[5] = v[4];
                v[4] = v[3].wrapping_add(t1);
                v[3] = v[2];
                v[2] = v[1];
                v[1] = v[0];
                v[0] = t1.wrapping_add(t2);
            }
            for i in 0..8 {
                h[i] = h[i].wrapping_add(v[i]);
            }
        }
        let mut out = [0u8; 32];
        for (i, word) in h.iter().enumerate() {
            out[i * 4..i * 4 + 4].copy_from_slice(&word.to_be_bytes());
        }
        out
    }
}
