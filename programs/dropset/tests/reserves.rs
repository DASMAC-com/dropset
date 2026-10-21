//! Positive controls for the reserve-zero invariant that every fixture
//! send checks (`common::assert_reserves_zero`). That check classifies an
//! account by its discriminator and skips anything it does not recognize,
//! so a green suite alone cannot tell "every reserve is zero" from "no
//! account was ever classified". Each test here plants one non-zero
//! reserve byte and expects the next instruction touching the account to
//! trip the invariant.

mod common;

use anchor_v2_testing::Signer;
use common::fixture::{vault_depositor_pda, Fixture};
use dropset::{MarketHeader, Price, RegistryHeader, VaultDepositorHeader};

/// Overwrite the header byte at `reserved_off` on `key` with a non-zero
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

#[test]
#[should_panic(expected = "VaultDepositorHeader._reserved")]
fn a_non_zero_depositor_reserve_trips_the_invariant() {
    let mut f = Fixture::seeded(1_000_000, 1_085_000);
    let admin = f.authority.insecure_clone();
    f.set_outside_deposits_approved(&admin, 0, true).unwrap();
    f.set_allow_outside_depositors(&admin, 0, true).unwrap();
    let alice = f.funded_depositor(400_000, 400_000);
    f.deposit(&alice, 0, 100_000, 0, 400_000, 400_000)
        .expect("open the position");
    let (position, _) = vault_depositor_pda(&f.market, 0, &alice.pubkey());
    plant_reserve_byte(
        &mut f,
        position,
        core::mem::offset_of!(VaultDepositorHeader, _reserved),
    );
    // A top-off touches the same position account.
    f.svm.expire_blockhash();
    let _ = f.deposit(&alice, 0, 50_000, 0, 400_000, 400_000);
}
