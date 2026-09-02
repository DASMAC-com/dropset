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
//! anchor scheme) and cover every entry in the `events` list of the
//! generated IDL (`sdk/idl/dropset.json`). The `tests` module pins that
//! both ways: `discriminators_match_anchor_scheme` derives each constant
//! from its struct name, and `every_idl_event_is_decoded` reads the IDL
//! itself and fails when an event is added that this codec does not
//! handle — the drift a name list alone cannot catch, since a list only
//! checks the names already on it. They are kept as constants here for the
//! same reason the account discriminators are (e.g.
//! [`crate::accounts::MARKET_HEADER_DISCRIMINATOR`]) — a decoder shouldn't
//! hash at runtime — and are `pub` so a consumer names them rather than
//! hand-copying the bytes.

// cspell:word undecoded

use crate::types::{
    CloseMarketTreasuryEvent, CloseRegistryFeeVaultEvent, CloseVaultEvent, CreateVaultEvent,
    DepositEvent, FillEvent, FreezeVaultEvent, PlatformFeeEvent, RealizeEvent,
    SetDefaultFeeConfigEvent, SetMarketFeeConfigEvent, SetMaxPlatformFeeEvent,
    SetMinLeaderShareEvent, SetRegistryDefaultsEvent, SetTakerFeeEvent, SweepResidualEvent,
    WithdrawEvent,
};
use borsh::BorshDeserialize;
use solana_pubkey::Pubkey;
use std::str::FromStr;

/// The anchor v2 `emit_cpi!` self-CPI tag, little-endian — the 8-byte
/// prefix on every event inner-instruction's data (`0x1d9acb512ea545e4`).
///
/// Hand-copied so this crate stays free of the on-chain `anchor_lang_v2`
/// dependency. The program test crate's `sdk_event_tag_matches_anchor` pins
/// this literal to `anchor_lang_v2::event::EVENT_IX_TAG_LE`, so a fork bump
/// that moved the tag fails that test (or the build, if it also renamed the
/// constant) rather than silently zeroing event decoding here and in the
/// indexer.
pub const EVENT_IX_TAG: u64 = 0x1d9a_cb51_2ea5_45e4;

/// [`EVENT_IX_TAG`] as the little-endian byte prefix to match on.
pub const EVENT_IX_TAG_LE: [u8; 8] = EVENT_IX_TAG.to_le_bytes();

/// Length of the discriminator that follows the tag.
pub const DISCRIMINATOR_LEN: usize = 8;

// Event discriminators (sha256("event:<Name>")[..8]), one per entry in the
// IDL `events` list, pinned both ways by the test module. Each carries a
// doc comment because they are `pub`: the module doc justifies exporting
// them so a consumer names one rather than hand-copying the bytes, and a
// consumer reading rustdoc should see what each one identifies.
/// Discriminator for `CloseMarketTreasuryEvent`.
pub const CLOSE_MARKET_TREASURY_EVENT_DISCRIMINATOR: [u8; 8] =
    [234, 64, 141, 172, 128, 178, 239, 232];
/// Discriminator for `CloseRegistryFeeVaultEvent`.
pub const CLOSE_REGISTRY_FEE_VAULT_EVENT_DISCRIMINATOR: [u8; 8] =
    [82, 31, 124, 13, 220, 141, 65, 50];
/// Discriminator for `CloseVaultEvent`.
pub const CLOSE_VAULT_EVENT_DISCRIMINATOR: [u8; 8] = [35, 37, 158, 74, 115, 93, 175, 136];
/// Discriminator for `CreateVaultEvent`.
pub const CREATE_VAULT_EVENT_DISCRIMINATOR: [u8; 8] = [42, 221, 241, 92, 177, 139, 118, 240];
/// Discriminator for `DepositEvent`.
pub const DEPOSIT_EVENT_DISCRIMINATOR: [u8; 8] = [120, 248, 61, 83, 31, 142, 107, 144];
/// Discriminator for `FillEvent`.
pub const FILL_EVENT_DISCRIMINATOR: [u8; 8] = [13, 89, 41, 228, 105, 178, 45, 112];
/// Discriminator for `FreezeVaultEvent`.
pub const FREEZE_VAULT_EVENT_DISCRIMINATOR: [u8; 8] = [9, 180, 143, 223, 189, 20, 1, 74];
/// Discriminator for `PlatformFeeEvent`.
pub const PLATFORM_FEE_EVENT_DISCRIMINATOR: [u8; 8] = [188, 157, 159, 156, 113, 199, 229, 159];
/// Discriminator for `RealizeEvent`.
pub const REALIZE_EVENT_DISCRIMINATOR: [u8; 8] = [255, 60, 160, 248, 4, 188, 32, 33];
/// Discriminator for `SetDefaultFeeConfigEvent`.
pub const SET_DEFAULT_FEE_CONFIG_EVENT_DISCRIMINATOR: [u8; 8] =
    [173, 121, 245, 191, 189, 52, 211, 216];
