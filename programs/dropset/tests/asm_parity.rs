// cspell:word nocapture
//! Rust↔ASM parity for the two quote-write fast paths —
//! `set_reference_price` and `set_liquidity_profile` — plus the offset
//! assertions that pin what `src/asm/entrypoint.s` hardcodes.
//!
//! Two layers:
//!
//! 1. [`asm_offsets_match_wire_format`] pins the assembly's instruction-data
//!    offsets and discriminators against what the generated client actually
//!    serializes. Always runs.
//!
//!    Its *layout* counterpart is no longer a test at all: every
//!    `MARKET_*` / `VAULT_*` / `RP_*` offset the assembly hardcodes is now
//!    checked at **compile time** in `src/asm_offsets.rs`, against the
//!    `.equ` table `build.rs` lifts out of `src/asm/entrypoint.s`. That is
//!    strictly stronger than asserting it here was — it reads the
//!    assembly, so a `layout.rs` reorder or width change breaks the
//!    **build** and cannot be papered over by editing a literal on this
//!    side. The offsets below come from that same lifted table.
//!
//! 2. The `*_parity` tests deploy the reference (feature-off) build beside
//!    the default asm `.so` and push identical inputs through both,
//!    asserting the assembly and the Rust kernels produce the same market
//!    bytes and the same domain error codes. They **skip** (rather than
//!    fail) when the reference oracle `.so` is absent, so a plain
//!    `cargo test` — which only builds the default asm `.so` — stays green.
//!    `make program-parity` builds both.
//!
//!    That skip is convenience for a bare local run and **must never be
//!    what CI does**, because a skipped test is scored as a passing one:
//!    every parity assertion in this file could evaporate into a green
//!    required job with nothing compared. So the skip is gated on
//!    [`REQUIRE_ORACLE_ENV`] — any run that sets it turns a missing oracle
//!    into a hard failure, and both `make test-parity` and the CI job do.
//!    See [`ref_built`].
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
use dropset::asm_offsets::equ;
use dropset::{Price, ReferencePrice, Vault};
use solana_pubkey::Pubkey;

/// Narrow an assembly offset for indexing. The lifted `.equ` table is `u64`
/// — the assembly's word — and every offset in it is tiny.
fn wire_off(offset: u64) -> usize {
    usize::try_from(offset).expect("assembly offset fits a usize")
}

#[test]
fn asm_offsets_match_wire_format() {
    // `set_liquidity_profile` (disc 6): the assembly reads `vault_idx` at
    // IX_VAULT_IDX_OFF and hands `sol_memcpy_` a source pointer of
    // IX_PROFILE_OFF. Pin both against the *real* serialization rather than
    // re-deriving them arithmetically — encode a recognizable payload and
    // locate it in the wire bytes.
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
    assert_eq!(
        u64::from(wire[0]),
        equ::DISCRIM_SET_LIQUIDITY_PROFILE,
        "discriminator"
    );
    let idx = wire_off(equ::IX_VAULT_IDX_OFF);
    assert_eq!(
        &wire[idx..idx + 4],
        &0x0403_0201u32.to_le_bytes(),
        "IX_VAULT_IDX_OFF"
    );
    assert_eq!(
        &wire[wire_off(equ::IX_PROFILE_OFF)..],
        &probe,
        "IX_PROFILE_OFF"
    );
    assert_eq!(
        PROFILE_BYTES as u64,
        equ::PROFILE_SIZE,
        "the fixture's blob width must be the assembly's copy length"
    );

    // Same treatment for disc 5, whose three u32 payload fields the
    // assembly reads at fixed offsets. `quote_unix` is last and therefore
    // the one a stale SDK builder would silently omit, so pin its position
    // against the real serialization too.
    let wire = dropset::instruction::SetReferencePrice {
        vault_idx: 0x0403_0201,
        price_bits: 0x0807_0605,
        quote_slot: 0x0C0B_0A09,
        quote_unix: 0x100F_0E0D,
    }
    .data();
    assert_eq!(wire.len(), 1 + 4 * size_of::<u32>(), "ix data len");
    assert_eq!(
        u64::from(wire[0]),
        equ::DISCRIM_SET_REFERENCE_PRICE,
        "discriminator"
    );
    for (offset, expected, label) in [
        (equ::IX_VAULT_IDX_OFF, 0x0403_0201u32, "IX_VAULT_IDX_OFF"),
        (equ::IX_PRICE_BITS_OFF, 0x0807_0605, "IX_PRICE_BITS_OFF"),
        (equ::IX_QUOTE_SLOT_OFF, 0x0C0B_0A09, "IX_QUOTE_SLOT_OFF"),
        (equ::IX_QUOTE_UNIX_OFF, 0x100F_0E0D, "IX_QUOTE_UNIX_OFF"),
    ] {
        let off = wire_off(offset);
        assert_eq!(&wire[off..off + 4], &expected.to_le_bytes(), "{label}");
    }

    // ── The fused-copy contract, wire side ───────────────────────────
    //
    // The disc-5 payload moves the two clock datums as a single
    // `ldxdw`/`stxdw` pair rather than two word copies. That is legal only
    // while the pair is adjacent *and in the same order* on both sides of
    // the copy — the instruction data and the vault record.
    //
    // The vault side is settled at compile time: `layout.rs` const-asserts
    // the field adjacency and `src/asm_offsets.rs` ties it to the offsets
    // the assembly actually stores through, including the bound past which
    // `base_atoms` begins. What no const can see is the *wire* side's byte
    // order, which is what this asserts.
    let pair = wire_off(equ::IX_QUOTE_SLOT_OFF);
    assert_eq!(
        &wire[pair..pair + 8],
        &0x100F_0E0D_0C0B_0A09u64.to_le_bytes(),
        "ix-side datum pair must read as one little-endian u64 \
         (quote_slot low, quote_unix high) for the fused ldxdw"
    );
}

