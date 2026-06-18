//! Teardown / rent-reclamation integration tests (the `admin-teardown`
//! feature surface, ENG-433).
//!
//! The headline test drives a full build-up — `init` → `create_market`
//! → `create_vault` → seed → outside `deposit` — and then the complete
//! teardown in the spec's prescribed order
//! (architecture.md § Account lifecycle and rent reclamation → Teardown
//! ordering):
//!
//!   force_withdraw_depositor (per depositor)
//!     → force_withdraw_leader (per vault)
//!     → close_market_treasury (per leg)
//!     → close_market
//!     → close_registry_fee_vault
//!     → close_registry
//!
//! At each step it asserts both halves of the ticket: every party gets
//! their rent / tokens back (depositor PDA rent → depositor, treasury /
//! market / registry rent → the admin's `rent_recipient`), and every
//! account ends up closed. Two ordering guards confirm the
//! pre-conditions reject out-of-order calls rather than corrupting state.

#![cfg(feature = "admin-teardown")]

mod common;

use anchor_v2_testing::{LiteSVM, Signer};
use common::fixture::{simple_profile, Fixture};
use dropset::Price;
use solana_pubkey::Pubkey;

/// Lamports balance of `pk`, or 0 if the account does not exist (closed
/// accounts are purged once their lamports hit zero).
fn lamports(svm: &LiteSVM, pk: &Pubkey) -> u64 {
    svm.get_account(pk).map(|a| a.lamports).unwrap_or(0)
}

/// Whether `pk` is a live account (exists with non-zero lamports).
fn exists(svm: &LiteSVM, pk: &Pubkey) -> bool {
    svm.get_account(pk).map(|a| a.lamports > 0).unwrap_or(false)
}

