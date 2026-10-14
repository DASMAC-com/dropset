//! Compile-time tie between the assembly's offset table and the Rust layout.
//!
//! `src/asm/entrypoint.s` stores through hardcoded byte offsets. Those
//! `.equ` literals are the **only** hand-written copy of the offsets the
//! assembly stores through — `layout.rs`'s own `size_of` assertions are a
//! separate wire-compatibility pin on the structs, not a second copy of
//! these:
//! `build.rs` lifts the whole symbol table into `$OUT_DIR/asm_equ.rs`
//! (re-exported as [`equ`]), and the assertions below compare each parsed
//! value against the offset derived from the real `#[repr(C)]` types. A
//! `layout.rs` reorder or width change that the assembly was not updated
//! for therefore fails the **build**.
//!
//! # Why parse the `.s` instead of restating its numbers
//!
//! Restating them is what this module replaces, and it did not work. The
//! assembly claimed its offsets were "pinned by the `offset_of!` assertion
//! test", but nothing read the assembly — the test hand-retyped the same
//! numbers, so the two were independent copies of one truth. Editing
//! `layout.rs` failed the test on its *literal*, and updating that literal
//! restored a green suite while leaving the assembly mis-targeted at the
//! old offset. Nothing in the loop ever consulted the `.s`.
//!
//! Parsing it closes that: the literal an author would have edited is now
//! read out of the assembly itself, so the only way to satisfy these
//! assertions is to change the assembly.
//!
//! # What is checked elsewhere, deliberately
//!
//! Two families of `.equ` symbols are pinned by *behavior* in
//! `tests/asm_parity.rs` and are not restated here, because a runtime check
//! against the real thing beats a second const:
//!
//! * **Error codes** (`E_UNAUTHORIZED`, `E_INVALID_SECTOR`) — the parity
//!   tests push a failing call through both builds and assert each surfaces
//!   the same `Custom(…)` code.
//! * **Instruction-data offsets** (`IX_*`) and the **discriminators** — the
//!   parity suite encodes a payload through the generated client and
//!   locates it in the wire bytes, which validates them against the real
//!   serialization rather than against another number. Only their
//!   *relationships* are asserted below, since those are what the fused
//!   load depends on.

use crate::{LiquidityProfile, Market, MarketHeader, ReferencePrice, Vault};
use core::mem::{offset_of, size_of};

/// The `.equ` symbol table, lifted verbatim out of `src/asm/entrypoint.s`
/// by `build.rs`. One `pub const` per directive, named as the assembly
/// names it.
pub mod equ {
    include!(concat!(env!("OUT_DIR"), "/asm_equ.rs"));
}

// ── agave aligned account serialization (the input-buffer ABI) ──────────
//
// A serialized account record is
//   [RuntimeAccount header(88) | data | MAX_PERMITTED_DATA_INCREASE(10240)
//    | pad-to-8 | rent_epoch(8)]
// preceded by an 8-byte account count. These are the runtime's ABI, not
// this crate's layout — a genuinely separate truth, which is why they are
// literals here rather than derived from anything.
//
// They are private: only the assertions below consume them. The parity
// suite used to carry its own copies, and no longer needs any — the
// offsets they were used to re-derive are checked here at compile time
// instead, so the suite imports only `equ`.

/// The 8-byte account count the input buffer opens with.
const NUM_ACCOUNTS_SIZE: u64 = 8;
/// Size of one serialized `RuntimeAccount` header.
const ACCT_HEADER_SIZE: u64 = 88;
/// Realloc headroom the runtime leaves after every account's data.
const MAX_PERMITTED_DATA_INCREASE: u64 = 10240;
/// The `rent_epoch` tail on each account record.
const RENT_EPOCH_SIZE: u64 = 8;
/// `is_signer`, within the account header.
const HDR_IS_SIGNER: u64 = 1;
/// `is_writable`, within the account header.
const HDR_IS_WRITABLE: u64 = 2;
/// `pubkey`, within the account header.
const HDR_PUBKEY: u64 = 8;
/// `data_len`, within the account header.
const HDR_DATA_LEN: u64 = 80;
/// Start of the account's data, within the account header.
const HDR_DATA: u64 = 88;
/// The anchor account discriminator that precedes `MarketHeader`.
///
/// `quote_write.rs` names this width too, for its own `usize` framing
/// arithmetic. The duplication is deliberate: both copies are private, the
/// value is anchor's fixed 8, and sharing one would mean widening
/// `market`'s private `mod quote_write` purely to export a constant.
const DISC_SIZE: u64 = 8;

// ── coverage of the lifted table ────────────────────────────────────────
//
// Pin how many symbols the assembly defines. Every assertion below names
// its symbol explicitly, so an offset that drifts is caught — but a symbol
// *added* to the assembly would be lifted, asserted by nothing, and
// silently reintroduce the unchecked-copy class this module exists to
// close. Failing here on a count change forces that decision to be made
// out loud: either add an assertion, or bump the count with a note saying
// why the new symbol needs none (the `E_*` and `DISCRIM_*` families are
// pinned by behavior instead — see the module header).
const _: () = assert!(equ::COUNT == 32);

