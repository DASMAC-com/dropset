// cspell:word undrained
//! Teardown / rent-reclamation integration tests (the `admin-teardown`
//! feature surface).
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
//!
//! The `close_*` steps drain any remaining token balance to a supplied
//! `token_recipient` before closing, so three further tests cover what
//! that reaches which the headline zero-balance run cannot: a market that
//! charged a taker fee, a treasury holding an unsolicited transfer, and a
//! registry fee vault holding collected market-creation fees. Each of
//! those balances would otherwise block its close permanently.

#![cfg(feature = "admin-teardown")]

mod common;

use anchor_v2_testing::{Keypair, LiteSVM, Signer};
use common::fixture::{simple_profile, Fixture};
use common::{create_associated_token_account, SIGNER_FUNDING_LAMPORTS, SPL_TOKEN_PROGRAM_ID};
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
    // Both treasuries are now drained, so they close cleanly with nothing
    // to pay out. The rejection of a treasury whose vaults still hold
    // inventory is covered separately by
    // `close_treasury_rejects_undrained_vaults`, and the non-zero drain
    // paths by the three tests that follow it.
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

/// Teardown's whole point is redeploying the program at the same id — so
/// bootstrap has to survive a hostile gap between the teardown and the
/// re-`init`. Every address the second bootstrap needs is derivable
/// while the accounts are gone: the registry is the fixed `[b"registry"]`
/// seed, the market PDA is `(base_mint, quote_mint)`, and all three
/// treasuries are ATAs over those. So the moment teardown closes them, a
/// griefer can re-create any of them for the cost of rent. Under plain
/// `init` that won the race permanently; `init_if_needed` makes it a
/// no-op.
#[test]
fn squatted_atas_do_not_block_bootstrap_after_teardown() {
    // Minimal build-up: a market with no vaults, so both treasuries are
    // already empty and close without any force-withdraw.
    let mut f = Fixture::bootstrap();
    let admin = f.authority.insecure_clone();
    let rent_recipient = f.funded_keypair(SIGNER_FUNDING_LAMPORTS);
    let rr = rent_recipient.pubkey();

    let (registry, market) = (f.registry, f.market);
    let (base_mint, quote_mint) = (f.base_mint, f.quote_mint);
    let (base_treasury, quote_treasury) = (f.base_treasury, f.quote_treasury);
    let (fee_mint, fee_vault) = (f.fee_mint, f.registry_fee_treasury);

    // ── Tear the whole deployment down ───────────────────────────────
    f.close_market_treasury(&admin, &base_mint, &base_treasury, &rr)
        .expect("close base treasury");
    f.close_market_treasury(&admin, &quote_mint, &quote_treasury, &rr)
        .expect("close quote treasury");
    f.close_market(&admin, &rr).expect("close market");
    f.close_registry_fee_vault(&admin, &rr)
        .expect("close fee vault");
    f.close_registry(&admin, &rr).expect("close registry");
    assert!(!exists(&f.svm, &registry));
    assert!(!exists(&f.svm, &market));
    assert!(!exists(&f.svm, &fee_vault));
    assert!(!exists(&f.svm, &base_treasury));
    assert!(!exists(&f.svm, &quote_treasury));

    // ── A griefer squats every ATA the redeploy will need ────────────
    let squatter = Keypair::new();
    f.svm
        .airdrop(&squatter.pubkey(), 10 * SIGNER_FUNDING_LAMPORTS)
        .unwrap();
    for (owner, mint) in [
        (registry, fee_mint),
        (market, base_mint),
        (market, quote_mint),
    ] {
        create_associated_token_account(
            &mut f.svm,
            &squatter,
            &owner,
            &mint,
            &SPL_TOKEN_PROGRAM_ID,
        );
    }
    assert!(exists(&f.svm, &fee_vault), "fee vault squatted");
    assert!(exists(&f.svm, &base_treasury), "base treasury squatted");
    assert!(exists(&f.svm, &quote_treasury), "quote treasury squatted");

    // ── Bootstrap again anyway ───────────────────────────────────────
    // Same re-bootstrap `full_lifecycle_teardown_then_bootstrap_again_at_
    // the_same_addresses` runs on a clean slate; the squat above is the
    // only difference. It expires the blockhash first, because the
    // redeploy's two instructions are byte-identical to the pair
    // `Fixture::bootstrap` already sent and LiteSVM would otherwise dedup
    // them as `AlreadyProcessed` before the program ran.
    f.init_and_create_market();

    // Everything is back at the same addresses, and the squatter owns
    // none of it — the ATA derivation pins each authority.
    assert!(exists(&f.svm, &registry));
    assert!(exists(&f.svm, &market));
    assert_eq!(f.registry_market_count(), 1, "market live again");
    let header = f.market_header();
    assert_eq!(header.base_treasury, base_treasury.to_bytes().into());
    assert_eq!(header.quote_treasury, quote_treasury.to_bytes().into());
    assert_eq!(header.base_mint, base_mint.to_bytes().into());
    assert_eq!(header.quote_mint, quote_mint.to_bytes().into());
    // Every adopted ATA answers to its PDA, not to the squatter.
    for treasury in [base_treasury, quote_treasury] {
        assert_eq!(
            f.token_account_owner(&treasury),
            market,
            "treasury authority is the market"
        );
    }
    assert_eq!(
        f.token_account_owner(&fee_vault),
        registry,
        "fee vault authority is the registry"
    );
}

