// cspell:word sweepable
//! `sweep_residual` integration tests — the admin recovery path for a
//! treasury balance that neither the vaults' depositors nor the accrued
//! protocol fee has a claim on.
//!
//! The invariant under test throughout is
//! `treasury.amount >= Σ vault.<leg>_atoms + accrued_<leg>_fee_atoms`, and
//! the residual is whatever slack that leaves. The interesting cases are
//! an **unsolicited transfer** straight to the treasury ATA (recovered), a
//! **fee'd swap** (accrued revenue, deliberately *not* recovered), and a
//! market quiet since its last sweep (nothing to pay out).
//!
//! The exact-in fill residue is the other thing that lands in this bucket
//! — `swap.rs` and the teardown drill cover it, since it needs a
//! taker-bound fill to produce and this suite's cases are built to isolate
//! the other two terms.

mod common;

use anchor_v2_testing::Signer;
use common::fixture::Fixture;
use dropset::Price;

const SEED_BASE: u64 = 1_000_000;
const SEED_QUOTE: u64 = 1_085_000;

#[test]
fn zero_residual_sweeps_nothing_and_reads_out_the_invariant() {
    let mut f = Fixture::seeded(SEED_BASE, SEED_QUOTE);
    let admin = f.authority.insecure_clone();
    // A recipient with live ATAs to point the sweep at.
    let recipient = f.funded_depositor(0, 0);
    let dest = f.base_ata(&recipient.pubkey());
    let dest_before = f.token_balance(&dest);

    let base_treasury = f.base_treasury;
    let base_mint = f.base_mint;
    let meta = f
        .sweep_residual_meta(&admin, &base_mint, &base_treasury, &dest)
        .expect("sweep succeeds on a healthy market");

    // Nothing moved — the treasury is fully claimed by the seeded vault.
    assert_eq!(f.token_balance(&dest), dest_before, "no residual to sweep");
    assert_eq!(f.token_balance(&base_treasury), SEED_BASE);

    // The event still fires, so the instruction doubles as an on-chain
    // read-out of the invariant's three terms.
    let ev = common::events::sweep_residual(&meta);
    assert_eq!(ev.market, f.market.to_bytes());
    assert_eq!(ev.mint, base_mint.to_bytes());
    assert_eq!(ev.destination, dest.to_bytes());
    assert_eq!(ev.treasury_amount, SEED_BASE);
    assert_eq!(ev.vault_sum, SEED_BASE);
    assert_eq!(ev.accrued_fee, 0);
    assert_eq!(ev.swept, 0);
}

#[test]
fn unsolicited_transfer_is_recovered() {
    let mut f = Fixture::seeded(SEED_BASE, SEED_QUOTE);
    let admin = f.authority.insecure_clone();
    let recipient = f.funded_depositor(0, 0);
    let dest = f.base_ata(&recipient.pubkey());

    // Someone mints straight into the treasury ATA. No vault has a claim
    // on these atoms, so no `Withdraw` could ever move them.
    let base_mint = f.base_mint;
    let base_treasury = f.base_treasury;
    f.donate(&base_mint, &base_treasury, 7_777);
    assert_eq!(f.token_balance(&base_treasury), SEED_BASE + 7_777);

    let meta = f
        .sweep_residual_meta(&admin, &base_mint, &base_treasury, &dest)
        .expect("sweep recovers the donation");

    let ev = common::events::sweep_residual(&meta);
    assert_eq!(ev.treasury_amount, SEED_BASE + 7_777);
    assert_eq!(ev.vault_sum, SEED_BASE);
    assert_eq!(ev.accrued_fee, 0);
    assert_eq!(ev.swept, 7_777);
    assert_eq!(f.token_balance(&dest), 7_777, "residual paid out in full");
    // Invariant restored exactly: treasury back to the vault's claim.
    assert_eq!(f.token_balance(&base_treasury), SEED_BASE);
    assert_eq!(f.vault(0).base_atoms.get(), SEED_BASE);

    // Idempotent — a second sweep finds nothing. Pointed at a *different*
    // destination so the transaction isn't byte-identical to the first
    // (LiteSVM rejects a repeated signature as `AlreadyProcessed`).
    let second = f.funded_depositor(0, 0);
    let dest2 = f.base_ata(&second.pubkey());
    let meta = f
        .sweep_residual_meta(&admin, &base_mint, &base_treasury, &dest2)
        .expect("second sweep is a no-op");
    assert_eq!(common::events::sweep_residual(&meta).swept, 0);
    assert_eq!(f.token_balance(&dest2), 0);
    assert_eq!(f.token_balance(&dest), 7_777);
}

#[test]
fn accrued_taker_fee_is_not_swept() {
    let mut f = Fixture::seeded(SEED_BASE, SEED_QUOTE);
    let admin = f.authority.insecure_clone();
    f.set_taker_fee(&admin, 10_000).expect("1% taker fee");

    let taker = f.funded_depositor(0, 200_000);
    f.swap(&taker, 0, 100_000, Price::INFINITY.as_u32(), 1)
        .expect("fee'd buy");

    let accrued = f.market_header().accrued_base_fee_atoms.get();
    assert!(accrued > 0, "the buy accrued a base-leg protocol fee");

    // The fee is real revenue sitting in the treasury — the sweep must
    // subtract it, not pay it out. A harvest is a separate, future lever.
    let dest = f.base_ata(&taker.pubkey());
    let taker_base = f.token_balance(&dest);
    let base_mint = f.base_mint;
    let base_treasury = f.base_treasury;
    let meta = f
        .sweep_residual_meta(&admin, &base_mint, &base_treasury, &dest)
        .expect("sweep after a fee'd swap");

    let ev = common::events::sweep_residual(&meta);
    assert_eq!(ev.accrued_fee, accrued);
    assert_eq!(
        ev.vault_sum + ev.accrued_fee,
        ev.treasury_amount,
        "custody invariant holds after a fee'd swap"
    );
    assert_eq!(ev.swept, 0, "accrued revenue is not a residual");
    assert_eq!(f.token_balance(&dest), taker_base, "nothing paid out");
    assert_eq!(
        f.market_header().accrued_base_fee_atoms.get(),
        accrued,
        "the counter is subtracted, never touched"
    );
}