#[test]
fn full_buildup_teardown_reclaims_all_rent() {
    // ── Build-up ─────────────────────────────────────────────────────
    // A vault led by a *distinct* keypair (not the admin), so the
    // force-withdraw teardown reflects the real shape — an operator
    // winding down someone else's vault — and the admin / leader accounts
    // never alias (Anchor v2 rejects duplicate mutable accounts).
    let mut f = Fixture::bootstrap();
    let admin = f.authority.insecure_clone();
    let leader = f.funded_keypair(10 * common::SIGNER_FUNDING_LAMPORTS);

    // Admin opens the vault on the leader's behalf (leader_override),
    // with the leader as quote authority and outside deposits enabled.
    f.create_vault(0, leader.pubkey(), true, leader.pubkey())
        .expect("admin opens leader's vault");
    let px = Price::encode(10_850_000, 0).unwrap();
    f.set_reference_price(&leader, 0, px.as_u32(), 0)
        .expect("leader sets reference price");
    f.set_liquidity_profile(&leader, 0, simple_profile(5_000, 10_000, u32::MAX))
        .expect("leader sets ladder");
    f.deposit_leader_as(&leader, 0, 1_000_000, 1_085_000, 1_000_000, 1_085_000)
        .expect("leader seeds the vault");
    // Admin approves outside deposits (leader opted in at open).
    f.set_outside_deposits_approved(&admin, 0, true)
        .expect("admin approves");

    let alice = f.funded_depositor(200_000, 200_000);
    f.deposit(&alice, 0, 50_000, 0, 200_000, 200_000)
        .expect("outside deposit");
    assert_eq!(f.market_header().outstanding_vault_depositors.get(), 1);
    assert_eq!(f.registry_market_count(), 1, "one live market");

    // A dedicated wallet to catch all reclaimed PDA / ATA rent so we can
    // assert the admin (operator) recovers it. Funded so it is a live
    // account from the start.
    let rent_recipient = f.funded_keypair(common::SIGNER_FUNDING_LAMPORTS);
    let rr = rent_recipient.pubkey();

    let (alice_b, alice_q) = (f.base_ata(&alice.pubkey()), f.quote_ata(&alice.pubkey()));
    let (lead_b, lead_q) = (f.base_ata(&leader.pubkey()), f.quote_ata(&leader.pubkey()));
    let alice_base_before = f.token_balance(&alice_b);
    let alice_quote_before = f.token_balance(&alice_q);
    let alice_lamports_before = lamports(&f.svm, &alice.pubkey());
    let lead_base_before = f.token_balance(&lead_b);
    let lead_quote_before = f.token_balance(&lead_q);

    // The registry fee vault never collected anything — every build-up
    // call was admin-signed, so the open fee was waived. It can be closed
    // without a sweep.
    assert_eq!(f.token_balance(&f.registry_fee_treasury), 0);

    // (Ordering pre-conditions are covered by the standalone
    // `close_*_rejects_*` tests below — repeating a close here against a
    // fresh blockhash would collide with the real close later under
    // LiteSVM's signature dedup.)

    // ── Step 1: force-withdraw the outside depositor ─────────────────
    f.force_withdraw_depositor(&admin, 0, &alice.pubkey())
        .expect("force_withdraw_depositor");
    assert!(
        f.vault_depositor(0, &alice.pubkey()).is_none(),
        "VaultDepositor PDA closed"
    );
    assert_eq!(
        f.market_header().outstanding_vault_depositors.get(),
        0,
        "outstanding depositor counter back to zero"
    );
    // Alice got her basket back and her PDA rent refunded (to her, not
    // the admin who initiated the close).
    assert!(f.token_balance(&alice_b) > alice_base_before);
    assert!(f.token_balance(&alice_q) > alice_quote_before);
    assert!(
        lamports(&f.svm, &alice.pubkey()) > alice_lamports_before,
        "depositor PDA rent refunded to the depositor"
    );

    // ── Step 2: force-withdraw the leader ────────────────────────────
    f.force_withdraw_leader(&admin, 0, &leader.pubkey())
        .expect("force_withdraw_leader");
    let v = f.vault(0);
    assert_eq!(v.total_shares.get(), 0, "vault fully drained");
    assert_eq!(v.leader_shares.get(), 0);
    assert_eq!(v.base_atoms.get(), 0);
    assert_eq!(v.quote_atoms.get(), 0);
    // Sector reclaimed to the free DLL: zeroed leader, off the active
    // list, and the free head now points at it.
    assert_eq!(v.leader, Pubkey::default().to_bytes().into());
    let h = f.market_header();
    assert_eq!(h.active_count.get(), 0, "active count dropped to zero");
    assert_eq!(h.head.get(), dropset::NULL_SECTOR, "active list empty");
    assert_eq!(h.free_head.get(), 0, "sector 0 reclaimed onto free list");
    // Treasuries fully drained — the close pre-condition.
    assert_eq!(f.token_balance(&f.base_treasury), 0);
    assert_eq!(f.token_balance(&f.quote_treasury), 0);
    // Leader received the remaining inventory.
    assert!(f.token_balance(&lead_b) > lead_base_before);
    assert!(f.token_balance(&lead_q) > lead_quote_before);

    // ── Step 3: close both treasuries ────────────────────────────────
    // Both treasuries are now drained, so they close cleanly. The
    // rejection of a still-funded treasury is covered separately by
    // `close_treasury_rejects_nonempty`.
    let (base_mint, quote_mint) = (f.base_mint, f.quote_mint);
    let (base_treasury, quote_treasury) = (f.base_treasury, f.quote_treasury);
    let rr_before_treasuries = lamports(&f.svm, &rr);
    f.close_market_treasury(&admin, &base_mint, &base_treasury, &rr)
        .expect("close base treasury");
    f.close_market_treasury(&admin, &quote_mint, &quote_treasury, &rr)
        .expect("close quote treasury");
    assert!(!exists(&f.svm, &base_treasury), "base treasury closed");
    assert!(!exists(&f.svm, &quote_treasury), "quote treasury closed");
    assert!(
        lamports(&f.svm, &rr) > rr_before_treasuries,
        "treasury rent landed with the operator"
    );

    // ── Step 4: close the market ─────────────────────────────────────
    let market = f.market;
    let rr_before_market = lamports(&f.svm, &rr);
    f.close_market(&admin, &rr).expect("close market");
    assert!(!exists(&f.svm, &market), "market account closed");
    assert!(
        lamports(&f.svm, &rr) > rr_before_market,
        "market rent landed with the operator"
    );
    // registry.market_count back to zero — the witness close_registry needs.
    assert_eq!(f.registry_market_count(), 0);

    // ── Step 5: close the registry fee vault ─────────────────────────
    let fee_vault = f.registry_fee_treasury;
    f.close_registry_fee_vault(&admin, &rr)
        .expect("close fee vault");
    assert!(!exists(&f.svm, &fee_vault), "registry fee vault closed");

    // ── Step 6: close the registry ───────────────────────────────────
    let registry = f.registry;
    let rr_before_registry = lamports(&f.svm, &rr);
    f.close_registry(&admin, &rr).expect("close registry");
    assert!(!exists(&f.svm, &registry), "registry account closed");
    assert!(
        lamports(&f.svm, &rr) > rr_before_registry,
        "registry rent landed with the operator"
    );

    // ── Final: zero on-chain state remains ───────────────────────────
    assert!(!exists(&f.svm, &market));
    assert!(!exists(&f.svm, &registry));
    assert!(!exists(&f.svm, &base_treasury));
    assert!(!exists(&f.svm, &quote_treasury));
    assert!(!exists(&f.svm, &fee_vault));
    assert!(f.vault_depositor(0, &alice.pubkey()).is_none());
}

