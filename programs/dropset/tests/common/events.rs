//! Decoders for the structured events the program emits via
//! `emit_cpi!`.
//!
//! anchor v2's `emit_cpi!` records each event as a self-CPI to the
//! program (authority = the `__event_authority` PDA), so the event data
//! lands in the transaction's *inner* instructions rather than the logs.
//! Each such inner-instruction `data` is
//!
//! ```text
//! EVENT_IX_TAG_LE (8)  ++  DISCRIMINATOR (8)  ++  body
//! ```
//!
//! where the body is the borsh wire form (wincode `BORSH_CONFIG`) for a
//! plain `#[event]` and the raw `repr(C)` bytes for `#[event(bytemuck)]`
//! (`FillEvent`). These helpers pull the tag-stripped payloads off a
//! [`TransactionMetadata`] and decode the fields under assertion. The
//! field order / encoding mirrors `events.rs` and the borsh wire format
//! (little-endian primitives, 1-byte `bool`, 32-byte `Address`) — so
//! these decoders double as a pin on that wire format.

#![allow(dead_code)]

use anchor_lang_v2::{bytemuck, event::EVENT_IX_TAG_LE, Discriminator};
use dropset::{
    CloseVaultEvent, DepositEvent, FillEvent, FreezeVaultEvent, OpenVaultEvent, RealizeEvent,
    WithdrawEvent,
};
use litesvm::types::TransactionMetadata;

/// Every event-CPI payload (`[disc(8)][body]`, tag stripped) emitted by
/// the transaction, in emission order.
pub fn event_payloads(meta: &TransactionMetadata) -> Vec<Vec<u8>> {
    meta.inner_instructions
        .iter()
        .flatten()
        .map(|ii| ii.instruction.data.as_slice())
        .filter(|data| data.starts_with(EVENT_IX_TAG_LE))
        .map(|data| data[EVENT_IX_TAG_LE.len()..].to_vec())
        .collect()
}

/// Tag-stripped bodies (discriminator stripped too) of every emitted
/// event whose discriminator matches `T`.
fn bodies_of<T: Discriminator>(meta: &TransactionMetadata) -> Vec<Vec<u8>> {
    let disc = T::DISCRIMINATOR;
    event_payloads(meta)
        .into_iter()
        .filter(|p| p.starts_with(disc))
        .map(|p| p[disc.len()..].to_vec())
        .collect()
}

/// The single emitted event of type `T`, or a panic if there isn't
/// exactly one.
fn one_body<T: Discriminator>(meta: &TransactionMetadata) -> Vec<u8> {
    let mut bodies = bodies_of::<T>(meta);
    assert_eq!(
        bodies.len(),
        1,
        "expected exactly one {} event, found {}",
        core::any::type_name::<T>(),
        bodies.len()
    );
    bodies.pop().unwrap()
}

/// How many events of type `T` the transaction emitted.
pub fn count<T: Discriminator>(meta: &TransactionMetadata) -> usize {
    bodies_of::<T>(meta).len()
}

// ── borsh cursor ───────────────────────────────────────────────────────

struct Cursor<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }
    fn take(&mut self, n: usize) -> &'a [u8] {
        let s = &self.buf[self.pos..self.pos + n];
        self.pos += n;
        s
    }
    fn pubkey(&mut self) -> [u8; 32] {
        self.take(32).try_into().unwrap()
    }
    fn u32(&mut self) -> u32 {
        u32::from_le_bytes(self.take(4).try_into().unwrap())
    }
    fn u64(&mut self) -> u64 {
        u64::from_le_bytes(self.take(8).try_into().unwrap())
    }
    fn i64(&mut self) -> i64 {
        i64::from_le_bytes(self.take(8).try_into().unwrap())
    }
    fn bool(&mut self) -> bool {
        self.take(1)[0] != 0
    }
    /// Assert the whole body was consumed — catches a wire-format drift
    /// (an added / reordered / wrong-width field) that a field-by-field
    /// read would otherwise mask.
    fn finish(self) {
        assert_eq!(self.pos, self.buf.len(), "event body had trailing bytes");
    }
}

// ── decoded shapes ─────────────────────────────────────────────────────
// Addresses are kept as raw bytes so a test compares against
// `pubkey.to_bytes()` without depending on the on-chain `Address` type.

#[derive(Debug)]
pub struct OpenVault {
    pub market: [u8; 32],
    pub sector_idx: u32,
    pub leader: [u8; 32],
    pub quote_authority: [u8; 32],
    pub perf_fee_rate: u32,
    pub min_leader_share: u32,
    pub allow_outside_depositors: bool,
}

pub fn open_vault(meta: &TransactionMetadata) -> OpenVault {
    let body = one_body::<OpenVaultEvent>(meta);
    let mut c = Cursor::new(&body);
    let d = OpenVault {
        market: c.pubkey(),
        sector_idx: c.u32(),
        leader: c.pubkey(),
        quote_authority: c.pubkey(),
        perf_fee_rate: c.u32(),
        min_leader_share: c.u32(),
        allow_outside_depositors: c.bool(),
    };
    c.finish();
    d
}

