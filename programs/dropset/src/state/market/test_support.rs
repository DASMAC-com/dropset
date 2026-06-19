//! Shared test fixtures for the slab-backed `market` submodule tests.
//! Builds a stack-allocated [`AccountBuffer`] and loads it as a **real**
//! [`Market`] with a fixed number of zeroed sectors and empty list heads,
//! so the `dll` and `access` test modules drive the on-chain code path
//! directly rather than a mock. Compiled only under `cfg(test)`.

use anchor_lang_v2::{testing::AccountBuffer, AnchorAccount, Discriminator};

use super::{Market, MarketHeader, NULL_SECTOR};

/// Number of sectors pre-allocated in the test fixture. Bigger
/// than any single test needs so the free-list interleaving
/// scenarios all have headroom.
pub(crate) const SECTORS: u32 = 4;

/// Total buffer bytes: `RuntimeAccount` header (96) + 8-byte
/// discriminator + `MarketHeader` + slab `len` field + `SECTORS`
/// × `size_of::<Vault>()`. Rounded up to a constant that fits the
/// largest test and stays on the stack comfortably.
pub(crate) const BUF_BYTES: usize = 4096;

/// Total data-region bytes (everything after `RuntimeAccount`):
/// `discriminator + MarketHeader + len_field + SECTORS * Vault`.
/// `Market::space_for(SECTORS)` already accounts for the slab
/// header / len / alignment padding; add the 8-byte
/// `#[account]` discriminator on top.
pub(crate) const DATA_LEN: usize = 8 + Market::space_for(SECTORS);

/// Build a fresh buffer with [`SECTORS`] zeroed sectors, empty
/// list heads, and the correct discriminator. Returns the buffer
/// (kept on the stack — caller holds it for the test lifetime).
pub(crate) fn setup() -> AccountBuffer<BUF_BYTES> {
    let buf = AccountBuffer::<BUF_BYTES>::new();
    buf.init(
        [0xAA; 32],
        crate::ID.to_bytes(),
        DATA_LEN,
        /* is_signer */ false,
        /* is_writable */ true,
        /* executable */ false,
    );
    // Write the discriminator + slab `len = SECTORS`. The rest of
    // the data region is already zero from `AccountBuffer::new`,
    // which doubles as zeroed `Vault`s.
    let mut data = vec![0u8; DATA_LEN];
    data[..8].copy_from_slice(<MarketHeader as Discriminator>::DISCRIMINATOR);
    // len field lives right after the header at LEN_OFFSET, which
    // is `disc + size_of::<MarketHeader>()` for Anchor accounts.
    let len_off = 8 + core::mem::size_of::<MarketHeader>();
    data[len_off..len_off + 4].copy_from_slice(&SECTORS.to_le_bytes());
    buf.write_data(&data);
    buf
}

/// Load the buffer as a mutable `Market` and initialize the list
/// heads + every sector's `next`/`prev` to `NULL_SECTOR`. Tests
/// then build whatever list state they need on top.
pub(crate) fn load_market(buf: &AccountBuffer<BUF_BYTES>) -> Market {
    let view = unsafe { buf.view() };
    // SAFETY: this is the only live wrapper over `view`'s data
    // for the test's duration.
    let mut market = unsafe { Market::load_mut(view).unwrap() };
    market.head = NULL_SECTOR.into();
    market.tombstone_head = NULL_SECTOR.into();
    market.free_head = NULL_SECTOR.into();
    for i in 0..SECTORS as usize {
        market.as_mut_slice()[i].next = NULL_SECTOR.into();
        market.as_mut_slice()[i].prev = NULL_SECTOR.into();
    }
    market
}
