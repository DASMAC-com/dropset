//! Positive controls for the reserve-zero invariant that every fixture
//! send checks (`common::assert_reserves_zero`). That check classifies an
//! account by its discriminator and skips anything it does not recognize,
//! so a green suite alone cannot tell "every reserve is zero" from "no
//! account was ever classified". Each test here plants one non-zero
//! reserve byte and expects the next instruction touching the account to
//! trip the invariant.

mod common;

use common::fixture::Fixture;
use dropset::{MarketHeader, Price, RegistryHeader};

/// Overwrite the first `_reserved` byte of `H` on `key` with a non-zero
/// value, behind the 8-byte discriminator.
fn plant_reserve_byte(f: &mut Fixture, key: solana_pubkey::Pubkey, reserved_off: usize) {
    let mut acct = f.svm.get_account(&key).expect("account");
    acct.data[8 + reserved_off] = 1;
    f.svm.set_account(key, acct).expect("reinstall account");
}

#[test]
#[should_panic(expected = "MarketHeader._reserved")]
fn a_non_zero_market_reserve_trips_the_invariant() {
    let mut f = Fixture::seeded(1_000_000, 1_085_000);
    let market = f.market;
    plant_reserve_byte(
        &mut f,
        market,
        core::mem::offset_of!(MarketHeader, _reserved),
    );
    let auth = f.authority.insecure_clone();
    let px = Price::encode(10_850_000, 0).unwrap().as_u32();
    let _ = f.set_reference_price(&auth, 0, px, 2);
}

#[test]
#[should_panic(expected = "RegistryHeader._reserved")]
fn a_non_zero_registry_reserve_trips_the_invariant() {
    let mut f = Fixture::bootstrap();
    let registry = f.registry;
    plant_reserve_byte(
        &mut f,
        registry,
        core::mem::offset_of!(RegistryHeader, _reserved),
    );
    let auth = f.authority.insecure_clone();
    let _ = f.set_registry_defaults(&auth, Some(1), None, None);
}