/// The teardown fee sweep must close *every* historical fee mint's ATA,
/// not just the bootstrap default. `set_market_fee_config` creates a
/// registry fee ATA per fee mint via `init_if_needed`, so re-pointing a
/// market at a fresh mint leaves the registry holding a *second* fee ATA
/// — exactly the case the sweep doc comment in `retune.rs` (and the
/// spec's *Account lifecycle and rent reclamation*) promises is covered.
/// This drives it end-to-end: re-point the market, then close *both* fee
/// ATAs via `close_registry_fee_vault`.
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

    // No market activity ran against either fee mint — no `create_vault`
    // ever charged a fee — so both fee vaults are empty and each close is
    // the zero-balance path, with nothing to drain. The collected-fee case
    // is `close_registry_fee_vault_drains_collected_fees`.
    assert_eq!(f.token_balance(&default_fee_vault), 0);
    assert_eq!(f.token_balance(&new_fee_vault), 0);

    // Close the market first — a fee vault may only go once every market
    // is closed, since `create_vault` and `create_market` both take it as
    // a plain constrained account.
    let (base_mint, quote_mint) = (f.base_mint, f.quote_mint);
    let (base_treasury, quote_treasury) = (f.base_treasury, f.quote_treasury);
    f.close_market_treasury(&admin, &base_mint, &base_treasury, &rr)
        .expect("close base treasury");
    f.close_market_treasury(&admin, &quote_mint, &quote_treasury, &rr)
        .expect("close quote treasury");
    f.close_market(&admin, &rr).expect("close market");

    // ── The sweep: close *both* historical fee ATAs ──────────────────
    // Close each in turn, asserting the operator's balance climbs on
    // *each* close — so both ATAs' rent is individually accounted for,
    // not just one. (`rr` signs nothing, so its balance can only rise.)
    let rr_before_default = lamports(&f.svm, &rr);
    f.close_registry_fee_vault(&admin, &rr)
        .expect("close the bootstrap default fee ATA");
    let rr_after_default = lamports(&f.svm, &rr);
    assert!(
        !exists(&f.svm, &default_fee_vault),
        "default fee ATA closed by the sweep"
    );
    assert!(
        rr_after_default > rr_before_default,
        "default fee ATA rent landed with the operator"
    );

    f.close_registry_fee_vault_for(&admin, &new_mint, &common::SPL_TOKEN_PROGRAM_ID, &rr)
        .expect("close the re-pointed mint's fee ATA");
    assert!(
        !exists(&f.svm, &new_fee_vault),
        "re-pointed mint's fee ATA closed by the sweep"
    );
    assert!(
        lamports(&f.svm, &rr) > rr_after_default,
        "re-pointed mint's fee ATA rent landed with the operator"
    );
}