#[derive(Debug)]
pub struct CloseVault {
    pub market: [u8; 32],
    pub sector_idx: u32,
    pub leader: [u8; 32],
    pub active_count_after: u32,
}

pub fn close_vault(meta: &TransactionMetadata) -> CloseVault {
    let body = one_body::<CloseVaultEvent>(meta);
    let mut c = Cursor::new(&body);
    let d = CloseVault {
        market: c.pubkey(),
        sector_idx: c.u32(),
        leader: c.pubkey(),
        active_count_after: c.u32(),
    };
    c.finish();
    d
}

#[derive(Debug)]
pub struct FreezeVault {
    pub market: [u8; 32],
    pub sector_idx: u32,
    pub leader: [u8; 32],
}

pub fn freeze_vault(meta: &TransactionMetadata) -> FreezeVault {
    let body = one_body::<FreezeVaultEvent>(meta);
    let mut c = Cursor::new(&body);
    let d = FreezeVault {
        market: c.pubkey(),
        sector_idx: c.u32(),
        leader: c.pubkey(),
    };
    c.finish();
    d
}

#[derive(Debug)]
pub struct Deposit {
    pub market: [u8; 32],
    pub sector_idx: u32,
    pub depositor: [u8; 32],
    pub is_leader: bool,
    pub is_seeding: bool,
    pub base_in: u64,
    pub quote_in: u64,
    pub shares_out: u64,
    pub total_shares_after: u64,
    pub leader_shares_after: u64,
    pub base_atoms_after: u64,
    pub quote_atoms_after: u64,
}

pub fn deposit(meta: &TransactionMetadata) -> Deposit {
    let body = one_body::<DepositEvent>(meta);
    let mut c = Cursor::new(&body);
    let d = Deposit {
        market: c.pubkey(),
        sector_idx: c.u32(),
        depositor: c.pubkey(),
        is_leader: c.bool(),
        is_seeding: c.bool(),
        base_in: c.u64(),
        quote_in: c.u64(),
        shares_out: c.u64(),
        total_shares_after: c.u64(),
        leader_shares_after: c.u64(),
        base_atoms_after: c.u64(),
        quote_atoms_after: c.u64(),
    };
    c.finish();
    d
}

#[derive(Debug)]
pub struct Withdraw {
    pub market: [u8; 32],
    pub sector_idx: u32,
    pub depositor: [u8; 32],
    pub is_leader: bool,
    pub shares_in: u64,
    pub base_out: u64,
    pub quote_out: u64,
    pub total_shares_after: u64,
    pub leader_shares_after: u64,
    pub base_atoms_after: u64,
    pub quote_atoms_after: u64,
    pub realized_pnl_delta: i64,
}

pub fn withdraw(meta: &TransactionMetadata) -> Withdraw {
    let body = one_body::<WithdrawEvent>(meta);
    let mut c = Cursor::new(&body);
    let d = Withdraw {
        market: c.pubkey(),
        sector_idx: c.u32(),
        depositor: c.pubkey(),
        is_leader: c.bool(),
        shares_in: c.u64(),
        base_out: c.u64(),
        quote_out: c.u64(),
        total_shares_after: c.u64(),
        leader_shares_after: c.u64(),
        base_atoms_after: c.u64(),
        quote_atoms_after: c.u64(),
        realized_pnl_delta: c.i64(),
    };
    c.finish();
    d
}

#[derive(Debug)]
pub struct Realize {
    pub market: [u8; 32],
    pub sector_idx: u32,
    pub shares_minted: u64,
    pub leader_shares_after: u64,
    pub total_shares_after: u64,
    pub hwm_after: u64,
}

pub fn realize(meta: &TransactionMetadata) -> Realize {
    let body = one_body::<RealizeEvent>(meta);
    let mut c = Cursor::new(&body);
    let d = Realize {
        market: c.pubkey(),
        sector_idx: c.u32(),
        shares_minted: c.u64(),
        leader_shares_after: c.u64(),
        total_shares_after: c.u64(),
        hwm_after: c.u64(),
    };
    c.finish();
    d
}

/// All `FillEvent`s emitted, in order. `FillEvent` is
/// `#[event(bytemuck)]`, so the body is the raw `repr(C)` struct and
/// decodes straight into the real type.
pub fn fills(meta: &TransactionMetadata) -> Vec<FillEvent> {
    bodies_of::<FillEvent>(meta)
        .iter()
        .map(|b| {
            assert_eq!(
                b.len(),
                core::mem::size_of::<FillEvent>(),
                "FillEvent body size mismatch"
            );
            bytemuck::pod_read_unaligned::<FillEvent>(b)
        })
        .collect()
}