/// The handler's leg select (`is_base`) is a second hand-duplicated
/// `if/else` over the same pair of fields the matcher's accrual selects, so
/// the quote arm needs its own witness — every other test here sweeps the
/// base leg, which would leave a base/quote mix-up in the `else` branch
/// entirely uncovered.
#[test]
fn quote_leg_sweeps_its_own_treasury() {
    let mut f = Fixture::seeded(SEED_BASE, SEED_QUOTE);
    let admin = f.authority.insecure_clone();
    let recipient = f.funded_depositor(0, 0);
    let dest = f.quote_ata(&recipient.pubkey());

    let quote_mint = f.quote_mint;
    let quote_treasury = f.quote_treasury;
    f.donate(&quote_mint, &quote_treasury, 4_242);

    let meta = f
        .sweep_residual_meta(&admin, &quote_mint, &quote_treasury, &dest)
        .expect("sweep recovers the quote-leg donation");

    let ev = common::events::sweep_residual(&meta);
    assert_eq!(ev.mint, quote_mint.to_bytes());
    assert_eq!(ev.destination, dest.to_bytes());
    // Measured against the *quote* inventory, not the base leg.
    assert_eq!(ev.vault_sum, SEED_QUOTE);
    assert_eq!(ev.treasury_amount, SEED_QUOTE + 4_242);
    assert_eq!(ev.swept, 4_242);
    assert_eq!(f.token_balance(&dest), 4_242);
    assert_eq!(f.token_balance(&quote_treasury), SEED_QUOTE);
    // The base leg is untouched by a quote-leg sweep.
    assert_eq!(f.token_balance(&f.base_treasury), SEED_BASE);
}

/// A tombstoned vault still holds depositor claims, which is why the sum
/// runs over the whole slab rather than the active DLL. Pin it: if that walk
/// were ever "optimized" to the active list, a tombstoned vault's inventory
/// would drop out of `vault_sum` and the sweep would pay depositor principal
/// out to the admin's destination — value loss, not a wrong read-out.
#[test]
fn tombstoned_vault_inventory_is_not_sweepable() {
    let mut f = Fixture::seeded(SEED_BASE, SEED_QUOTE);
    let admin = f.authority.insecure_clone();
    let leader = f.authority.insecure_clone();
    let recipient = f.funded_depositor(0, 0);
    let dest = f.base_ata(&recipient.pubkey());

    // Leader tombstones the vault; its inventory stays put (depositor flows
    // remain open until it drains), so it leaves the active DLL while still
    // holding every atom in the treasury.
    f.close_vault(&leader, 0)
        .expect("leader tombstones the vault");
    assert_eq!(f.vault(0).base_atoms.get(), SEED_BASE, "inventory retained");
    assert_eq!(
        f.market_header().active_count.get(),
        0,
        "off the active DLL"
    );

    let base_mint = f.base_mint;
    let base_treasury = f.base_treasury;
    let meta = f
        .sweep_residual_meta(&admin, &base_mint, &base_treasury, &dest)
        .expect("sweep against a tombstoned-only slab");

    let ev = common::events::sweep_residual(&meta);
    assert_eq!(
        ev.vault_sum, SEED_BASE,
        "the tombstoned sector's inventory is still counted as claimed"
    );
    assert_eq!(ev.swept, 0, "nothing is sweepable");
    assert_eq!(f.token_balance(&dest), 0, "no depositor principal paid out");
    assert_eq!(f.token_balance(&base_treasury), SEED_BASE);
}

#[test]
fn non_admin_cannot_sweep() {
    let mut f = Fixture::seeded(SEED_BASE, SEED_QUOTE);
    let outsider = f.funded_depositor(0, 0);
    let dest = f.base_ata(&outsider.pubkey());
    let base_mint = f.base_mint;
    let base_treasury = f.base_treasury;
    f.donate(&base_mint, &base_treasury, 500);

    let err = f
        .sweep_residual(&outsider, &base_mint, &base_treasury, &dest)
        .expect_err("a non-admin signer must be rejected");
    common::assert_program_error(&err, dropset::DropsetError::Unauthorized);
    assert_eq!(f.token_balance(&dest), 0, "no payout on the rejected call");
}

/// A market-owned ATA whose mint is neither market leg has no leg to
/// compute a residual against — there is no inventory field and no accrued
/// counter to subtract, so its whole balance would read as residual. The
/// `associated_token` constraint resolves (the account really is
/// `ata(market, mint)`), so the handler's explicit leg check is what
/// rejects it — mirroring `close_market_treasury`.
#[test]
fn non_leg_mint_rejected() {
    let mut f = Fixture::seeded(SEED_BASE, SEED_QUOTE);
    let admin = f.authority.insecure_clone();
    let recipient = f.funded_depositor(0, 0);
    let dest = f.base_ata(&recipient.pubkey());
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
        .sweep_residual(&admin, &other_mint, &other_treasury, &dest)
        .expect_err("a non-leg market-owned ATA must not be swept");
    common::assert_program_error(&err, dropset::DropsetError::NotAMarketTreasury);
}