/// A treasury whose vaults still hold inventory cannot be closed — the
/// rent reclamation order requires draining (force-withdraw) first.
///
/// This is the guard that keeps drain-on-close honest: the close pays the
/// treasury's remaining balance out to `token_recipient`, so without it an
/// operator could skip the force-withdraws and route depositor principal
/// to themselves. It rejects on the *vaults'* claim rather than on the
/// token balance, which is what lets the accrued protocol fee through.
#[test]
fn close_treasury_rejects_undrained_vaults() {
    let mut f = Fixture::seeded(1_000_000, 1_085_000);
    let admin = f.authority.insecure_clone();
    let rr = f.funded_keypair(common::SIGNER_FUNDING_LAMPORTS).pubkey();
    // The seed left both treasuries holding inventory, claimed by the vault.
    assert!(f.token_balance(&f.base_treasury) > 0);
    assert!(f.vault(0).base_atoms.get() > 0);
    let (base_mint, base_treasury) = (f.base_mint, f.base_treasury);
    let err = f
        .close_market_treasury(&admin, &base_mint, &base_treasury, &rr)
        .expect_err("treasury whose vaults hold inventory must not close");
    common::assert_program_error(&err, dropset::DropsetError::MarketVaultsNotDrained);
}

/// A treasury may not be closed out from under a **live** market, even
/// when that leg's vaults happen to hold none of it.
///
/// This is the case the per-leg claim check alone does not cover, and the
/// reason `close_market_treasury` also requires an empty active list. A
/// vault bought out of its base entirely sits at `Σ base_atoms == 0`
/// while trading normally — an ordinary end state, not a contrived one.
/// Without the second guard an admin could harvest that leg's accrued
/// fees and destroy the ATA under a live market, and nothing re-creates a
/// treasury for a market that already exists, so the leg would be bricked
/// permanently.
#[test]
fn close_treasury_rejects_a_live_market_with_an_empty_leg() {
    let mut f = Fixture::seeded(1_000_000, 1_085_000);
    let admin = f.authority.insecure_clone();
    let rr = f.funded_keypair(common::SIGNER_FUNDING_LAMPORTS).pubkey();
    f.set_taker_fee(&admin, 10_000).expect("1% taker fee");

    // Buy the vault out of its base entirely. The vault stays live and on
    // the active list — it still quotes, and still holds quote inventory.
    let taker = f.funded_depositor(0, 10_000_000);
    f.swap(&taker, 0, 5_000_000, Price::INFINITY.as_u32(), 1)
        .expect("taker sweeps the ask side");

    // The claim check passes — there is genuinely no base claim left —
    // and the leg holds accrued protocol fees, so a drain would pay out.
    assert_eq!(f.vault(0).base_atoms.get(), 0, "base fully bought out");
    assert!(
        f.market_header().accrued_base_fee_atoms.get() > 0,
        "the fills accrued a base-leg fee worth harvesting"
    );
    assert_eq!(f.market_header().active_count.get(), 1, "market is live");

    let (base_mint, base_treasury) = (f.base_mint, f.base_treasury);
    let err = f
        .close_market_treasury(&admin, &base_mint, &base_treasury, &rr)
        .expect_err("a live market's treasury must not be closeable");
    common::assert_program_error(&err, dropset::DropsetError::MarketHasActiveVaults);
    assert!(
        exists(&f.svm, &base_treasury),
        "the treasury survives the rejected close"
    );
}