/// Discriminator for `SetMarketFeeConfigEvent`.
pub const SET_MARKET_FEE_CONFIG_EVENT_DISCRIMINATOR: [u8; 8] = [29, 171, 38, 30, 62, 131, 204, 214];
/// Discriminator for `SetMaxPlatformFeeEvent`.
pub const SET_MAX_PLATFORM_FEE_EVENT_DISCRIMINATOR: [u8; 8] = [45, 40, 179, 93, 26, 27, 196, 209];
/// Discriminator for `SetMinLeaderShareEvent`.
pub const SET_MIN_LEADER_SHARE_EVENT_DISCRIMINATOR: [u8; 8] =
    [159, 194, 164, 181, 227, 131, 179, 105];
/// Discriminator for `SetRegistryDefaultsEvent`.
pub const SET_REGISTRY_DEFAULTS_EVENT_DISCRIMINATOR: [u8; 8] = [138, 35, 107, 189, 236, 175, 31, 9];
/// Discriminator for `SetTakerFeeEvent`.
pub const SET_TAKER_FEE_EVENT_DISCRIMINATOR: [u8; 8] = [175, 232, 242, 29, 241, 48, 172, 41];
/// Discriminator for `SweepResidualEvent`.
pub const SWEEP_RESIDUAL_EVENT_DISCRIMINATOR: [u8; 8] = [10, 97, 22, 134, 106, 210, 95, 7];
/// Discriminator for `WithdrawEvent`.
pub const WITHDRAW_EVENT_DISCRIMINATOR: [u8; 8] = [22, 9, 133, 26, 160, 44, 71, 192];

/// Every event the program emits via `emit_cpi!`, decoded into its
/// generated struct: the ones the indexer rolls up (fills, the
/// liquidity-flow events), the admin retuning events the teardown path
/// reconstructs from history (see `interface.md` §1), and the
/// custody-and-payout records — `PlatformFee` is an integrator's only
/// on-chain receipt, and the two `Close*` events are the only on-chain
/// statement of where a closed treasury's atoms went.
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
    SetMaxPlatformFee(SetMaxPlatformFeeEvent),
    PlatformFee(PlatformFeeEvent),
    SweepResidual(SweepResidualEvent),
    CloseMarketTreasury(CloseMarketTreasuryEvent),
    CloseRegistryFeeVault(CloseRegistryFeeVaultEvent),
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
            Self::SetMaxPlatformFee(_) => "SetMaxPlatformFeeEvent",
            Self::PlatformFee(_) => "PlatformFeeEvent",
            Self::SweepResidual(_) => "SweepResidualEvent",
            Self::CloseMarketTreasury(_) => "CloseMarketTreasuryEvent",
            Self::CloseRegistryFeeVault(_) => "CloseRegistryFeeVaultEvent",
        }
    }
}

/// Why a tag-stripped event payload could not be decoded.
///
/// These were once one bare `None`, which made three very different
/// conditions indistinguishable — and the interesting two are both *drift*
/// signals, invisible by construction while they collapsed together.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DecodeError {
    /// Shorter than a discriminator, so it names no event.
    TooShort,
    /// Well-formed, but no variant claims this discriminator: the program
    /// emitted an event this build does not know. Carries the bytes so a
    /// consumer can log which one.
    UnknownDiscriminator([u8; 8]),
    /// The discriminator matched and the body failed borsh.
    BodyDecode,
    /// The body decoded but left bytes unread — the shape on chain is
    /// wider than the shape this build reads, which is what appending a
    /// field to an existing event looks like from here.
    ///
    /// A plain `deserialize` reads a prefix and never checks the
    /// remainder, so without this the added field is silently discarded
    /// and the event still decodes as the old shape. In-repo that drift is
    /// caught by the IDL and client-diff gates; what those cannot reach is
    /// a **running binary against an already-upgraded on-chain program**,
    /// which is the case this variant exists for.
    TrailingBytes {
        /// How many bytes the body carried beyond the decoded struct.
        unread: usize,
    },
}

