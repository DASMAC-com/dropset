// cspell:word nocapture
//! Rust↔ASM parity for the two quote-write fast paths —
//! `set_reference_price` and `set_liquidity_profile` — plus the offset
//! assertions that pin what `src/asm/entrypoint.s` hardcodes.
//!
//! Two layers:
//!
//! 1. [`asm_offsets_match_layout`] re-derives every offset the assembly
//!    hardcodes — from the real `#[repr(C)]` / `#[account]` layout and
//!    agave's aligned account serialization — and asserts it against the
//!    literal the `.s` uses. A `layout.rs` reorder / width change (or a
//!    wrong ABI offset) breaks this test rather than silently mis-stamping
//!    on-chain. Always runs.
//!
//! 2. The `*_parity` tests deploy the reference (feature-off) build beside
//!    the default asm `.so` and push identical inputs through both,
//!    asserting the assembly and the Rust kernels produce the same market
//!    bytes and the same domain error codes. They **skip** (rather than
//!    fail) when the reference oracle `.so` is absent, so a plain
//!    `cargo test` — which only builds the default asm `.so` — stays green.
//!    `make program-parity` builds both.
//!
//! Only *legitimate* inputs are compared for byte-parity: a real wallet
//! signer carries no data, so the assembly's signer-empty / writable
//! layout guards never fire on inputs the reference build would accept.
//! Those structural guards are asm-only (they keep the market at a static
//! input offset) and are intentionally not part of the parity contract —
//! see the architecture spec's **SetReferencePrice**.
//!
//! The same carve-out covers **short instruction data**, which the assembly
//! deliberately does not bound: the reference build rejects it at
//! deserialization, while the fast path either faults or copies trailing
//! input-buffer bytes into the caller's own ladder. Both are self-inflicted
//! and neither can escape the caller's own bounds-checked sector (see
//! `entrypoint.s`'s note under the error codes), so no case below feeds a
//! truncated payload to the two builds expecting equal outcomes.
//!
//! Keep new parity cases **in this file**: the required `Tests (asm parity)`
//! CI job is the only one that builds the reference oracle, and it runs
//! exactly `--test asm_parity`. A case in another test binary would only
//! ever run against the default asm `.so`, so a divergence could pass CI.

mod common;

use anchor_lang_v2::InstructionData;
use anchor_v2_testing::Signer;
use common::fixture::{simple_profile, Fixture, PROFILE_BYTES};
use core::mem::{offset_of, size_of};
use dropset::{LiquidityProfile, Market, MarketHeader, Price, ReferencePrice, Vault};
use solana_pubkey::Pubkey;

// ── agave aligned account serialization (the input-buffer ABI) ──────────
// A serialized account record is
//   [RuntimeAccount header(88) | data | MAX_PERMITTED_DATA_INCREASE(10240)
//    | pad-to-8 | rent_epoch(8)]
// preceded by an 8-byte account count. These mirror the constants in
// `entrypoint.s`; the asserts below tie them to what the assembly encodes.
const NUM_ACCOUNTS_SIZE: usize = 8;
const ACCT_HEADER_SIZE: usize = 88;
const MAX_PERMITTED_DATA_INCREASE: usize = 10240;
const RENT_EPOCH_SIZE: usize = 8;
// RuntimeAccount header field offsets.
const HDR_IS_SIGNER: usize = 1;
const HDR_IS_WRITABLE: usize = 2;
const HDR_PUBKEY: usize = 8;
const HDR_DATA_LEN: usize = 80;
const HDR_DATA: usize = 88;
// Anchor account discriminator.
const DISC_SIZE: usize = 8;