/// The headline case for drain-on-close: a market that ever charged a
/// taker fee must still tear down end to end, with the accrued atoms
/// landing in the operator's token account.
///
/// Before the drain, this market was **impossible to close**. The fee is
/// booked to `accrued_<leg>_fee_atoms` and stays in the treasury; no
/// harvest instruction exists, and `sweep_residual` deliberately
/// subtracts the accrued counter rather than paying it out — so after
/// every depositor and leader is force-withdrawn the treasury still held a
/// balance, and the close's old empty-account requirement hard-rejected
/// forever. Localnet teardown only ever worked because
/// `DEFAULT_TAKER_FEE` is zero.
#[test]
fn fee_charging_market_tears_down_and_drains_the_accrued_fee() {
    let mut f = Fixture::bootstrap();
    let (leader, alice) = f.with_outside_depositor();
    let admin = f.authority.insecure_clone();
    f.set_taker_fee(&admin, 10_000).expect("1% taker fee");

    // A fee'd Buy: the fee slice is withheld from the taker's proceeds and
    // booked to the market's base-leg accumulator, claimed by no vault.
    let taker = f.funded_depositor(0, 200_000);
    f.swap(&taker, 0, 100_000, Price::INFINITY.as_u32(), 1)
        .expect("fee-bearing swap");
    let accrued = f.market_header().accrued_base_fee_atoms.get();
    assert!(accrued > 0, "the fill accrued a protocol fee");

    // Pay out every party's claim, in the documented order.
    f.force_withdraw_depositor(&admin, 0, &alice.pubkey())
        .expect("force_withdraw_depositor");
    f.force_withdraw_leader(&admin, 0, &leader.pubkey())
        .expect("force_withdraw_leader");

    // The vaults are empty, yet the treasury is *not* — it holds exactly
    // the accrued fee. This is the state that used to be terminal.
    let (base_mint, quote_mint) = (f.base_mint, f.quote_mint);
    let (base_treasury, quote_treasury) = (f.base_treasury, f.quote_treasury);
    assert_eq!(f.vault(0).base_atoms.get(), 0, "no vault claim remains");
    assert_eq!(
        f.token_balance(&base_treasury),
        accrued,
        "treasury still holds the accrued fee and nothing else"
    );

    // Drain-on-close hands it to the operator's token account.
    let harvest = f.funded_keypair(common::SIGNER_FUNDING_LAMPORTS);
    let (harvest_base, harvest_quote) = f.create_atas(&harvest.pubkey());
    let rr = f.funded_keypair(common::SIGNER_FUNDING_LAMPORTS).pubkey();
    f.close_market_treasury_to(&admin, &base_mint, &base_treasury, &harvest_base, &rr)
        .expect("base treasury closes despite holding the accrued fee");
    assert_eq!(
        f.token_balance(&harvest_base),
        accrued,
        "the accrued fee was paid to the drain recipient"
    );
    assert_eq!(
        f.market_header().accrued_base_fee_atoms.get(),
        0,
        "the counter is zeroed with the atoms it claimed"
    );
    assert!(!exists(&f.svm, &base_treasury), "base treasury closed");

    // The quote leg accrued nothing (a Buy fees the base leg), so its
    // close is the zero-balance path — no token CPI, nothing paid out.
    f.close_market_treasury_to(&admin, &quote_mint, &quote_treasury, &harvest_quote, &rr)
        .expect("quote treasury closes");
    assert_eq!(
        f.token_balance(&harvest_quote),
        0,
        "a Buy accrues no quote-leg fee"
    );
    assert!(!exists(&f.svm, &quote_treasury), "quote treasury closed");

    // …and the market itself now closes. That is the unblock.
    let market = f.market;
    f.close_market(&admin, &rr)
        .expect("a fee-charging market can be closed");
    assert!(!exists(&f.svm, &market), "market account closed");
    assert_eq!(f.registry_market_count(), 0);
}