/// The stamped reference price (`stamp`, `price`, `quote_slot`,
/// `quote_unix`) plus the post-stamp market nonce — the observable state
/// the assembly and the kernel must agree on.
type Stamp = (u64, u32, u32, u32, u64);

fn valid_price() -> u32 {
    Price::encode(10_850_000, 0).unwrap().as_u32()
}

/// Set by any run that requires the reference oracle to be present —
/// `make test-parity` and the `Tests (asm parity)` CI job both export it.
///
/// It exists because nextest and the default harness alike score an early
/// `return` as **passed**: without this gate, a build or cache mishap that
/// left `dropset_ref.so` missing would take all eleven comparison tests
/// below with it and still report a green required check. The value is
/// never read, only its presence.
const REQUIRE_ORACLE_ENV: &str = "DROPSET_REQUIRE_PARITY_ORACLE";

/// Whether the reference oracle `.so` is on disk — the guard every
/// comparison test below opens with.
///
/// # Panics
///
/// If the oracle is absent while [`REQUIRE_ORACLE_ENV`] is set. That is the
/// whole point: a bare `cargo test --test asm_parity` skips (the oracle is
/// genuinely not built), while CI fails loudly rather than silently
/// asserting nothing.
fn ref_built() -> bool {
    if std::path::Path::new(common::REF_PROGRAM_SO_PATH).exists() {
        return true;
    }
    assert!(
        std::env::var_os(REQUIRE_ORACLE_ENV).is_none(),
        "reference oracle missing at {} while {REQUIRE_ORACLE_ENV} is set.\n\
         This run requires the Rust↔ASM comparison to actually happen, so a \
         skip is a failure: build the pair with `make program-parity` (or \
         investigate why the cached artifact pair is incomplete). Unset \
         {REQUIRE_ORACLE_ENV} only for a deliberate asm-only local run.",
        common::REF_PROGRAM_SO_PATH,
    );
    false
}