#[test]
fn asm_offsets_match_layout() {
    // account 0 (signer) field offsets: base is num_accounts(8).
    assert_eq!(NUM_ACCOUNTS_SIZE + HDR_IS_SIGNER, 9, "SIGNER_IS_SIGNER_OFF");
    assert_eq!(NUM_ACCOUNTS_SIZE + HDR_PUBKEY, 16, "SIGNER_PUBKEY_OFF");
    assert_eq!(NUM_ACCOUNTS_SIZE + HDR_DATA_LEN, 88, "SIGNER_DATA_LEN_OFF");

    // account 1 (market) base, with the signer carrying zero data — its
    // data region is therefore just the DATA_INCREASE pad, contributing
    // nothing between the header and the rent-epoch tail.
    let market_base =
        NUM_ACCOUNTS_SIZE + ACCT_HEADER_SIZE + MAX_PERMITTED_DATA_INCREASE + RENT_EPOCH_SIZE;
    assert_eq!(market_base, 10344, "MARKET_BASE");
    assert_eq!(
        market_base + HDR_IS_WRITABLE,
        10346,
        "MARKET_IS_WRITABLE_OFF"
    );
    assert_eq!(market_base + HDR_DATA_LEN, 10424, "MARKET_DATA_LEN_OFF");
    assert_eq!(market_base + HDR_DATA, 10432, "MARKET_DATA_OFF");

    // market data framing: [disc(8)][MarketHeader][len:u32][pad][vaults].
    let data_off = market_base + HDR_DATA;
    assert_eq!(
        data_off + DISC_SIZE + offset_of!(MarketHeader, nonce),
        10440,
        "MARKET_NONCE_OFF"
    );
    assert_eq!(
        data_off + DISC_SIZE + size_of::<MarketHeader>(),
        10693,
        "MARKET_LEN_OFF"
    );
    // `Market::space_for(0)` IS the slab's ITEMS_OFFSET (align_up over the
    // len field to align_of::<Vault>() == 4), so this pins the pad.
    assert_eq!(Market::space_for(0), 268, "SLAB_ITEMS_OFF");
    assert_eq!(size_of::<Vault>(), 560, "VAULT_SIZE");

    // Vault field offsets the two payloads write to.
    assert_eq!(
        offset_of!(Vault, quote_authority),
        40,
        "VAULT_QUOTE_AUTHORITY_OFF"
    );
    let rp = offset_of!(Vault, reference_price);
    assert_eq!(rp + offset_of!(ReferencePrice, stamp), 72, "RP_STAMP_OFF");
    assert_eq!(rp + offset_of!(ReferencePrice, price), 80, "RP_PRICE_OFF");
    assert_eq!(
        rp + offset_of!(ReferencePrice, quote_slot),
        84,
        "RP_QUOTE_SLOT_OFF"
    );
    assert_eq!(offset_of!(Vault, profile), 144, "VAULT_PROFILE_OFF");
    assert_eq!(size_of::<LiquidityProfile>(), 160, "PROFILE_SIZE");

    // Instruction-data layout: the assembly reads `vault_idx` at +1 and
    // hands `sol_memcpy_` a source pointer of +5, so pin those against the
    // *real* serialization rather than re-deriving them arithmetically —
    // encode a recognizable payload and locate it in the wire bytes.
    let probe: [u8; PROFILE_BYTES] = core::array::from_fn(|i| (i % 251 + 1) as u8);
    let wire = dropset::instruction::SetLiquidityProfile {
        vault_idx: 0x0403_0201,
        profile_bytes: probe,
    }
    .data();
    assert_eq!(
        wire.len(),
        1 + size_of::<u32>() + PROFILE_BYTES,
        "ix data len"
    );
    assert_eq!(wire[0], 6, "discriminator");
    assert_eq!(
        &wire[1..5],
        &0x0403_0201u32.to_le_bytes(),
        "IX_VAULT_IDX_OFF"
    );
    assert_eq!(&wire[5..], &probe, "IX_PROFILE_OFF");
    assert_eq!(PROFILE_BYTES, size_of::<LiquidityProfile>());
}

/// The stamped reference price plus the post-stamp market nonce — the
/// observable state the assembly and the kernel must agree on.
type Stamp = (u64, u32, u32, u64);

fn valid_price() -> u32 {
    Price::encode(10_850_000, 0).unwrap().as_u32()
}

fn ref_built() -> bool {
    std::path::Path::new(common::REF_PROGRAM_SO_PATH).exists()
}