/// The operational question teardown exists to answer: can a market that
/// **actually ran** be wound down completely and then stood back up at the
/// same addresses — the state-layer half of "redeploy the program at the
/// same id"?
///
/// So this drives every value-bearing path a live market accumulates, then
/// reclaims all of it and rebuilds:
///
/// 1. a vault with an outside depositor, quoting a ladder;
/// 1. a fee'd fill — protocol revenue into `accrued_base_fee_atoms`, and
///    spread capture that lifts value-per-share above the HWM;
/// 1. a **realized perf fee** — the leader's slice minted as shares on the
///    next touch of the vault;
/// 1. an **unsolicited transfer**, recovered with `sweep_residual` while
///    the market is still live;
/// 1. the full teardown, including the accrued fee leaving via the
///    treasury drain;
/// 1. `init` + `create_market` **again**, on the same still-deployed
///    program and the same mints — so the registry, market and both
///    treasury ATAs are re-created at the exact addresses just closed,
///    owned by the same PDAs, and usable (a fresh vault seeds into them).
#[test]
fn full_lifecycle_teardown_then_bootstrap_again_at_the_same_addresses() {
    let mut f = Fixture::bootstrap();
    let admin = f.authority.insecure_clone();

    // ── A vault that trades ──────────────────────────────────────────
    // 10% perf fee, so the realize below mints a visible leader slice.
    let leader = f.funded_keypair(10 * common::SIGNER_FUNDING_LAMPORTS);
    f.create_vault(100_000, leader.pubkey(), true, leader.pubkey())
        .expect("admin opens the leader's vault");
    let px = Price::encode(10_850_000, 0).unwrap();
    f.set_reference_price(&leader, 0, px.as_u32(), 0)
        .expect("leader quotes");
    f.set_liquidity_profile(&leader, 0, simple_profile(5_000, 10_000, u32::MAX))
        .expect("leader sets a ladder");
    f.deposit_leader_as(&leader, 0, 1_000_000, 1_085_000, 1_000_000, 1_085_000)
        .expect("leader seeds");
    f.set_outside_deposits_approved(&admin, 0, true)
        .expect("admin approves outside deposits");
    let alice = f.funded_depositor(200_000, 200_000);
    f.deposit(&alice, 0, 50_000, 0, 200_000, 200_000)
        .expect("outside deposit");

    // ── Fee'd fills: protocol revenue + spread capture ───────────────
    // A **round trip**, not a single fill. One-way flow profits the vault
    // in value terms but leaves it lopsided, and value-per-share is
    // measured by `isqrt(base·quote)`, which penalizes imbalance — a lone
    // Buy actually drives that number *down*. Selling the base straight
    // back returns the vault near its starting ratio while it keeps the
    // half-spread off both legs, which is what lifts VPS above the HWM and
    // gives the realize below a real gain to work with.
    f.set_taker_fee(&admin, 10_000).expect("1% taker fee");
    let taker = f.funded_depositor(0, 400_000);
    f.swap(&taker, 0, 200_000, Price::INFINITY.as_u32(), 1)
        .expect("fee-bearing Buy");
    let taker_base = f.token_balance(&f.base_ata(&taker.pubkey()));
    assert!(taker_base > 0, "the Buy filled");
    f.swap(&taker, 1, taker_base, Price::ZERO.as_u32(), 1)
        .expect("fee-bearing Sell closing the round trip");

    // Both legs accrued: the Buy paid the taker in base, the Sell in quote.
    let h = f.market_header();
    let (accrued_base, accrued_quote) = (
        h.accrued_base_fee_atoms.get(),
        h.accrued_quote_fee_atoms.get(),
    );
    assert!(
        accrued_base > 0 && accrued_quote > 0,
        "the round trip accrued protocol revenue on both legs"
    );
    f.assert_treasury_invariant();

    // ── Realized P&L: the leader's perf fee, minted as shares ────────
    // The round trip left value-per-share above the HWM stamped at seed
    // time, and the next touch of the vault realizes that gain. Alice
    // trimming her stake is that touch, so this also exercises a depositor
    // exiting a vault that owes its leader a fee.
    let hwm_before = f.vault(0).hwm.get();
    let leader_shares_before = f.vault(0).leader_shares.get();
    f.svm.expire_blockhash();
    f.withdraw(&alice, 0, 10_000, 0, 0)
        .expect("depositor trims her stake, realizing the perf fee");
    let v = f.vault(0);
    assert!(
        v.hwm.get() > hwm_before,
        "HWM advanced — the gain was realized, not carried"
    );
    assert!(
        v.leader_shares.get() > leader_shares_before,
        "perf-fee shares were minted to the leader ({} → {})",
        leader_shares_before,
        v.leader_shares.get()
    );

    // ── An unsolicited transfer, swept while the market is live ──────
    // The pre-teardown recovery path: atoms nobody has a claim on, which
    // no `Withdraw` could ever pay out.
    const STRAY: u64 = 7_777;
    let (base_mint, quote_mint) = (f.base_mint, f.quote_mint);
    let (base_treasury, quote_treasury) = (f.base_treasury, f.quote_treasury);
    common::mint_to(&mut f.svm, &admin, &base_mint, &base_treasury, STRAY);
    let sweeper = f.funded_keypair(common::SIGNER_FUNDING_LAMPORTS);
    let (sweep_dest, _) = f.create_atas(&sweeper.pubkey());
    let meta = f
        .sweep_residual_meta(&admin, &base_mint, &base_treasury, &sweep_dest)
        .expect("sweep the stray transfer");
    let ev = common::events::sweep_residual(&meta);
    assert_eq!(ev.swept, STRAY, "exactly the stray atoms were swept");
    assert_eq!(
        ev.accrued_fee, accrued_base,
        "the accrued fee was left behind"
    );
    assert_eq!(f.token_balance(&sweep_dest), STRAY);
    f.assert_treasury_invariant();

    // ── Teardown: every claim paid, every account closed ─────────────
    let rr = f.funded_keypair(common::SIGNER_FUNDING_LAMPORTS).pubkey();
    f.force_withdraw_depositor(&admin, 0, &alice.pubkey())
        .expect("force_withdraw_depositor");
    f.force_withdraw_leader(&admin, 0, &leader.pubkey())
        .expect("force_withdraw_leader");
    assert_eq!(f.vault(0).total_shares.get(), 0, "vault fully drained");

    // What is left in each treasury is exactly that leg's accrued fee —
    // the realized perf fee was paid in *shares*, so it left with the
    // leader's force-withdraw rather than sitting here.
    assert_eq!(
        f.token_balance(&base_treasury),
        accrued_base,
        "only protocol revenue remains in base custody"
    );
    assert_eq!(
        f.token_balance(&quote_treasury),
        accrued_quote,
        "only protocol revenue remains in quote custody"
    );

    let harvest = f.funded_keypair(common::SIGNER_FUNDING_LAMPORTS);
    let (harvest_base, harvest_quote) = f.create_atas(&harvest.pubkey());
    f.close_market_treasury_to(&admin, &base_mint, &base_treasury, &harvest_base, &rr)
        .expect("close base treasury");
    f.close_market_treasury_to(&admin, &quote_mint, &quote_treasury, &harvest_quote, &rr)
        .expect("close quote treasury");
    assert_eq!(
        f.token_balance(&harvest_base),
        accrued_base,
        "the base-leg accrued fee was harvested on the way out"
    );
    assert_eq!(
        f.token_balance(&harvest_quote),
        accrued_quote,
        "the quote-leg accrued fee was harvested on the way out"
    );
    // Both counters, not just the base one: the handler's leg select is a
    // hand-written `if is_base { … } else { … }`, so without the quote
    // assertion a handler that zeroed the base counter in both arms would
    // pass the whole suite.
    let h = f.market_header();
    assert_eq!(h.accrued_base_fee_atoms.get(), 0, "base counter zeroed");
    assert_eq!(h.accrued_quote_fee_atoms.get(), 0, "quote counter zeroed");
    let market = f.market;
    let registry = f.registry;
    let fee_vault = f.registry_fee_treasury;
    f.close_market(&admin, &rr).expect("close market");
    f.close_registry_fee_vault(&admin, &rr)
        .expect("close fee vault");
    f.close_registry(&admin, &rr).expect("close registry");

    // Zero on-chain state: this is the point at which a real operator
    // would `solana program deploy` a new binary at the same id.
    for closed in [market, registry, fee_vault, base_treasury, quote_treasury] {
        assert!(!exists(&f.svm, &closed), "{closed} closed");
    }

    // ── Stand it back up, same program, same addresses ───────────────
    f.init_and_create_market();

    assert!(exists(&f.svm, &registry), "registry re-created");
    assert!(exists(&f.svm, &fee_vault), "registry fee vault re-created");
    assert_eq!(
        f.market, market,
        "the market PDA is seed-derived, so it returns to the same address"
    );
    assert!(
        exists(&f.svm, &market),
        "market re-created at the same address"
    );
    assert_eq!(f.registry_market_count(), 1, "one live market again");

    // The treasuries are back at the same addresses *and* still owned by
    // the market PDA — the ownership half of the question, which a mere
    // existence check would miss.
    for (leg, treasury) in [("base", base_treasury), ("quote", quote_treasury)] {
        assert!(exists(&f.svm, &treasury), "{leg} treasury re-created");
        assert_eq!(
            f.token_account_owner(&treasury),
            market,
            "{leg} treasury is owned by the market PDA"
        );
        assert_eq!(f.token_balance(&treasury), 0, "{leg} treasury starts empty");
    }

    // And they work: a fresh vault seeds real inventory into them.
    let leader2 = f.funded_keypair(10 * common::SIGNER_FUNDING_LAMPORTS);
    f.create_vault(0, leader2.pubkey(), false, leader2.pubkey())
        .expect("a vault opens on the rebuilt market");
    f.set_reference_price(&leader2, 0, px.as_u32(), 0)
        .expect("leader quotes on the rebuilt market");
    f.set_liquidity_profile(&leader2, 0, simple_profile(5_000, 10_000, u32::MAX))
        .expect("ladder on the rebuilt market");
    f.deposit_leader_as(&leader2, 0, 500_000, 542_500, 500_000, 542_500)
        .expect("seed the rebuilt market");
    assert_eq!(
        f.token_balance(&base_treasury),
        500_000,
        "the re-created treasury takes custody exactly as before"
    );
    f.assert_treasury_invariant();
}

