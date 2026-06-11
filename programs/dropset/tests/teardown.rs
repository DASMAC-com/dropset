//! Teardown / rent-reclamation integration tests (the `admin-teardown`
//! feature surface, ENG-433).
//!
//! The headline test drives a full build-up — `init` → `register_market`
//! → `register_vault` → seed → outside `deposit` — and then the complete
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

/// Lamport balance of `pk`, or 0 if the account does not exist (closed
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
    f.register_vault(0, leader.pubkey(), true, leader.pubkey())
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
    // Drained treasury cannot still be closed twice — but first confirm
    // a non-empty one would be rejected is covered elsewhere; here both
    // are empty.
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