/// Open vault 0, stamp `(price_bits, quote_slot)`, and read back
/// `(stamp, price, quote_slot, nonce)`.
fn stamp_and_read(mut f: Fixture, price_bits: u32, quote_slot: u32) -> Stamp {
    let auth = f.authority.pubkey();
    f.create_vault(0, auth, false, Pubkey::default())
        .expect("create_vault");
    let signer = f.authority.insecure_clone();
    f.set_reference_price(&signer, 0, price_bits, quote_slot)
        .expect("set_reference_price");
    let v = f.vault(0);
    (
        v.reference_price.stamp.get(),
        v.reference_price.price.as_u32(),
        v.reference_price.quote_slot.get(),
        f.market_header().nonce.get(),
    )
}

#[test]
fn happy_path_parity() {
    if !ref_built() {
        eprintln!("skipping happy_path_parity: reference oracle absent (`make program-parity`)");
        return;
    }
    // Identical bootstrap + op sequence on each build, so the pre-stamp
    // nonce matches; the resulting stamp is then byte-identical only if the
    // assembly (default build) writes the same bytes the kernel does.
    let reference = stamp_and_read(Fixture::bootstrap_ref(), valid_price(), 7);
    let asm = stamp_and_read(Fixture::bootstrap(), valid_price(), 7);
    assert_eq!(
        reference, asm,
        "asm stamp must byte-match the reference build"
    );
    // And the stamp is what we expect: flush armed over a zero pre-nonce,
    // price + slot stored raw, nonce bumped to 1.
    assert_eq!(
        asm.0,
        dropset::FLUSH_BIT,
        "stamp = pre_nonce(0) | FLUSH_BIT"
    );
    assert_eq!(asm.1, valid_price());
    assert_eq!(asm.2, 7);
    assert_eq!(asm.3, 1, "nonce bumped");
}

#[test]
fn invalid_price_stored_raw_parity() {
    if !ref_built() {
        eprintln!("skipping invalid_price_stored_raw_parity: reference oracle absent");
        return;
    }
    // The write validates neither the price nor the slot; both builds store
    // an invalid significand and a far-future slot verbatim.
    let bits = 5_000_000;
    let reference = stamp_and_read(Fixture::bootstrap_ref(), bits, 1_000_000);
    let asm = stamp_and_read(Fixture::bootstrap(), bits, 1_000_000);
    assert_eq!(reference, asm);
    assert_eq!(asm.1, bits);
    assert_eq!(asm.2, 1_000_000);
}

#[test]
fn unauthorized_parity() {
    if !ref_built() {
        eprintln!("skipping unauthorized_parity: reference oracle absent");
        return;
    }
    let ref_err = unauthorized_err(Fixture::bootstrap_ref());
    let asm_err = unauthorized_err(Fixture::bootstrap());
    // Domain error: both surface DropsetError::Unauthorized (Custom 6005).
    assert!(
        ref_err.contains("Custom(6005)"),
        "reference unauthorized: {ref_err}"
    );
    assert!(
        asm_err.contains("Custom(6005)"),
        "asm unauthorized: {asm_err}"
    );
}

fn unauthorized_err(mut f: Fixture) -> String {
    let auth = f.authority.pubkey();
    f.create_vault(0, auth, false, Pubkey::default())
        .expect("create_vault");
    let stranger = f.funded_keypair(common::SIGNER_FUNDING_LAMPORTS);
    f.set_reference_price(&stranger, 0, valid_price(), 0)
        .expect_err("non quote-authority must reject")
}

#[test]
fn out_of_range_sector_parity() {
    if !ref_built() {
        eprintln!("skipping out_of_range_sector_parity: reference oracle absent");
        return;
    }
    let ref_err = oob_err(Fixture::bootstrap_ref());
    let asm_err = oob_err(Fixture::bootstrap());
    // Domain error: both surface DropsetError::InvalidSectorIndex (6010).
    assert!(ref_err.contains("Custom(6010)"), "reference oob: {ref_err}");
    assert!(asm_err.contains("Custom(6010)"), "asm oob: {asm_err}");
}