// ── account 0: signer ───────────────────────────────────────────────────
const _: () = assert!(equ::SIGNER_IS_SIGNER_OFF == NUM_ACCOUNTS_SIZE + HDR_IS_SIGNER);
const _: () = assert!(equ::SIGNER_PUBKEY_OFF == NUM_ACCOUNTS_SIZE + HDR_PUBKEY);
const _: () = assert!(equ::SIGNER_DATA_LEN_OFF == NUM_ACCOUNTS_SIZE + HDR_DATA_LEN);

// ── account 1: market ───────────────────────────────────────────────────
//
// The signer is required to carry no data, so its data region contributes
// only the realloc pad and the market record sits at a static offset.
/// Byte offset of the market's account record within the input buffer.
const MARKET_BASE: u64 =
    NUM_ACCOUNTS_SIZE + ACCT_HEADER_SIZE + MAX_PERMITTED_DATA_INCREASE + RENT_EPOCH_SIZE;
const _: () = assert!(equ::MARKET_BASE == MARKET_BASE);
const _: () = assert!(equ::MARKET_IS_WRITABLE_OFF == MARKET_BASE + HDR_IS_WRITABLE);
const _: () = assert!(equ::MARKET_DATA_LEN_OFF == MARKET_BASE + HDR_DATA_LEN);
const _: () = assert!(equ::MARKET_DATA_OFF == MARKET_BASE + HDR_DATA);

// ── market data framing: [disc(8)][MarketHeader][len:u32][pad][vaults] ──
/// Byte offset of the market's *data* within the input buffer.
const MARKET_DATA_OFF: u64 = MARKET_BASE + HDR_DATA;
const _: () = assert!(
    equ::MARKET_NONCE_OFF == MARKET_DATA_OFF + DISC_SIZE + offset_of!(MarketHeader, nonce) as u64
);
const _: () =
    assert!(equ::MARKET_LEN_OFF == MARKET_DATA_OFF + DISC_SIZE + size_of::<MarketHeader>() as u64);
// `Market::space_for(0)` IS the slab's items offset (align_up over the len
// field to align_of::<Vault>()), so this pins the pad as well as the base.
const _: () = assert!(equ::SLAB_ITEMS_OFF == Market::space_for(0) as u64);
const _: () = assert!(equ::VAULT_SIZE == size_of::<Vault>() as u64);
const _: () = assert!(equ::PROFILE_SIZE == size_of::<LiquidityProfile>() as u64);

// ── Vault field offsets the two payloads write to ───────────────────────
const _: () = assert!(equ::VAULT_QUOTE_AUTHORITY_OFF == offset_of!(Vault, quote_authority) as u64);
/// `reference_price` within a `Vault`, the base for the four `RP_*` offsets.
const RP: u64 = offset_of!(Vault, reference_price) as u64;
const _: () = assert!(equ::RP_STAMP_OFF == RP + offset_of!(ReferencePrice, stamp) as u64);
const _: () = assert!(equ::RP_PRICE_OFF == RP + offset_of!(ReferencePrice, price) as u64);
const _: () = assert!(equ::RP_QUOTE_SLOT_OFF == RP + offset_of!(ReferencePrice, quote_slot) as u64);
const _: () = assert!(equ::RP_QUOTE_UNIX_OFF == RP + offset_of!(ReferencePrice, quote_unix) as u64);
const _: () = assert!(equ::VAULT_PROFILE_OFF == offset_of!(Vault, profile) as u64);

// ── the fused-copy contract ─────────────────────────────────────────────
//
// Disc 5 moves `quote_slot` and `quote_unix` as ONE double-word, which is
// legal only while the pair is adjacent and in that order on both sides of
// the copy. `layout.rs` const-asserts the vault-side adjacency; these pin
// what the assembly encodes, and the wire side is covered by the parity
// suite's serialization check.
const _: () = assert!(equ::RP_QUOTE_UNIX_OFF == equ::RP_QUOTE_SLOT_OFF + 4);
const _: () = assert!(equ::IX_QUOTE_UNIX_OFF == equ::IX_QUOTE_SLOT_OFF + 4);
// The store covers vault+RP_QUOTE_SLOT_OFF..+8, and `base_atoms` begins at
// exactly that upper bound — so "nothing bleeds" is stated here rather than
// left to arithmetic. Load-bearing for fund safety, not merely for
// correctness: a drift that put leader-supplied bytes into `base_atoms`
// would corrupt pooled inventory. See `tests/asm_parity.rs`'s
// `stamp_write_footprint_parity`, which proves the same bound at runtime.
const _: () = assert!(equ::RP_QUOTE_UNIX_OFF + 4 == offset_of!(Vault, base_atoms) as u64);

// ── payload framing ─────────────────────────────────────────────────────
const _: () = assert!(equ::IX_PRICE_BITS_OFF == equ::IX_VAULT_IDX_OFF + 4);
const _: () = assert!(equ::IX_QUOTE_SLOT_OFF == equ::IX_PRICE_BITS_OFF + 4);
const _: () = assert!(equ::IX_PROFILE_OFF == equ::IX_VAULT_IDX_OFF + 4);

const _: () = assert!(equ::FLUSH_BIT == crate::FLUSH_BIT);
