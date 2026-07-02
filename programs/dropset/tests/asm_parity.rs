//! Rust↔ASM parity for the `set_reference_price` fast path, plus the
//! offset assertions that pin what `src/asm/entrypoint.s` hardcodes.
//!
//! Two layers:
//!
//! 1. [`asm_offsets_match_layout`] re-derives every offset the assembly
//!    hardcodes — from the real `#[repr(C)]` / `#[account]` layout and
//!    agave's aligned account serialization — and asserts it against the
//!    literal the `.s` uses. A `layout.rs` reorder / width change (or a
//!    miscomputed ABI offset) breaks this test rather than silently
//!    mis-stamping on-chain. Always runs.
//!
//! 2. The `*_parity` tests deploy the `asm-entrypoint` artifact
//!    (`make program-asm`) beside the default reference build and push
//!    identical inputs through both, asserting the assembly and the Rust
//!    kernel produce the same stamp and the same domain error codes. They
//!    **skip** (rather than fail) when the asm `.so` is absent, so a plain
//!    `cargo test` — which only builds the reference `.so` — stays green.
//!
//! Only *legitimate* inputs are compared for byte-parity: a real wallet
//! signer carries no data, so the assembly's signer-empty / writable
//! layout guards never fire on inputs the reference build would accept.
//! Those structural guards are asm-only (they keep the market at a static
//! input offset) and are intentionally not part of the parity contract —
//! see the architecture spec's **SetReferencePrice**.

mod common;

use anchor_v2_testing::Signer;
use common::fixture::Fixture;
use core::mem::{offset_of, size_of};
use dropset::{Market, MarketHeader, Price, ReferencePrice, Vault};
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

    // account 1 (market) base, with the signer carrying zero data.
    let market_base =
        NUM_ACCOUNTS_SIZE + ACCT_HEADER_SIZE + 0 + MAX_PERMITTED_DATA_INCREASE + RENT_EPOCH_SIZE;
    assert_eq!(market_base, 10344, "MARKET_BASE");
    assert_eq!(market_base + HDR_IS_WRITABLE, 10346, "MARKET_IS_WRITABLE_OFF");
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
        10677,
        "MARKET_LEN_OFF"
    );
    // `Market::space_for(0)` IS the slab's ITEMS_OFFSET (align_up over the
    // len field to align_of::<Vault>() == 4), so this pins the 3-byte pad.
    assert_eq!(Market::space_for(0), 252, "SLAB_ITEMS_OFF");
    assert_eq!(size_of::<Vault>(), 560, "VAULT_SIZE");

    // Vault field offsets the stamp writes to.
    assert_eq!(offset_of!(Vault, quote_authority), 40, "VAULT_QUOTE_AUTHORITY_OFF");
    let rp = offset_of!(Vault, reference_price);
    assert_eq!(rp + offset_of!(ReferencePrice, stamp), 72, "RP_STAMP_OFF");
    assert_eq!(rp + offset_of!(ReferencePrice, price), 80, "RP_PRICE_OFF");
    assert_eq!(rp + offset_of!(ReferencePrice, quote_slot), 84, "RP_QUOTE_SLOT_OFF");
}

/// The stamped reference price plus the post-stamp market nonce — the
/// observable state the assembly and the kernel must agree on.
type Stamp = (u64, u32, u32, u64);

fn valid_price() -> u32 {
    Price::encode(10_850_000, 0).unwrap().as_u32()
}

fn asm_built() -> bool {
    std::path::Path::new(common::ASM_PROGRAM_SO_PATH).exists()
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
    if !asm_built() {
        eprintln!("skipping happy_path_parity: asm .so absent (run `make program-asm`)");
        return;
    }
    // Identical bootstrap + op sequence on each build, so the pre-stamp
    // nonce matches; the resulting stamp is then byte-identical only if the
    // assembly writes the same bytes the kernel does.
    let reference = stamp_and_read(Fixture::bootstrap(), valid_price(), 7);
    let asm = stamp_and_read(Fixture::bootstrap_asm(), valid_price(), 7);
    assert_eq!(reference, asm, "asm stamp must byte-match the reference build");
    // And the stamp is what we expect: flush armed over a zero pre-nonce,
    // price + slot stored raw, nonce bumped to 1.
    assert_eq!(asm.0, dropset::FLUSH_BIT, "stamp = pre_nonce(0) | FLUSH_BIT");
    assert_eq!(asm.1, valid_price());
    assert_eq!(asm.2, 7);
    assert_eq!(asm.3, 1, "nonce bumped");
}

#[test]
fn invalid_price_stored_raw_parity() {
    if !asm_built() {
        eprintln!("skipping invalid_price_stored_raw_parity: asm .so absent");
        return;
    }
    // The write validates neither the price nor the slot; both builds store
    // an invalid significand and a far-future slot verbatim.
    let bits = 5_000_000;
    let reference = stamp_and_read(Fixture::bootstrap(), bits, 1_000_000);
    let asm = stamp_and_read(Fixture::bootstrap_asm(), bits, 1_000_000);
    assert_eq!(reference, asm);
    assert_eq!(asm.1, bits);
    assert_eq!(asm.2, 1_000_000);
}

#[test]
fn unauthorized_parity() {
    if !asm_built() {
        eprintln!("skipping unauthorized_parity: asm .so absent");
        return;
    }
    let ref_err = unauthorized_err(Fixture::bootstrap());
    let asm_err = unauthorized_err(Fixture::bootstrap_asm());
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
    if !asm_built() {
        eprintln!("skipping out_of_range_sector_parity: asm .so absent");
        return;
    }
    let ref_err = oob_err(Fixture::bootstrap());
    let asm_err = oob_err(Fixture::bootstrap_asm());
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

/// The CU report the issue asks for: measure a `set_reference_price` stamp
/// on each build and print the assembly-vs-reference comparison. The
/// fast path skips Anchor's dispatch + account deserialization, so it must
/// come in cheaper — asserted so a regression that erodes the saving fails
/// the test. Run with `--nocapture` (or read the make-test-parity log) to
/// see the table.
#[test]
fn cu_report() {
    if !asm_built() {
        eprintln!("skipping cu_report: asm .so absent (run `make program-asm`)");
        return;
    }
    let reference = stamp_cu(Fixture::bootstrap());
    let asm = stamp_cu(Fixture::bootstrap_asm());
    let saved = reference.saturating_sub(asm);
    eprintln!("set_reference_price compute units");
    eprintln!("  reference (Rust entrypoint): {reference}");
    eprintln!("  asm fast path:               {asm}");
    eprintln!("  saved:                       {saved}");
    assert!(
        asm < reference,
        "asm fast path ({asm} CU) should undercut the reference ({reference} CU)"
    );
}