fn oob_err(mut f: Fixture) -> String {
    let auth = f.authority.pubkey();
    f.create_vault(0, auth, false, Pubkey::default())
        .expect("create_vault");
    let signer = f.authority.insecure_clone();
    f.set_reference_price(&signer, 99, valid_price(), 0)
        .expect_err("vault_idx past the slab length must reject")
}

// ── set_liquidity_profile (disc 6) ──────────────────────────────────────

/// A distinct value on every level of both sides, so a truncated, shifted,
/// or partially-applied `sol_memcpy_` shows up rather than hiding behind
/// zeroed tail levels. (The little-endian encodings still contain zero
/// bytes — roughly half the blob — so byte-exactness is asserted directly
/// against the stored region, never inferred from which bytes *changed*.)
fn full_ladder() -> [u8; PROFILE_BYTES] {
    let levels: Vec<(u32, u16, u32)> = (0..8)
        .map(|i| (1_000 + i as u32 * 37, 100 + i as u16 * 11, 500 + i as u32))
        .collect();
    let bids: Vec<(u32, u16, u32)> = levels
        .iter()
        .map(|&(o, s, e)| (o + 7, s + 3, e + 9))
        .collect();
    common::fixture::ladder_profile(&levels, &bids)
}

/// The observable state of a profile write: the stored blob, the reference
/// price triple it must leave untouched, and the post-write market nonce.
type ProfileWrite = (Vec<u8>, u64, u32, u32, u64);

/// Open vault 0, stamp a price, then write `profile` and read back
/// everything the two builds must agree on.
fn write_profile_and_read(mut f: Fixture, profile: [u8; PROFILE_BYTES]) -> ProfileWrite {
    let auth = f.authority.pubkey();
    f.create_vault(0, auth, false, Pubkey::default())
        .expect("create_vault");
    let signer = f.authority.insecure_clone();
    f.set_reference_price(&signer, 0, valid_price(), 11)
        .expect("set_reference_price");
    f.set_liquidity_profile(&signer, 0, profile)
        .expect("set_liquidity_profile");
    let v = f.vault(0);
    (
        anchor_lang_v2::bytemuck::bytes_of(&v.profile).to_vec(),
        v.reference_price.stamp.get(),
        v.reference_price.price.as_u32(),
        v.reference_price.quote_slot.get(),
        f.market_header().nonce.get(),
    )
}

#[test]
fn profile_happy_path_parity() {
    if !ref_built() {
        eprintln!("skipping profile_happy_path_parity: reference oracle absent");
        return;
    }
    let ladder = full_ladder();
    let reference = write_profile_and_read(Fixture::bootstrap_ref(), ladder);
    let asm = write_profile_and_read(Fixture::bootstrap(), ladder);
    assert_eq!(
        reference, asm,
        "asm profile write must byte-match the reference build"
    );
    // And it is what we expect: the blob verbatim, flush armed over the
    // pre-write nonce (1, after the price stamp), price / slot preserved.
    assert_eq!(asm.0, ladder, "profile stored verbatim");
    assert_eq!(
        asm.1,
        1 | dropset::FLUSH_BIT,
        "stamp = pre_nonce(1) | FLUSH"
    );
    assert_eq!(asm.2, valid_price(), "reference price untouched");
    assert_eq!(asm.3, 11, "quote_slot untouched");
    assert_eq!(asm.4, 2, "nonce bumped by the profile write");
}

#[test]
fn profile_over_bps_stored_raw_parity() {
    if !ref_built() {
        eprintln!("skipping profile_over_bps_stored_raw_parity: reference oracle absent");
        return;
    }
    // Neither build validates the ladder's contents: an over-cap side (and a
    // far-future expiry) is stored verbatim on both, with the size invariant
    // left to the match-time flush.
    let over = common::fixture::ladder_profile(&[(5_000, 20_000, u32::MAX)], &[(0, 30_000, 0)]);
    let reference = write_profile_and_read(Fixture::bootstrap_ref(), over);
    let asm = write_profile_and_read(Fixture::bootstrap(), over);
    assert_eq!(reference, asm);
    assert_eq!(asm.0, over, "over-cap ladder stored raw");
}