impl std::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TooShort => write!(f, "payload shorter than an event discriminator"),
            Self::UnknownDiscriminator(disc) => {
                write!(f, "no event claims discriminator {disc:?}")
            }
            Self::BodyDecode => write!(f, "event body failed to deserialize"),
            Self::TrailingBytes { unread } => write!(
                f,
                "event body left {unread} byte(s) unread — the on-chain shape is \
                 wider than this build reads"
            ),
        }
    }
}

impl std::error::Error for DecodeError {}

/// Decode one tag-stripped event payload (`[discriminator(8)][body]`),
/// reporting *why* it failed.
///
/// The body decodes via borsh against the generated struct; for the
/// bytemuck `FillEvent` the generated struct's explicit padding fields make
/// the borsh read byte-identical to the on-chain `repr(C)` bytes. The
/// decode also requires the body to be fully consumed, matching the
/// program's own test decoder — which asserts the same thing, with the
/// comment that it "catches a wire-format drift (an added / reordered /
/// wrong-width field) that a field-by-field read would otherwise mask".
pub fn try_decode_event_payload(payload: &[u8]) -> Result<DropsetEvent, DecodeError> {
    if payload.len() < DISCRIMINATOR_LEN {
        return Err(DecodeError::TooShort);
    }
    let (disc, mut body) = payload.split_at(DISCRIMINATOR_LEN);
    // Not a second `TooShort` origin: the length guard above already
    // returned, so `split_at(DISCRIMINATOR_LEN)` provably yields 8 bytes.
    let disc: [u8; 8] = disc.try_into().expect("split_at guarantees 8 bytes");
    macro_rules! decode {
        ($variant:ident, $ty:ty) => {{
            let decoded = <$ty>::deserialize(&mut body).map_err(|_| DecodeError::BodyDecode)?;
            if !body.is_empty() {
                return Err(DecodeError::TrailingBytes { unread: body.len() });
            }
            DropsetEvent::$variant(decoded)
        }};
    }
    let event = match disc {
        FILL_EVENT_DISCRIMINATOR => decode!(Fill, FillEvent),
        DEPOSIT_EVENT_DISCRIMINATOR => decode!(Deposit, DepositEvent),
        WITHDRAW_EVENT_DISCRIMINATOR => decode!(Withdraw, WithdrawEvent),
        CREATE_VAULT_EVENT_DISCRIMINATOR => decode!(CreateVault, CreateVaultEvent),
        CLOSE_VAULT_EVENT_DISCRIMINATOR => decode!(CloseVault, CloseVaultEvent),
        FREEZE_VAULT_EVENT_DISCRIMINATOR => decode!(FreezeVault, FreezeVaultEvent),
        REALIZE_EVENT_DISCRIMINATOR => decode!(Realize, RealizeEvent),
        SET_MIN_LEADER_SHARE_EVENT_DISCRIMINATOR => {
            decode!(SetMinLeaderShare, SetMinLeaderShareEvent)
        }
        SET_MARKET_FEE_CONFIG_EVENT_DISCRIMINATOR => {
            decode!(SetMarketFeeConfig, SetMarketFeeConfigEvent)
        }
        SET_TAKER_FEE_EVENT_DISCRIMINATOR => decode!(SetTakerFee, SetTakerFeeEvent),
        SET_DEFAULT_FEE_CONFIG_EVENT_DISCRIMINATOR => {
            decode!(SetDefaultFeeConfig, SetDefaultFeeConfigEvent)
        }
        SET_REGISTRY_DEFAULTS_EVENT_DISCRIMINATOR => {
            decode!(SetRegistryDefaults, SetRegistryDefaultsEvent)
        }
        SET_MAX_PLATFORM_FEE_EVENT_DISCRIMINATOR => {
            decode!(SetMaxPlatformFee, SetMaxPlatformFeeEvent)
        }
        PLATFORM_FEE_EVENT_DISCRIMINATOR => decode!(PlatformFee, PlatformFeeEvent),
        SWEEP_RESIDUAL_EVENT_DISCRIMINATOR => decode!(SweepResidual, SweepResidualEvent),
        CLOSE_MARKET_TREASURY_EVENT_DISCRIMINATOR => {
            decode!(CloseMarketTreasury, CloseMarketTreasuryEvent)
        }
        CLOSE_REGISTRY_FEE_VAULT_EVENT_DISCRIMINATOR => {
            decode!(CloseRegistryFeeVault, CloseRegistryFeeVaultEvent)
        }
        unknown => return Err(DecodeError::UnknownDiscriminator(unknown)),
    };
    Ok(event)
}