/// The teardown fee sweep must close *every* historical fee mint's ATA,
/// not just the bootstrap default. ENG-508 (PR #111) made
/// `set_market_fee_config` create a registry fee ATA per fee mint via
/// `init_if_needed`, so re-pointing a market at a fresh mint leaves the
/// registry holding a *second* fee ATA — exactly the case the sweep doc
/// comment in `retune.rs` (and the spec's *Account lifecycle and rent
/// reclamation*) promises is covered. This drives it end-to-end:
/// re-point the market, then close *both* fee ATAs via
/// `close_registry_fee_vault`.
#[test]
fn teardown_sweeps_every_historical_fee_mint() {
    let mut f = Fixture::bootstrap();
    let admin = f.authority.insecure_clone();
    let rr = f.funded_keypair(common::SIGNER_FUNDING_LAMPORTS).pubkey();

    // The bootstrap default fee ATA, created at `init`.
    let default_fee_vault = f.registry_fee_treasury;
    assert!(exists(&f.svm, &default_fee_vault), "bootstrap fee ATA live");

    // Re-point the market at a fresh fee mint. `set_market_fee_config`
    // eagerly creates the matching registry fee ATA (`init_if_needed`),
    // so the registry now holds *two* fee ATAs — the multi-mint shape the
    // sweep has to handle.
    let new_mint = common::create_spl_mint(&mut f.svm, &admin);
    f.set_market_fee_config(&admin, &new_mint, &common::SPL_TOKEN_PROGRAM_ID, 42_000)
        .expect("admin re-points the market fee at a fresh mint");
    let new_fee_vault =
        common::associated_token_address(&f.registry, &new_mint, &common::SPL_TOKEN_PROGRAM_ID);
    assert!(exists(&f.svm, &new_fee_vault), "second fee ATA created");
    assert_ne!(
        default_fee_vault, new_fee_vault,
        "the re-point yields a distinct second fee ATA"
    );

    // Every build-up call was admin-signed, so the open fee was waived and
    // both fee vaults are empty — the close pre-condition holds for each.
    assert_eq!(f.token_balance(&default_fee_vault), 0);
    assert_eq!(f.token_balance(&new_fee_vault), 0);

    // ── The sweep: close *both* historical fee ATAs ──────────────────
    let rr_before = lamports(&f.svm, &rr);
    f.close_registry_fee_vault(&admin, &rr)
        .expect("close the bootstrap default fee ATA");
    f.close_registry_fee_vault_for(&admin, &new_mint, &common::SPL_TOKEN_PROGRAM_ID, &rr)
        .expect("close the re-pointed mint's fee ATA");

    assert!(
        !exists(&f.svm, &default_fee_vault),
        "default fee ATA closed by the sweep"
    );
    assert!(
        !exists(&f.svm, &new_fee_vault),
        "re-pointed mint's fee ATA closed by the sweep"
    );
    assert!(
        lamports(&f.svm, &rr) > rr_before,
        "both fee ATAs' rent landed with the operator"
    );
}

/// A non-empty treasury cannot be closed — the rent reclamation order
/// requires draining (force-withdraw) first.
#[test]
fn close_treasury_rejects_nonempty() {
    let mut f = Fixture::seeded(1_000_000, 1_085_000);
    let admin = f.authority.insecure_clone();
    let rr = f.funded_keypair(common::SIGNER_FUNDING_LAMPORTS).pubkey();
    // The seed left both treasuries holding inventory.
    assert!(f.token_balance(&f.base_treasury) > 0);
    let (base_mint, base_treasury) = (f.base_mint, f.base_treasury);
    let err = f
        .close_market_treasury(&admin, &base_mint, &base_treasury, &rr)
        .expect_err("treasury with a balance must not close");
    common::assert_program_error(&err, dropset::DropsetError::TokenAccountNotEmpty);
}