/// Open vault 0, stamp `(price_bits, quote_slot, quote_unix)`, and read
/// back `(stamp, price, quote_slot, quote_unix, nonce)`.
///
/// The datum is passed explicitly rather than taken from each bank's own
/// clock: the two builds run on separate `LiteSVM` instances, and a
/// parity comparison must not depend on their clocks agreeing.
fn stamp_and_read(mut f: Fixture, price_bits: u32, quote_slot: u32, quote_unix: u32) -> Stamp {
    let auth = f.authority.pubkey();
    f.create_vault(0, auth, false, Pubkey::default())
        .expect("create_vault");
    let signer = f.authority.insecure_clone();
    f.set_reference_price_at(&signer, 0, price_bits, quote_slot, quote_unix)
        .expect("set_reference_price");
    let v = f.vault(0);
    (
        v.reference_price.stamp.get(),
        v.reference_price.price.as_u32(),
        v.reference_price.quote_slot.get(),
        v.reference_price.quote_unix.get(),
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
    let reference = stamp_and_read(Fixture::bootstrap_ref(), valid_price(), 7, 1_700_000_000);
    let asm = stamp_and_read(Fixture::bootstrap(), valid_price(), 7, 1_700_000_000);
    assert_eq!(
        reference, asm,
        "asm stamp must byte-match the reference build"
    );
    // And the stamp is what we expect: flush armed over a zero pre-nonce,
    // price + slot + wall-clock datum stored raw, nonce bumped to 1.
    assert_eq!(
        asm.0,
        dropset::FLUSH_BIT,
        "stamp = pre_nonce(0) | FLUSH_BIT"
    );
    assert_eq!(asm.1, valid_price());
    assert_eq!(asm.2, 7);
    assert_eq!(asm.3, 1_700_000_000, "quote_unix stored raw");
    assert_eq!(asm.4, 1, "nonce bumped");
}

#[test]
fn invalid_price_stored_raw_parity() {
    if !ref_built() {
        eprintln!("skipping invalid_price_stored_raw_parity: reference oracle absent");
        return;
    }
    // The write validates none of the price, the slot, or the wall-clock
    // datum; both builds store an invalid significand, a far-future slot,
    // and a zero datum verbatim. A zero datum is the fail-closed case —
    // it is stored, not rejected, and kills the vault at match time.
    let bits = 5_000_000;
    let reference = stamp_and_read(Fixture::bootstrap_ref(), bits, 1_000_000, 0);
    let asm = stamp_and_read(Fixture::bootstrap(), bits, 1_000_000, 0);
    assert_eq!(reference, asm);
    assert_eq!(asm.1, bits);
    assert_eq!(asm.2, 1_000_000);
    assert_eq!(asm.3, 0);
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

/// A recognizable non-zero `base_atoms` for the target sector. Every byte is
/// distinct and non-zero, so a store that overruns into *any* part of it
/// shows up in the diff.
const SEEDED_BASE_ATOMS: u64 = 0x1234_5678_9ABC_DEF0;

/// The neighboring sector's `base_atoms`, likewise recognizable and
/// distinct from [`SEEDED_BASE_ATOMS`], so a store that lands one sector low
/// overwrites bytes that were neither zero nor the target's.
const NEIGHBOR_BASE_ATOMS: u64 = 0x0BAD_0BAD_0BAD_0BAD;

/// The target sector's stamp, and the neighboring sector's. Explicit for
/// the same reason as [`PROFILE_QUOTE_UNIX`] — the two builds' clocks cannot
/// be compared — and **pairwise distinct in every field**, which is what
/// makes a mis-targeted store visible: a store landing one sector low, or
/// reading the wrong payload word, changes a byte rather than rewriting the
/// value already there.
const STAMP_TARGET: (u32, u32) = (7, 1_700_000_000);
/// See [`STAMP_TARGET`]. Distinct slot, distinct datum, distinct price.
const STAMP_NEIGHBOR: (u32, u32) = (3, 1_600_000_000);

/// A valid price distinct from [`valid_price`], for the neighboring sector.
///
/// Without this the two sectors shared a price word, and a mis-targeted
/// store that wrote only `price` one sector low would write the value
/// already there — invisible to a byte diff, which can only see a change.
///
/// The significand must carry exactly 8 digits (`Price` normalizes to
/// `[10_000_000, 99_999_999]`), so this is not a free choice of any
/// smaller-looking number.
fn neighbor_price() -> u32 {
    Price::encode(12_500_000, 0).unwrap().as_u32()
}

/// Open two vaults, seed the neighboring sector's reference price and both
/// sectors' `base_atoms`, then stamp a price onto the *second* sector.
///
/// Returns every `(index, new_value)` the stamp changed in the market
/// account's data, plus the target sector's `base_atoms` afterwards.
fn stamp_write_footprint(mut f: Fixture) -> (Vec<(usize, u8)>, u64) {
    let auth = f.authority.pubkey();
    f.create_vault(0, auth, false, Pubkey::default())
        .expect("create_vault 0");
    f.create_vault(1, auth, false, Pubkey::default())
        .expect("create_vault 1");
    let signer = f.authority.insecure_clone();
    // Seed sector 0's reference price, so a store that lands one sector low
    // overwrites recognizable bytes rather than zeros. Every field differs
    // from the target write below, price included.
    f.set_reference_price_at(
        &signer,
        0,
        neighbor_price(),
        STAMP_NEIGHBOR.0,
        STAMP_NEIGHBOR.1,
    )
    .expect("seed vault 0's reference price");
    // Both sectors' `base_atoms` non-zero — see `poke_base_atoms`. This is
    // the field the fused store's upper bound abuts, and a fresh vault
    // leaves it at zero, where a clobber-to-zero would be invisible.
    f.poke_base_atoms(0, NEIGHBOR_BASE_ATOMS);
    f.poke_base_atoms(1, SEEDED_BASE_ATOMS);

    let before = f.market_data();
    f.set_reference_price_at(&signer, 1, valid_price(), STAMP_TARGET.0, STAMP_TARGET.1)
        .expect("set_reference_price");
    let after = f.market_data();

    let changed = before
        .iter()
        .zip(after.iter())
        .enumerate()
        .filter(|(_, (b, a))| b != a)
        .map(|(i, (_, &a))| (i, a))
        .collect();
    (changed, f.vault(1).base_atoms.get())
}

#[test]
fn stamp_write_footprint_parity() {
    if !ref_built() {
        eprintln!("skipping stamp_write_footprint_parity: reference oracle absent");
        return;
    }
    // The disc-6 path has had a whole-account footprint diff since it
    // landed; this is the disc-5 counterpart. It exists because the
    // assembly's own guidance invites further fusion here ("minimize total
    // copies — fuse adjacent u32s into one u64 move wherever layout
    // allows"), and the scalar readback assertions above only pin the
    // four fields this path is *supposed* to write. Whether it wrote
    // anything else is a question only a whole-account diff can answer.
    let reference = stamp_write_footprint(Fixture::bootstrap_ref());
    let asm = stamp_write_footprint(Fixture::bootstrap());
    // Identity-free like the profile footprint — the changed bytes are the
    // nonce, the stamp and the payload u32s, no pubkeys — so the two
    // fixtures' independent keypairs don't make the builds incomparable.
    assert_eq!(
        reference, asm,
        "asm and reference must move the same bytes to the same values"
    );

    // Vault 1 of two is the target; everything outside these three ranges
    // must be untouched.
    let vault1 = common::fixture::vault_byte_offset(1);
    let nonce = 8..8 + size_of::<u64>();
    let rp = vault1 + offset_of!(Vault, reference_price);
    let stamp = rp + offset_of!(ReferencePrice, stamp);
    let stamp = stamp..stamp + size_of::<u64>();
    // `price`, `quote_slot` and `quote_unix` are three adjacent u32s — the
    // latter two written as a single fused double-word — so the payload is
    // one contiguous span, and it ends exactly where `base_atoms` begins.
    // Pinning that adjacency here is what makes the range check below a
    // statement about the fused store's upper bound rather than arithmetic.
    let payload = rp + offset_of!(ReferencePrice, price);
    let payload = payload..payload + 3 * size_of::<u32>();
    assert_eq!(
        payload.end,
        vault1 + offset_of!(Vault, base_atoms),
        "the payload span must end exactly at base_atoms"
    );
    for (idx, _) in &asm.0 {
        assert!(
            nonce.contains(idx) || stamp.contains(idx) || payload.contains(idx),
            "byte {idx} changed outside nonce / stamp / reference-price \
             payload of sector 1"
        );
    }
    // And the field immediately past the fused store is intact — asserted
    // directly rather than inferred from the changed set, which can only
    // ever prove a subset (a write of the value already there is not a
    // change). This is the assertion a widened or overlapping store fails.
    assert_eq!(asm.1, SEEDED_BASE_ATOMS, "base_atoms untouched");
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

/// The wall-clock datum the profile-write cases stamp before writing.
///
/// Explicit rather than taken from the bank clock, for the same reason
/// [`stamp_and_read`] takes one: the two builds run on separate `LiteSVM`
/// instances, so anything derived from their clocks cannot be compared.
/// Distinguished against a single-/double-digit `quote_slot` per the
/// fixture's convention, so a slot/wall transposition is visible in any
/// assertion that reads either.
const PROFILE_QUOTE_UNIX: u32 = 1_700_000_011;

/// The observable state of a profile write: the stored blob, the reference
/// price fields it must leave untouched (stamp aside, which it re-arms), and
/// the post-write market nonce.
///
/// `quote_unix` is here deliberately. It is the wall half of the dual-domain
/// expiry gate, and omitting it left one specific corruption invisible: the
/// profile write clobbering it *to zero*. A footprint diff cannot see that
/// (a byte that was already zero never shows up as changed), so the only
/// thing that can is reading the field back and comparing it.
type ProfileWrite = (Vec<u8>, u64, u32, u32, u32, u64);

/// Open vault 0, stamp a price, then write `profile` and read back
/// everything the two builds must agree on.
fn write_profile_and_read(mut f: Fixture, profile: [u8; PROFILE_BYTES]) -> ProfileWrite {
    let auth = f.authority.pubkey();
    f.create_vault(0, auth, false, Pubkey::default())
        .expect("create_vault");
    let signer = f.authority.insecure_clone();
    f.set_reference_price_at(&signer, 0, valid_price(), 11, PROFILE_QUOTE_UNIX)
        .expect("set_reference_price");
    f.set_liquidity_profile(&signer, 0, profile)
        .expect("set_liquidity_profile");
    let v = f.vault(0);
    (
        anchor_lang_v2::bytemuck::bytes_of(&v.profile).to_vec(),
        v.reference_price.stamp.get(),
        v.reference_price.price.as_u32(),
        v.reference_price.quote_slot.get(),
        v.reference_price.quote_unix.get(),
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
    assert_eq!(asm.4, PROFILE_QUOTE_UNIX, "quote_unix untouched");
    assert_eq!(asm.5, 2, "nonce bumped by the profile write");
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
    // Bound the write's blast radius on each build, then compare. The
    // `PROFILE_BYTES`-wide `sol_memcpy_` is the one payload here big enough
    // to run off its field, so the assertion is which bytes of the *whole*
    // market account moved:
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
    assert_eq!(asm.2, ladder, "the whole profile blob is in place");
}

/// Open two vaults, stamp the second's reference price, write `profile` onto
/// that second sector, and return every `(index, new_value)` the write
/// changed in the market account's data, the post-write nonce, and the
/// target sector's stored profile region.
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
    // Stamp the TARGET sector's reference price non-zero before the
    // before-image is taken. This is what makes the "leaves the reference
    // price alone" half of this test real: a footprint diff can only ever
    // see a change *away* from the stored value, so with these fields left
    // at zero a profile write that clobbered one of them **to zero** would
    // change no byte and pass silently. `quote_unix` is the case that
    // matters most — it is the wall half of the dual-domain expiry gate,
    // and zeroing it is precisely the corruption a diff cannot otherwise
    // see.
    f.set_reference_price_at(&signer, 1, valid_price(), 11, PROFILE_QUOTE_UNIX)
        .expect("stamp the target sector's reference price");

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
    let now = f.now_unix().get();
    f.set_reference_price_meta(&signer, 0, valid_price(), 0, now)
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
/// syscall is metered at `max(10, len / 250)` CU — 10 CU at `PROFILE_BYTES`
/// (224) — against ~56 for the 28 8-byte load/store pairs a hand-rolled copy
/// of that width would need. Run with `--nocapture` (or read the
/// make-test-parity log) to see the table.
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