/// [`try_decode_event_payload`], discarding the reason.
///
/// The right form for a caller that wants **one** event variant and has a
/// fallback when it gets nothing — the maker-bot and the TUI both walk
/// inner instructions looking only for `Fill`, so "this blob is not the
/// event I want" is their ordinary case rather than a signal, and both
/// reconstruct from inventory when a walk yields nothing.
///
/// Prefer the `try_` form when a failure is *actionable*: an indexer
/// persisting every event has nothing to fall back on, so a blob that is
/// tagged as ours and still will not decode is drift worth logging rather
/// than dropping silently.
///
/// **Note for either caller:** [`try_decode_event_payload`] now requires
/// the body to be fully consumed, so a trailing field appended on chain
/// makes an event stop decoding here instead of silently decoding as its
/// old, narrower shape. That is the intended direction — a truncated fill
/// is worse than no fill — but it is a behavior change, and `.ok()`
/// discards the [`DecodeError::TrailingBytes`] that would explain it.
pub fn decode_event_payload(payload: &[u8]) -> Option<DropsetEvent> {
    try_decode_event_payload(payload).ok()
}

/// Strip the `EVENT_IX_TAG_LE` prefix from one inner-instruction `data`,
/// yielding the `[discriminator][body]` payload — or `None` if this inner
/// instruction is not a Dropset event-CPI.
pub fn strip_event_tag(inner_ix_data: &[u8]) -> Option<&[u8]> {
    inner_ix_data
        .strip_prefix(&EVENT_IX_TAG_LE)
        .filter(|payload| payload.len() >= DISCRIMINATOR_LEN)
}

/// Assemble a transaction's full account-key list in the order an
/// instruction's `program_id_index` addresses: the message's static keys
/// first, then the address-lookup-table loaded addresses (writable, then
/// readonly).
///
/// Returns `None` if a loaded address won't parse — the caller then cannot
/// safely attribute any event in the transaction and must fail closed
/// rather than trust an unverified emitter.
///
/// The loaded addresses are taken as base58 string slices rather than as
/// `solana_transaction_status::UiLoadedAddresses` so this crate stays free
/// of the transaction-status dependency (the same reason
/// [`EVENT_IX_TAG`] is hand-copied rather than imported from
/// `anchor_lang_v2`). Every caller has the two `Vec<String>` fields to
/// hand; unwrapping their `OptionSerializer` is three lines at the call
/// site and keeps this the one shared, tested implementation.
pub fn full_account_keys(
    static_keys: &[Pubkey],
    loaded_writable: &[String],
    loaded_readonly: &[String],
) -> Option<Vec<Pubkey>> {
    let mut keys = static_keys.to_vec();
    for encoded in loaded_writable.iter().chain(loaded_readonly.iter()) {
        keys.push(Pubkey::from_str(encoded).ok()?);
    }
    Some(keys)
}

/// Resolve one inner instruction's `program_id_index` against the
/// transaction's full account-key list (see [`full_account_keys`]).
///
/// `None` when the index is out of range, which is the fail-closed answer:
/// a blob whose emitter cannot be resolved must not be attributed to any
/// program.
pub fn emitting_program(account_keys: &[Pubkey], program_id_index: u8) -> Option<Pubkey> {
    account_keys.get(program_id_index as usize).copied()
}