#[test]
fn profile_write_footprint_parity() {
    if !ref_built() {
        eprintln!("skipping profile_write_footprint_parity: reference oracle absent");
        return;
    }
    // Bound the write's blast radius on each build, then compare. A 160-byte
    // `sol_memcpy_` is the one payload here big enough to run off its field,
    // so the assertion is which bytes of the *whole* market account moved:
    // only `market.nonce`, the target sector's `reference_price.stamp`, and
    // its `profile`. A bleed into `remaining`, into `reference_price.price`,
    // or into the neighboring sector shows up as an out-of-range index.
    let ladder = full_ladder();
    let reference = profile_write_footprint(Fixture::bootstrap_ref(), ladder);
    let asm = profile_write_footprint(Fixture::bootstrap(), ladder);
    // The footprint is identity-free (the changed bytes are the nonce, the
    // stamp and the ladder — no pubkeys), so the two fixtures' independently
    // generated keypairs don't make the builds incomparable.
    assert_eq!(
        reference, asm,
        "asm and reference must move the same bytes to the same values"
    );

    // Vault 1 of two is the target; everything outside these three ranges
    // must be untouched.
    let vault1 = common::fixture::vault_byte_offset(1);
    let nonce = 8..8 + size_of::<u64>();
    let stamp = vault1 + offset_of!(Vault, reference_price) + offset_of!(ReferencePrice, stamp);
    let stamp = stamp..stamp + size_of::<u64>();
    let profile = vault1 + offset_of!(Vault, profile);
    let profile = profile..profile + PROFILE_BYTES;
    for (idx, _) in &asm.0 {
        assert!(
            nonce.contains(idx) || stamp.contains(idx) || profile.contains(idx),
            "byte {idx} changed outside nonce / stamp / profile of sector 1"
        );
    }
    // And the payload landed in full. Asserted against the stored region
    // rather than the changed-byte set: a ladder byte that was already zero
    // never shows up as "changed", so the changed set can only ever prove a
    // subset of the copy.
    assert_eq!(asm.2, ladder, "the whole 160-byte blob is in place");
}

/// Open two vaults, write `profile` onto the *second*, and return every
/// `(index, new_value)` the write changed in the market account's data,
/// the post-write nonce, and the target sector's stored profile region.
fn profile_write_footprint(
    mut f: Fixture,
    profile: [u8; PROFILE_BYTES],
) -> (Vec<(usize, u8)>, u64, Vec<u8>) {
    let auth = f.authority.pubkey();
    // Two vaults; the slab allocates them into sectors 0 then 1. The
    // `perf_fee_rate` differs only to keep the two transactions distinct
    // (identical ones in the same slot are rejected as `AlreadyProcessed`).
    f.create_vault(0, auth, false, Pubkey::default())
        .expect("create_vault 0");
    f.create_vault(1, auth, false, Pubkey::default())
        .expect("create_vault 1");
    let signer = f.authority.insecure_clone();
    // Seed sector 0 with a different ladder, so a copy that lands one sector
    // low would overwrite recognizable bytes rather than zeros.
    f.set_liquidity_profile(&signer, 0, simple_profile(5_000, 10_000, u32::MAX))
        .expect("seed vault 0's ladder");

    let before = f.market_data();
    f.set_liquidity_profile(&signer, 1, profile)
        .expect("set_liquidity_profile");
    let after = f.market_data();

    let changed = before
        .iter()
        .zip(after.iter())
        .enumerate()
        .filter(|(_, (b, a))| b != a)
        .map(|(i, (_, &a))| (i, a))
        .collect();
    let stored = common::fixture::vault_byte_offset(1) + offset_of!(Vault, profile);
    (
        changed,
        f.market_header().nonce.get(),
        after[stored..stored + PROFILE_BYTES].to_vec(),
    )
}

#[test]
fn profile_unauthorized_parity() {
    if !ref_built() {
        eprintln!("skipping profile_unauthorized_parity: reference oracle absent");
        return;
    }
    let ref_err = profile_unauthorized_err(Fixture::bootstrap_ref());
    let asm_err = profile_unauthorized_err(Fixture::bootstrap());
    // Domain error: both surface DropsetError::Unauthorized (Custom 6005).
    assert!(
        ref_err.contains("Custom(6005)"),
        "reference unauthorized: {ref_err}"
    );
    assert!(
        asm_err.contains("Custom(6005)"),
        "asm unauthorized: {asm_err}"
    );
}

