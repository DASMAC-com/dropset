//! `register_vault` integration tests — admin leader-override path,
//! non-admin fee path, the perf-fee bound, the cap-exceeded gate, the
//! quote-authority guard, and active-DLL linkage. All built on the
//! shared [`Fixture`].

mod common;

use anchor_v2_testing::{Keypair, Signer};
use common::fixture::Fixture;
use common::{REGISTER_MARKET_FEE_ATOMS, SIGNER_FUNDING_LAMPORTS};
use dropset::DropsetError;
use solana_pubkey::Pubkey;

#[test]
fn admin_can_open_vault_for_another_leader() {
    let mut f = Fixture::bootstrap();
    let foreign = Keypair::new();
    // Admin (authority) seats `foreign` as both leader and quote
    // authority via the admin-only override, at a 10% perf fee.
    f.register_vault(100_000, foreign.pubkey(), false, foreign.pubkey())
        .expect("admin leader-override should succeed");

    let v = f.vault(0);
    assert_eq!(v.leader, foreign.pubkey().to_bytes().into());
    assert_eq!(v.quote_authority, foreign.pubkey().to_bytes().into());
    assert_eq!(v.perf_fee_rate.get(), 100_000);
}

#[test]
fn non_admin_with_foreign_leader_override_rejects() {
    let mut f = Fixture::bootstrap();
    let outsider = f.funded_keypair(10 * SIGNER_FUNDING_LAMPORTS);
    let foreign = Keypair::new();
    // A non-admin overriding the leader to someone other than
    // themselves is rejected — and before the fee transfer, which
    // `register_vault_as` pre-funds so the rejection isn't masked by an
    // insufficient-funds error.
    let err = f
        .register_vault_as(&outsider, 0, outsider.pubkey(), false, foreign.pubkey())
        .expect_err("non-admin override to a foreign pubkey must reject");
    common::assert_program_error(&err, DropsetError::LeaderOverrideNotAllowed);
}

#[test]
fn invalid_perf_fee_rate_rejects() {
    let mut f = Fixture::bootstrap();
    // perf_fee_rate > 1_000_000 ppm (> 100%).
    let err = f
        .register_vault(1_000_001, f.authority.pubkey(), false, Pubkey::default())
        .expect_err("perf_fee_rate > 1_000_000 must reject");
    common::assert_program_error(&err, DropsetError::InvalidPerfFeeRate);
}

#[test]
fn non_admin_pays_open_vault_fee() {
    let mut f = Fixture::bootstrap();
    let bob = f.funded_keypair(SIGNER_FUNDING_LAMPORTS);
    let treasury_before = f.token_balance(&f.registry_fee_treasury);

    f.register_vault_as(&bob, 0, bob.pubkey(), false, Pubkey::default())
        .expect("non-admin opens a vault by paying the fee");

    assert_eq!(
        f.token_balance(&f.registry_fee_treasury) - treasury_before,
        REGISTER_MARKET_FEE_ATOMS,
        "fee credited to the registry treasury"
    );
    assert_eq!(
        f.token_balance(&f.fee_ata(&bob.pubkey())),
        0,
        "payer's fee ATA fully debited"
    );
    assert_eq!(f.market_header().active_count.get(), 1, "vault opened");
}

#[test]
fn rejects_vault_cap_exceeded() {
    let mut f = Fixture::bootstrap();
    // Default cap is 10. Vary perf_fee_rate per call so the
    // transactions aren't byte-identical (LiteSVM dedups signatures).
    for i in 0..10u32 {
        f.register_vault(i, f.authority.pubkey(), false, Pubkey::default())
            .expect("vault within cap");
    }
    assert_eq!(f.market_header().active_count.get(), 10);
    let err = f
        .register_vault(10, f.authority.pubkey(), false, Pubkey::default())
        .expect_err("the 11th vault must exceed the cap");
    common::assert_program_error(&err, DropsetError::VaultCapExceeded);
}

#[test]
fn rejects_default_quote_authority() {
    let mut f = Fixture::bootstrap();
    let err = f
        .register_vault(0, Pubkey::default(), false, Pubkey::default())
        .expect_err("Address::default() quote_authority must reject");
    common::assert_program_error(&err, DropsetError::Unauthorized);
}

#[test]
fn vault_lands_at_active_head_and_increments_count() {
    let mut f = Fixture::bootstrap();
    f.register_vault(0, f.authority.pubkey(), false, Pubkey::default())
        .expect("first vault");
    assert_eq!(
        f.market_header().head.get(),
        0,
        "first vault at active head"
    );
    assert_eq!(f.market_header().active_count.get(), 1);

    // Second vault (distinct perf so the txn differs) is prepended.
    f.register_vault(1, f.authority.pubkey(), false, Pubkey::default())
        .expect("second vault");
    assert_eq!(
        f.market_header().head.get(),
        1,
        "most recent vault prepended at the active head"
    );
    assert_eq!(f.market_header().active_count.get(), 2);
}