/// Whether this inner instruction was emitted by `program_id`.
///
/// **This is the check that makes an event-CPI blob trustworthy.** Both
/// halves of the `[tag][discriminator]` envelope are public and carry no
/// Dropset-specific secret — the tag is an anchor-wide constant and the
/// discriminator is `sha256("event:<Name>")[..8]`, which any program with
/// a struct of that name reproduces exactly — so anyone can forge the
/// bytes. The emitting program id is the only thing `emit_cpi!`'s self-CPI
/// actually authenticates, and `getSignaturesForAddress` is
/// address-indexed rather than invoked-program-indexed, so a transaction
/// that merely *references* the program is polled and hydrated. Without
/// this check a foreign program's `emit_cpi!` of a Dropset-shaped payload
/// is indistinguishable from a real event.
pub fn emitted_by(account_keys: &[Pubkey], program_id_index: u8, program_id: &Pubkey) -> bool {
    emitting_program(account_keys, program_id_index).as_ref() == Some(program_id)
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

    /// Every discriminator this codec decodes, paired with the event name
    /// it claims to be. The single list both name-scheme and IDL-coverage
    /// tests read, so a variant added to one is checked by both.
    const DECODED: &[([u8; 8], &str)] = &[
        (
            CLOSE_MARKET_TREASURY_EVENT_DISCRIMINATOR,
            "CloseMarketTreasuryEvent",
        ),
        (
            CLOSE_REGISTRY_FEE_VAULT_EVENT_DISCRIMINATOR,
            "CloseRegistryFeeVaultEvent",
        ),
        (CLOSE_VAULT_EVENT_DISCRIMINATOR, "CloseVaultEvent"),
        (CREATE_VAULT_EVENT_DISCRIMINATOR, "CreateVaultEvent"),
        (DEPOSIT_EVENT_DISCRIMINATOR, "DepositEvent"),
        (FILL_EVENT_DISCRIMINATOR, "FillEvent"),
        (FREEZE_VAULT_EVENT_DISCRIMINATOR, "FreezeVaultEvent"),
        (PLATFORM_FEE_EVENT_DISCRIMINATOR, "PlatformFeeEvent"),
        (REALIZE_EVENT_DISCRIMINATOR, "RealizeEvent"),
        (
            SET_DEFAULT_FEE_CONFIG_EVENT_DISCRIMINATOR,
            "SetDefaultFeeConfigEvent",
        ),
        (
            SET_MARKET_FEE_CONFIG_EVENT_DISCRIMINATOR,
            "SetMarketFeeConfigEvent",
        ),
        (
            SET_MAX_PLATFORM_FEE_EVENT_DISCRIMINATOR,
            "SetMaxPlatformFeeEvent",
        ),
        (
            SET_MIN_LEADER_SHARE_EVENT_DISCRIMINATOR,
            "SetMinLeaderShareEvent",
        ),
        (
            SET_REGISTRY_DEFAULTS_EVENT_DISCRIMINATOR,
            "SetRegistryDefaultsEvent",
        ),
        (SET_TAKER_FEE_EVENT_DISCRIMINATOR, "SetTakerFeeEvent"),
        (SWEEP_RESIDUAL_EVENT_DISCRIMINATOR, "SweepResidualEvent"),
        (WITHDRAW_EVENT_DISCRIMINATOR, "WithdrawEvent"),
    ];

    #[test]
    fn discriminators_match_anchor_scheme() {
        for (constant, name) in DECODED {
            assert_eq!(*constant, anchor_event_discriminator(name), "{name}");
        }
    }

    /// **The structural pin.** Read the IDL's own `events` list and require
    /// every entry to have a constant here carrying the discriminator the
    /// IDL records.
    ///
    /// Note what this does and does not pin. It compares the IDL against
    /// [`DECODED`], a hand-written list — so it catches an event *added*
    /// to the IDL and never given a constant, which is the gap that let
    /// this codec fall five events behind the program. It does **not** by
    /// itself prove `try_decode_event_payload` dispatches on those
    /// constants; deleting a `match disc` arm leaves this test green.
    /// That mutation is caught, but downstream, by the indexer's
    /// `every_event_payload_carries_every_field_the_idl_declares`, which
    /// drives the decoder over all 17 events — so `cargo test -p
    /// dropset-sdk` alone does not pin dispatch, and this crate is
    /// publishable on its own.
    ///
    /// This is the test whose absence let the codec fall five events behind
    /// the program. `discriminators_match_anchor_scheme` cannot catch that:
    /// it iterates a hand-written list, so an event missing from the list is
    /// missing from the check too. Two of the five were added in a single
    /// commit that regenerated every other client surface — IDL, Rust
    /// types, TS twins — and touched neither this codec nor the indexer,
    /// with nothing to fail.
    ///
    /// To deliberately leave an event undecoded, add it to `EXEMPT` with a
    /// written reason. An unexplained gap is the failure this guards.
    #[test]
    fn every_idl_event_is_decoded() {
        /// Events the codec deliberately does not decode, each with why.
        const EXEMPT: &[(&str, &str)] = &[];

        let idl: serde_json::Value = serde_json::from_str(include_str!("../../idl/dropset.json"))
            .expect("the generated IDL parses");
        let events = idl["events"]
            .as_array()
            .expect("the IDL carries an events array");
        assert!(!events.is_empty(), "no events read — is the path right?");

        for event in events {
            let name = event["name"].as_str().expect("every event has a name");
            if let Some((_, reason)) = EXEMPT.iter().find(|(exempt, _)| *exempt == name) {
                assert!(!reason.is_empty(), "{name} is exempt with no reason");
                continue;
            }
            let expected: Vec<u8> = event["discriminator"]
                .as_array()
                .expect("every event has a discriminator")
                .iter()
                .map(|b| b.as_u64().expect("a discriminator byte") as u8)
                .collect();

            let found = DECODED
                .iter()
                .find(|(_, decoded)| *decoded == name)
                .unwrap_or_else(|| {
                    panic!(
                        "the IDL declares {name} but this codec does not decode it — \
                         add a DropsetEvent variant, or an EXEMPT entry saying why not"
                    )
                });
            assert_eq!(found.0.to_vec(), expected, "{name} discriminator");
        }

        assert_eq!(
            DECODED.len() + EXEMPT.len(),
            events.len(),
            "the codec decodes an event the IDL does not declare"
        );
    }

    #[test]
    fn strip_rejects_non_event_data() {
        assert!(strip_event_tag(&[1, 2, 3]).is_none());
        let mut tagged = EVENT_IX_TAG_LE.to_vec();
        tagged.extend_from_slice(&FILL_EVENT_DISCRIMINATOR);
        // tag present, payload is exactly a discriminator (== DISCRIMINATOR_LEN)
        assert!(strip_event_tag(&tagged).is_some());
    }

    #[test]
    fn unknown_discriminator_decodes_to_none() {
        let mut payload = [9u8; 8].to_vec();
        payload.extend_from_slice(&[0u8; 16]);
        assert!(decode_event_payload(&payload).is_none());
        // The reason is now recoverable, and carries which discriminator —
        // this is the drift signal that used to collapse into a bare None.
        assert_eq!(
            try_decode_event_payload(&payload),
            Err(DecodeError::UnknownDiscriminator([9u8; 8]))
        );
    }

    /// The three failure conditions are distinguishable, not one `None`.
    #[test]
    fn each_failure_reports_its_own_reason() {
        assert_eq!(
            try_decode_event_payload(&[1, 2, 3]),
            Err(DecodeError::TooShort)
        );
        // Right discriminator, body too short to be a FillEvent.
        let mut truncated = FILL_EVENT_DISCRIMINATOR.to_vec();
        truncated.extend_from_slice(&[0u8; 4]);
        assert_eq!(
            try_decode_event_payload(&truncated),
            Err(DecodeError::BodyDecode)
        );
    }

    /// A field appended on chain leaves bytes unread. A plain `deserialize`
    /// reads the prefix and silently discards the addition, so the event
    /// would decode as the old shape and look perfectly healthy; requiring
    /// the body to be consumed is what turns that into a signal.
    #[test]
    fn a_wider_body_than_this_build_reads_is_rejected() {
        let event = SetTakerFeeEvent {
            market: Pubkey::new_unique(),
            taker_fee: 7,
        };
        let mut payload = SET_TAKER_FEE_EVENT_DISCRIMINATOR.to_vec();
        borsh::to_writer(&mut payload, &event).unwrap();
        // Exact body still decodes.
        assert!(try_decode_event_payload(&payload).is_ok());

        // The same event with two extra bytes appended, as a program that
        // grew the struct would emit it.
        payload.extend_from_slice(&[0xab, 0xcd]);
        assert_eq!(
            try_decode_event_payload(&payload),
            Err(DecodeError::TrailingBytes { unread: 2 })
        );
    }

    /// The five events the codec used to drop decode now, including the
    /// two whose absence meant protocol revenue leaving custody and every
    /// integrator payout were recorded as nothing at all.
    #[test]
    fn the_custody_and_payout_events_decode() {
        let fee = PlatformFeeEvent {
            market: Pubkey::new_unique(),
            taker: Pubkey::new_unique(),
            fee_authority: Pubkey::new_unique(),
            mint: Pubkey::new_unique(),
            atoms: 1_234,
            platform_fee_bps: 25,
        };
        let mut payload = PLATFORM_FEE_EVENT_DISCRIMINATOR.to_vec();
        borsh::to_writer(&mut payload, &fee).unwrap();
        assert_eq!(
            try_decode_event_payload(&payload),
            Ok(DropsetEvent::PlatformFee(fee))
        );

        let closed = CloseRegistryFeeVaultEvent {
            fee_mint: Pubkey::new_unique(),
            token_recipient: Pubkey::new_unique(),
            rent_recipient: Pubkey::new_unique(),
            collected: 9_999,
        };
        let mut payload = CLOSE_REGISTRY_FEE_VAULT_EVENT_DISCRIMINATOR.to_vec();
        borsh::to_writer(&mut payload, &closed).unwrap();
        assert_eq!(
            try_decode_event_payload(&payload),
            Ok(DropsetEvent::CloseRegistryFeeVault(closed))
        );
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
        let mut payload = FILL_EVENT_DISCRIMINATOR.to_vec();
        borsh::to_writer(&mut payload, &fill).unwrap();
        assert_eq!(
            decode_event_payload(&payload),
            Some(DropsetEvent::Fill(fill))
        );
    }

    /// `program_id_index` addresses static keys first, then loaded
    /// writable, then loaded readonly — the order a transaction with a
    /// lookup table composes its account keys.
    #[test]
    fn resolves_keys_static_then_writable_then_readonly() {
        let static_a = Pubkey::new_from_array([10; 32]);
        let static_b = Pubkey::new_from_array([11; 32]);
        let writable = Pubkey::new_from_array([12; 32]);
        let readonly = Pubkey::new_from_array([13; 32]);
        let keys = full_account_keys(
            &[static_a, static_b],
            &[writable.to_string()],
            &[readonly.to_string()],
        )
        .expect("all parse");
        assert_eq!(keys, vec![static_a, static_b, writable, readonly]);
    }

    /// With no lookup-table loads the static keys stand alone.
    #[test]
    fn resolves_keys_without_loaded_addresses() {
        let static_a = Pubkey::new_from_array([10; 32]);
        let keys = full_account_keys(&[static_a], &[], &[]).expect("static-only resolves");
        assert_eq!(keys, vec![static_a]);
    }

    /// A loaded address that won't parse means the full list cannot be
    /// trusted, so no event in the transaction may be attributed.
    #[test]
    fn malformed_loaded_address_resolves_to_none() {
        assert!(full_account_keys(
            &[Pubkey::new_from_array([10; 32])],
            &["not-a-pubkey".to_string()],
            &[],
        )
        .is_none());
    }

    /// The spoof this check exists for: a byte-identical event emitted by
    /// any program other than ours is not ours. Both halves of the
    /// `[tag][discriminator]` envelope are public, so the emitter is the
    /// only discriminating signal.
    #[test]
    fn emitted_by_accepts_only_the_named_program() {
        let ours = Pubkey::new_from_array([1; 32]);
        let foreign = Pubkey::new_from_array([9; 32]);
        let account_keys = vec![ours, foreign];

        assert!(emitted_by(&account_keys, 0, &ours));
        assert!(
            !emitted_by(&account_keys, 1, &ours),
            "forged by a foreigner"
        );
        assert_eq!(emitting_program(&account_keys, 1), Some(foreign));
    }

    /// An out-of-range `program_id_index` (e.g. an unresolved key)
    /// resolves to no account, so the event fails closed rather than
    /// indexing out of bounds.
    #[test]
    fn emitted_by_rejects_an_out_of_range_program_index() {
        let ours = Pubkey::new_from_array([1; 32]);
        assert!(!emitted_by(&[ours], 7, &ours));
        assert_eq!(emitting_program(&[ours], 7), None);
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
        for chunk in msg.as_chunks::<64>().0 {
            let mut w = [0u32; 64];
            for (i, word) in chunk.as_chunks::<4>().0.iter().enumerate() {
                w[i] = u32::from_be_bytes(*word);
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