fn profile_unauthorized_err(mut f: Fixture) -> String {
    let auth = f.authority.pubkey();
    f.create_vault(0, auth, false, Pubkey::default())
        .expect("create_vault");
    let stranger = f.funded_keypair(common::SIGNER_FUNDING_LAMPORTS);
    f.set_liquidity_profile(&stranger, 0, full_ladder())
        .expect_err("non quote-authority must reject")
}

#[test]
fn profile_out_of_range_sector_parity() {
    if !ref_built() {
        eprintln!("skipping profile_out_of_range_sector_parity: reference oracle absent");
        return;
    }
    let ref_err = profile_oob_err(Fixture::bootstrap_ref());
    let asm_err = profile_oob_err(Fixture::bootstrap());
    // Domain error: both surface DropsetError::InvalidSectorIndex (6010).
    assert!(ref_err.contains("Custom(6010)"), "reference oob: {ref_err}");
    assert!(asm_err.contains("Custom(6010)"), "asm oob: {asm_err}");
}

fn profile_oob_err(mut f: Fixture) -> String {
    let auth = f.authority.pubkey();
    f.create_vault(0, auth, false, Pubkey::default())
        .expect("create_vault");
    let signer = f.authority.insecure_clone();
    f.set_liquidity_profile(&signer, 99, full_ladder())
        .expect_err("vault_idx past the slab length must reject")
}

// ── CU report ──────────────────────────────────────────────────────────

/// Compute units for one happy-path stamp.
fn stamp_cu(mut f: Fixture) -> u64 {
    let auth = f.authority.pubkey();
    f.create_vault(0, auth, false, Pubkey::default())
        .expect("create_vault");
    let signer = f.authority.insecure_clone();
    f.set_reference_price_meta(&signer, 0, valid_price(), 0)
        .expect("set_reference_price")
        .compute_units_consumed
}

/// Compute units for one happy-path profile write.
fn profile_cu(mut f: Fixture) -> u64 {
    let auth = f.authority.pubkey();
    f.create_vault(0, auth, false, Pubkey::default())
        .expect("create_vault");
    let signer = f.authority.insecure_clone();
    f.set_liquidity_profile_meta(&signer, 0, full_ladder())
        .expect("set_liquidity_profile")
        .compute_units_consumed
}

/// The CU report the issues ask for: measure each quote write on both builds
/// and print the assembly-vs-reference comparison. Both fast paths skip
/// Anchor's dispatch + account deserialization, so each must come in
/// cheaper — asserted so a regression that erodes the saving fails the test.
/// The profile row is also the `sol_memcpy_`-versus-chunked measurement: the
/// syscall is metered at `max(10, len / 250)` CU, against ~40 for the 20
/// 8-byte load/store pairs a hand-rolled 160-byte copy would need. Run with
/// `--nocapture` (or read the make-test-parity log) to see the table.
#[test]
fn cu_report() {
    if !ref_built() {
        eprintln!("skipping cu_report: reference oracle absent (run `make program-parity`)");
        return;
    }
    for (label, reference, asm) in [
        (
            "set_reference_price",
            stamp_cu(Fixture::bootstrap_ref()),
            stamp_cu(Fixture::bootstrap()),
        ),
        (
            "set_liquidity_profile",
            profile_cu(Fixture::bootstrap_ref()),
            profile_cu(Fixture::bootstrap()),
        ),
    ] {
        let saved = reference.saturating_sub(asm);
        eprintln!("{label} compute units");
        eprintln!("  reference (Rust entrypoint): {reference}");
        eprintln!("  asm fast path:               {asm}");
        eprintln!("  saved:                       {saved}");
        assert!(
            asm < reference,
            "{label}: asm fast path ({asm} CU) should undercut the reference ({reference} CU)"
        );
    }
}