/// Only a registry admin may drive the teardown surface.
#[test]
fn force_withdraw_leader_rejects_non_admin() {
    let mut f = Fixture::seeded(1_000_000, 1_085_000);
    let stranger = f.funded_keypair(10 * common::SIGNER_FUNDING_LAMPORTS);
    let leader = f.authority.pubkey();
    let err = f
        .force_withdraw_leader(&stranger, 0, &leader)
        .expect_err("non-admin cannot force-withdraw");
    common::assert_program_error(&err, dropset::DropsetError::Unauthorized);
}

/// Ordering pre-condition: a market with open treasuries cannot be
/// closed — the treasuries must be drained and closed first.
#[test]
fn close_market_rejects_open_treasury() {
    let mut f = Fixture::seeded(1_000_000, 1_085_000);
    let admin = f.authority.insecure_clone();
    let rr = f.funded_keypair(common::SIGNER_FUNDING_LAMPORTS).pubkey();
    // A freshly seeded, leader-only vault has no outstanding depositors,
    // so close_market clears that gate but trips on the live treasuries.
    let err = f
        .close_market(&admin, &rr)
        .expect_err("close_market must reject while treasuries are open");
    common::assert_program_error(&err, dropset::DropsetError::MarketTreasuryNotClosed);
}

/// Ordering pre-condition: the registry cannot be closed while it still
/// has live markets (`market_count != 0`).
#[test]
fn close_registry_rejects_live_markets() {
    let mut f = Fixture::seeded(1_000_000, 1_085_000);
    let admin = f.authority.insecure_clone();
    let rr = f.funded_keypair(common::SIGNER_FUNDING_LAMPORTS).pubkey();
    let err = f
        .close_registry(&admin, &rr)
        .expect_err("close_registry must reject while market_count > 0");
    common::assert_program_error(&err, dropset::DropsetError::RegistryHasMarkets);
}

/// `force_withdraw_depositor` must reclaim a sector it empties — the
/// invariant "a drained sector is reclaimed" has to hold regardless of
/// teardown order. Here we deliberately drain the leader *first* (out of
/// the documented depositors-first order), leaving the last depositor to
/// zero `total_shares`; the depositor path is then the one that has to
/// reclaim. (Mirrors the leader path's `new_total == 0` reclaim.)
#[test]
fn force_withdraw_depositor_reclaims_emptied_sector() {
    let mut f = Fixture::bootstrap();
    let (leader, alice) = f.with_outside_depositor();
    let admin = f.authority.insecure_clone();

    // Leader exits first. The vault still holds alice's shares, so the
    // sector is *not* reclaimed yet: leader preserved, still on the
    // active list.
    f.force_withdraw_leader(&admin, 0, &leader.pubkey())
        .expect("force_withdraw_leader");
    let v = f.vault(0);
    assert_eq!(v.leader_shares.get(), 0, "leader stake drained");
    assert!(
        v.total_shares.get() > 0,
        "depositor shares still outstanding"
    );
    assert_ne!(
        v.leader,
        Pubkey::default().to_bytes().into(),
        "sector not reclaimed while a depositor remains — leader preserved"
    );
    assert_eq!(f.market_header().active_count.get(), 1, "still active");

    // Last depositor exits — this drives `total_shares -> 0`, so the
    // depositor path must reclaim the sector.
    f.force_withdraw_depositor(&admin, 0, &alice.pubkey())
        .expect("force_withdraw_depositor");
    let v = f.vault(0);
    assert_eq!(v.total_shares.get(), 0, "vault fully drained");
    assert_eq!(
        v.leader,
        Pubkey::default().to_bytes().into(),
        "sector reclaimed on the depositor path — leader zeroed"
    );
    let h = f.market_header();
    assert_eq!(h.active_count.get(), 0, "active count dropped to zero");
    assert_eq!(h.head.get(), dropset::NULL_SECTOR, "active list now empty");
    assert_eq!(
        h.free_head.get(),
        0,
        "sector 0 reclaimed onto the free list"
    );
    assert_eq!(
        h.outstanding_vault_depositors.get(),
        0,
        "depositor counter back to zero"
    );
}

/// Only a registry admin may force-withdraw a depositor.
#[test]
fn force_withdraw_depositor_rejects_non_admin() {
    let mut f = Fixture::bootstrap();
    let (_leader, alice) = f.with_outside_depositor();
    let stranger = f.funded_keypair(10 * common::SIGNER_FUNDING_LAMPORTS);
    let err = f
        .force_withdraw_depositor(&stranger, 0, &alice.pubkey())
        .expect_err("non-admin cannot force-withdraw a depositor");
    common::assert_program_error(&err, dropset::DropsetError::Unauthorized);
}