/// Drain-on-close also recovers an **unsolicited transfer**. Anyone can
/// send tokens straight to a treasury ATA, and on the old empty-account
/// rule a single atom of dust from a stranger was enough to block the
/// close — and with it the whole teardown run. No vault is even open here,
/// so the balance is unambiguously nobody's claim.
#[test]
fn close_treasury_recovers_an_unsolicited_transfer() {
    let mut f = Fixture::bootstrap();
    let admin = f.authority.insecure_clone();
    let rr = f.funded_keypair(common::SIGNER_FUNDING_LAMPORTS).pubkey();

    // A stranger's dust lands in the market's base treasury.
    const DUST: u64 = 12_345;
    let (base_mint, base_treasury) = (f.base_mint, f.base_treasury);
    common::mint_to(&mut f.svm, &admin, &base_mint, &base_treasury, DUST);
    assert_eq!(f.token_balance(&base_treasury), DUST);
    assert_eq!(
        f.market_header().accrued_base_fee_atoms.get(),
        0,
        "no fee was ever charged — the balance is purely unsolicited"
    );

    let recipient = f.funded_keypair(common::SIGNER_FUNDING_LAMPORTS);
    let (recipient_base, _) = f.create_atas(&recipient.pubkey());
    f.close_market_treasury_to(&admin, &base_mint, &base_treasury, &recipient_base, &rr)
        .expect("dust must not block the close");
    assert_eq!(
        f.token_balance(&recipient_base),
        DUST,
        "the unsolicited atoms were recovered rather than stranded"
    );
    assert!(!exists(&f.svm, &base_treasury), "base treasury closed");
}