/// Only a registry admin may close a market treasury.
#[test]
fn close_market_treasury_rejects_non_admin() {
    let mut f = Fixture::seeded(1_000_000, 1_085_000);
    let stranger = f.funded_keypair(10 * common::SIGNER_FUNDING_LAMPORTS);
    let rr = f.funded_keypair(common::SIGNER_FUNDING_LAMPORTS).pubkey();
    let (base_mint, base_treasury) = (f.base_mint, f.base_treasury);
    let err = f
        .close_market_treasury(&stranger, &base_mint, &base_treasury, &rr)
        .expect_err("non-admin cannot close a market treasury");
    common::assert_program_error(&err, dropset::DropsetError::Unauthorized);
}

/// Only a registry admin may close the market.
#[test]
fn close_market_rejects_non_admin() {
    let mut f = Fixture::seeded(1_000_000, 1_085_000);
    let stranger = f.funded_keypair(10 * common::SIGNER_FUNDING_LAMPORTS);
    let rr = f.funded_keypair(common::SIGNER_FUNDING_LAMPORTS).pubkey();
    let err = f
        .close_market(&stranger, &rr)
        .expect_err("non-admin cannot close the market");
    common::assert_program_error(&err, dropset::DropsetError::Unauthorized);
}

/// Only a registry admin may close the registry fee vault.
#[test]
fn close_registry_fee_vault_rejects_non_admin() {
    let mut f = Fixture::seeded(1_000_000, 1_085_000);
    let stranger = f.funded_keypair(10 * common::SIGNER_FUNDING_LAMPORTS);
    let rr = f.funded_keypair(common::SIGNER_FUNDING_LAMPORTS).pubkey();
    let err = f
        .close_registry_fee_vault(&stranger, &rr)
        .expect_err("non-admin cannot close the registry fee vault");
    common::assert_program_error(&err, dropset::DropsetError::Unauthorized);
}

/// Only a registry admin may close the registry.
#[test]
fn close_registry_rejects_non_admin() {
    let mut f = Fixture::seeded(1_000_000, 1_085_000);
    let stranger = f.funded_keypair(10 * common::SIGNER_FUNDING_LAMPORTS);
    let rr = f.funded_keypair(common::SIGNER_FUNDING_LAMPORTS).pubkey();
    let err = f
        .close_registry(&stranger, &rr)
        .expect_err("non-admin cannot close the registry");
    common::assert_program_error(&err, dropset::DropsetError::Unauthorized);
}

/// `close_market` must reject while any `VaultDepositor` PDA is still
/// open — `outstanding_vault_depositors` is the witness, checked before
/// the treasury gate.
#[test]
fn close_market_rejects_with_outstanding_depositors() {
    let mut f = Fixture::bootstrap();
    f.with_outside_depositor();
    let admin = f.authority.insecure_clone();
    let rr = f.funded_keypair(common::SIGNER_FUNDING_LAMPORTS).pubkey();
    assert_eq!(f.market_header().outstanding_vault_depositors.get(), 1);
    let err = f
        .close_market(&admin, &rr)
        .expect_err("close_market must reject while a depositor PDA is open");
    common::assert_program_error(&err, dropset::DropsetError::MarketHasDepositors);
}

/// `close_market_treasury` must reject a market-owned ATA whose mint is
/// neither market leg. The `associated_token` constraint resolves (the
/// account *is* `ata(market, mint)`), so the handler's explicit leg
/// check is what rejects it.
#[test]
fn close_treasury_rejects_non_leg_mint() {
    let mut f = Fixture::seeded(1_000_000, 1_085_000);
    let admin = f.authority.insecure_clone();
    let rr = f.funded_keypair(common::SIGNER_FUNDING_LAMPORTS).pubkey();
    let market = f.market;
    let other_mint = common::create_spl_mint(&mut f.svm, &admin);
    let other_treasury = common::create_associated_token_account(
        &mut f.svm,
        &admin,
        &market,
        &other_mint,
        &common::SPL_TOKEN_PROGRAM_ID,
    );
    let err = f
        .close_market_treasury(&admin, &other_mint, &other_treasury, &rr)
        .expect_err("a non-leg market-owned ATA must not be closeable");
    common::assert_program_error(&err, dropset::DropsetError::NotAMarketTreasury);
}