/// The registry fee vault has the same disease as a market treasury, and
/// the same cure. `create_vault` on the non-admin path charges the open
/// fee into the registry's fee ATA, and **no** instruction moves tokens
/// out of it — so a single collected fee used to leave the fee vault, and
/// therefore the registry, permanently impossible to close (blocking the
/// redeploy).
#[test]
fn close_registry_fee_vault_drains_collected_fees() {
    let mut f = Fixture::bootstrap();
    let admin = f.authority.insecure_clone();
    let rr = f.funded_keypair(common::SIGNER_FUNDING_LAMPORTS).pubkey();

    // A non-admin opens a vault and pays the fee — the only way tokens
    // ever enter this account.
    let bob = f.funded_keypair(10 * common::SIGNER_FUNDING_LAMPORTS);
    f.create_vault_as(&bob, 0, bob.pubkey(), false, Pubkey::default())
        .expect("non-admin opens a vault and pays the fee");
    let fee_vault = f.registry_fee_treasury;
    assert_eq!(
        f.token_balance(&fee_vault),
        common::CREATE_MARKET_FEE_ATOMS,
        "the open fee was collected"
    );

    // Wind the market down first — the fee vault may only close once
    // every market is gone. Bob's vault was never seeded, so the leader
    // force-withdraw is the empty-vault reclaim path.
    f.force_withdraw_leader(&admin, 0, &bob.pubkey())
        .expect("reclaim bob's empty vault");
    let (base_mint, quote_mint) = (f.base_mint, f.quote_mint);
    let (base_treasury, quote_treasury) = (f.base_treasury, f.quote_treasury);
    f.close_market_treasury(&admin, &base_mint, &base_treasury, &rr)
        .expect("close base treasury");
    f.close_market_treasury(&admin, &quote_mint, &quote_treasury, &rr)
        .expect("close quote treasury");
    f.close_market(&admin, &rr).expect("close market");

    let fee_mint = f.fee_mint;
    let collector = f.funded_keypair(common::SIGNER_FUNDING_LAMPORTS);
    let collector_ata = f.create_ata_for(&collector.pubkey(), &fee_mint);
    f.close_registry_fee_vault_to(
        &admin,
        &fee_mint,
        &common::SPL_TOKEN_PROGRAM_ID,
        &collector_ata,
        &rr,
    )
    .expect("a fee vault holding collected fees still closes");
    assert_eq!(
        f.token_balance(&collector_ata),
        common::CREATE_MARKET_FEE_ATOMS,
        "the collected fees were paid out, not stranded"
    );
    assert!(!exists(&f.svm, &fee_vault), "registry fee vault closed");
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

/// A vault that was created but never seeded still occupies a sector, so
/// `active_count > 0` blocks `close_market` — but it has no stake to
/// drain, and the share guards used to reject the reclaim with
/// `InsufficientShares` (`0x178e`). `force_withdraw_leader` must instead
/// treat the zero-stake vault as a no-op-then-reclaim, returning the
/// sector to the free list so admin teardown can proceed. This drives the
/// empty-vault path end-to-end: open a vault, never seed it, then
/// force-withdraw the leader and assert the sector is reclaimed and the
/// (empty) treasuries then close so `close_market` clears.
#[test]
fn force_withdraw_leader_reclaims_empty_vault() {
    let mut f = Fixture::bootstrap();
    let admin = f.authority.insecure_clone();
    let leader = f.funded_keypair(10 * common::SIGNER_FUNDING_LAMPORTS);
    let rr = f.funded_keypair(common::SIGNER_FUNDING_LAMPORTS).pubkey();

    // Open a vault on the leader's behalf and stop — no reference price,
    // no ladder, no leader seed. The vault occupies sector 0 with zero
    // stake.
    f.create_vault(0, leader.pubkey(), true, leader.pubkey())
        .expect("admin opens an empty vault");
    let v = f.vault(0);
    assert_eq!(v.total_shares.get(), 0, "vault never seeded");
    assert_eq!(v.leader_shares.get(), 0);
    assert_eq!(f.market_header().active_count.get(), 1, "sector occupied");

    // Force-withdrawing the leader of an empty vault must not error on the
    // share guards — it reclaims the sector instead.
    f.force_withdraw_leader(&admin, 0, &leader.pubkey())
        .expect("force_withdraw_leader reclaims an empty vault");

    // Sector reclaimed to the free DLL: zeroed leader, off the active
    // list, free head pointing at it.
    let v = f.vault(0);
    assert_eq!(
        v.leader,
        Pubkey::default().to_bytes().into(),
        "sector reclaimed — leader zeroed"
    );
    let h = f.market_header();
    assert_eq!(h.active_count.get(), 0, "active count dropped to zero");
    assert_eq!(h.head.get(), dropset::NULL_SECTOR, "active list empty");
    assert_eq!(h.free_head.get(), 0, "sector 0 reclaimed onto free list");

    // An empty vault never funded the treasuries, so they close cleanly
    // and `close_market` then clears — the whole point of the reclaim.
    let (base_mint, quote_mint) = (f.base_mint, f.quote_mint);
    let (base_treasury, quote_treasury) = (f.base_treasury, f.quote_treasury);
    let market = f.market;
    f.close_market_treasury(&admin, &base_mint, &base_treasury, &rr)
        .expect("close base treasury");
    f.close_market_treasury(&admin, &quote_mint, &quote_treasury, &rr)
        .expect("close quote treasury");
    f.close_market(&admin, &rr).expect("close market");
    assert!(
        !exists(&f.svm, &market),
        "market closed after empty reclaim"
    );
    assert_eq!(f.registry_market_count(), 0);
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

/// The registry fee vault may not be closed while a market is live — the
/// registry-side counterpart to `close_market_treasury`'s active-list
/// guard, and the ordering the spec prescribes ("once every market is
/// gone").
///
/// It matters for the same two reasons: the balance a live registry holds
/// is collected fee revenue that the drain now pays out, and `create_vault`
/// / `create_market` both take the fee ATA as a plain constrained account,
/// so destroying it under a live market breaks vault creation outright.
#[test]
fn close_registry_fee_vault_rejects_live_markets() {
    let mut f = Fixture::bootstrap();
    let admin = f.authority.insecure_clone();
    let rr = f.funded_keypair(common::SIGNER_FUNDING_LAMPORTS).pubkey();
    assert_eq!(f.registry_market_count(), 1, "the market is live");

    let fee_vault = f.registry_fee_treasury;
    let err = f
        .close_registry_fee_vault(&admin, &rr)
        .expect_err("the fee vault must not close while a market is live");
    common::assert_program_error(&err, dropset::DropsetError::RegistryHasMarkets);
    assert!(exists(&f.svm, &fee_vault), "the fee vault survives");
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
